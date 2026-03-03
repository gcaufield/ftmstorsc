//! Connects to our Bluetooth GATT service and exercises the characteristic.

use bluer::{
    Adapter, AdapterEvent, Device, Result, Uuid,
    adv::Advertisement,
    agent::Agent,
    gatt::local::{
        Application, Characteristic, CharacteristicNotify, CharacteristicNotifyMethod,
        CharacteristicRead, Profile, Service,
    },
    gatt::remote::Characteristic as RemoteCharacteristic,
};
use futures::{FutureExt, StreamExt, pin_mut};
use std::{collections::HashSet, sync::Arc, time::Duration};
use tokio::{
    sync::Mutex,
    time::{sleep, timeout},
};

/// Service UUID for GATT example.
const RSC_SERVICE_UUID: Uuid = Uuid::from_u128(0x0000181400001000800000805F9B34FB);
const FTM_SERVICE_UUID: Uuid = Uuid::from_u128(0x0000182600001000800000805F9B34FB);

/// Characteristic UUID for GATT example.
const RSC_FEATURE_UUID: Uuid = Uuid::from_u128(0x00002A5400001000800000805F9B34FB);
const RSC_MEASUREMENT_UUID: Uuid = Uuid::from_u128(0x00002A5300001000800000805F9B34FB);
const TREADMILL_DATA_UUID: Uuid = Uuid::from_u128(0x00002ACD00001000800000805F9B34FB);

async fn has_service(device: &Device) -> Result<bool> {
    let addr = device.address();
    let uuids = device.uuids().await?.unwrap_or_default();
    println!("Discovered device {} with service UUIDs {:?}", addr, &uuids);
    let md = device.manufacturer_data().await?;
    println!("    Manufacturer data: {:x?}", &md);

    return Ok(uuids.contains(&FTM_SERVICE_UUID));
}
async fn connect_device(device: &Device) -> Result<()> {
    if !device.is_connected().await? {
        println!("    Connecting...");
        let mut retries = 10;
        loop {
            match device.connect().await {
                Ok(()) => break,
                Err(err) if retries > 0 => {
                    println!("    Connect error: {}", &err);
                    retries -= 1;
                }
                Err(err) => return Err(err),
            }
        }
        println!("    Connected");
    } else {
        println!("    Already connected");
    }
    Ok(())
}

async fn find_treadmill_data(device: &Device) -> Result<Option<RemoteCharacteristic>> {
    println!("    Enumerating services...");
    for service in device.services().await? {
        let uuid = service.uuid().await?;
        println!("    Service UUID: {}", &uuid);
        println!("    Service data: {:?}", service.all_properties().await?);
        if uuid == FTM_SERVICE_UUID {
            println!("    Found our service!");
            for char in service.characteristics().await? {
                let uuid = char.uuid().await?;
                println!("    Characteristic UUID: {}", &uuid);
                println!(
                    "    Characteristic data: {:?}",
                    char.all_properties().await?
                );
                if uuid == TREADMILL_DATA_UUID {
                    println!("    Found our characteristic!");
                    return Ok(Some(char));
                }
            }
        }
    }

    println!("    Not found!");

    Ok(None)
}

fn get_speed(data: &Vec<u8>) -> Option<u32> {
    if (data[0] & 0x01) != 0 {
        None
    } else {
        Some(data[2] as u32 | ((data[3] as u32) << 8))
    }
}

async fn exercise_characteristic(
    device: &Device,
    char: &RemoteCharacteristic,
    notify_val: Arc<Mutex<Vec<u8>>>,
) -> Result<()> {
    println!("    Starting notification session");
    {
        let notify = char.notify().await?;
        pin_mut!(notify);
        loop {
            match timeout(Duration::from_secs(3), notify.next()).await {
                Ok(value) => match value {
                    Some(val) => {
                        println!("    Notification value: {:x?}", &val);
                        let speed = match get_speed(&val) {
                            Some(sp) => sp,
                            None => continue,
                        };

                        println!("   Recived Speed: {} dam/hour", speed);
                        let speed_mps_256 = (speed * 256 * 10) / 3600;

                        let mut value = notify_val.lock().await;
                        *value = vec![0x00, speed_mps_256 as u8, (speed_mps_256 >> 8) as u8, 0x00];
                    }
                    None => break,
                },
                Err(_) => {
                    if device.is_connected().await? {
                        println!("    Device still connected");
                        continue;
                    }
                    println!("    Notification session was terminated");
                    break;
                }
            }
        }
        println!("    Stopping notification session");
    }

    Ok(())
}

async fn search_for_device(adapter: &Adapter) -> Result<Device> {
    let discover = adapter.discover_devices().await?;
    pin_mut!(discover);
    while let Some(evt) = discover.next().await {
        match evt {
            AdapterEvent::DeviceAdded(addr) => {
                let device = adapter.device(addr)?;
                if has_service(&device).await? {
                    println!("    Device provides our service!");
                    return Ok(device);
                }
            }
            _ => (),
        }
    }

    Err(bluer::Error {
        kind: bluer::ErrorKind::NotAvailable,
        message: String::new(),
    })
}

fn build_rsc_feature() -> Characteristic {
    Characteristic {
        uuid: RSC_FEATURE_UUID,
        read: Some(CharacteristicRead {
            read: true,
            fun: Box::new(move |_| async move { Ok(vec![0x00, 0x00]) }.boxed()),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn build_rsc_measurement(value_notify: Arc<Mutex<Vec<u8>>>) -> Characteristic {
    Characteristic {
        uuid: RSC_MEASUREMENT_UUID,
        notify: Some(CharacteristicNotify {
            notify: true,
            method: CharacteristicNotifyMethod::Fun(Box::new(move |mut notifier| {
                let value = value_notify.clone();
                async move {
                    tokio::spawn(async move {
                        println!(
                            "Notification session start with confirming={:?}",
                            notifier.confirming()
                        );
                        loop {
                            {
                                let value = value.lock().await;
                                println!("Notifying with value {:x?}", &*value);
                                if let Err(err) = notifier.notify(value.to_vec()).await {
                                    println!("Notification error: {}", &err);
                                    break;
                                }
                            }
                            sleep(Duration::from_millis(1250)).await;
                        }
                        println!("Notification session stop");
                    });
                }
                .boxed()
            })),
            ..Default::default()
        }),
        ..Default::default()
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> bluer::Result<()> {
    env_logger::init();
    let session = bluer::Session::new().await?;
    let adapter = session.default_adapter().await?;
    let _agent_handle = session
        .register_agent(Agent {
            ..Default::default()
        })
        .await?;
    adapter.set_powered(true).await?;
    adapter.set_pairable(true).await?;

    let value = Arc::new(Mutex::new(vec![]));

    {
        println!(
            "Discovering on Bluetooth adapter {} with address {}\n",
            adapter.name(),
            adapter.address().await?
        );

        // Start Advertising
        let le_advertisement = Advertisement {
            service_uuids: vec![RSC_SERVICE_UUID].into_iter().collect(),
            discoverable: Some(true),
            local_name: Some("Bridge".to_string()),
            ..Default::default()
        };

        let _adv_handle = adapter.advertise(le_advertisement).await?;
        let app = Application {
            services: vec![Service {
                uuid: RSC_SERVICE_UUID,
                primary: true,
                characteristics: vec![build_rsc_feature(), build_rsc_measurement(value.clone())],
                ..Default::default()
            }],
            ..Default::default()
        };

        let _app_handle = adapter.serve_gatt_application(app).await?;
        let _profile_handle = adapter.register_gatt_profile(Profile {
            uuids: HashSet::from([FTM_SERVICE_UUID]),
            ..Default::default()
        });

        let device = search_for_device(&adapter).await?;

        loop {
            if device.is_connected().await? {
                device.disconnect().await?;
            }

            match connect_device(&device).await {
                Ok(()) => (),
                Err(_) => continue,
            }

            let char = match find_treadmill_data(&device).await {
                Ok(res) => match res {
                    Some(char) => char,
                    None => {
                        println!("   Char Not Found");
                        break;
                    }
                },
                Err(e) => {
                    println!("  Error: {}", e);
                    continue;
                }
            };

            match exercise_characteristic(&device, &char, value.clone()).await {
                _ => (),
            }
        }
    }

    Ok(())
}
