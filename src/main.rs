//! Connects to our Bluetooth GATT service and exercises the characteristic.

use bluer::{
    Adapter, AdapterEvent, Device, Result, Uuid,
    adv::Advertisement,
    agent::Agent,
    gatt::local::{
        Application, Characteristic, CharacteristicNotify, CharacteristicNotifyMethod,
        CharacteristicRead, Service,
    },
    gatt::remote::Characteristic as RemoteCharacteristic,
};
use futures::{FutureExt, StreamExt, pin_mut};
use std::{sync::Arc, time::Duration};
use tokio::{
    sync::Mutex,
    time::{sleep, timeout},
};

/// Service UUID for GATT example.
const SERVICE_UUID: Uuid = Uuid::from_u128(0x0000181400001000800000805F9B34FB);

/// Characteristic UUID for GATT example.
const FEATURE_UUID: Uuid = Uuid::from_u128(0x00002A5400001000800000805F9B34FB);
const CHARACTERISTIC_UUID: Uuid = Uuid::from_u128(0x00002A5300001000800000805F9B34FB);

/// Manufacturer id for LE advertisement.
#[allow(dead_code)]
const MANUFACTURER_ID: u16 = 0xf00d;

async fn has_service(device: &Device) -> Result<bool> {
    let addr = device.address();
    let uuids = device.uuids().await?.unwrap_or_default();
    println!("Discovered device {} with service UUIDs {:?}", addr, &uuids);
    let md = device.manufacturer_data().await?;
    println!("    Manufacturer data: {:x?}", &md);

    return Ok(uuids.contains(&SERVICE_UUID));
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

async fn find_rsc_characteristic(device: &Device) -> Result<Option<RemoteCharacteristic>> {
    println!("    Enumerating services...");
    for service in device.services().await? {
        let uuid = service.uuid().await?;
        println!("    Service UUID: {}", &uuid);
        println!("    Service data: {:?}", service.all_properties().await?);
        if uuid == SERVICE_UUID {
            println!("    Found our service!");
            for char in service.characteristics().await? {
                let uuid = char.uuid().await?;
                println!("    Characteristic UUID: {}", &uuid);
                println!(
                    "    Characteristic data: {:?}",
                    char.all_properties().await?
                );
                if uuid == CHARACTERISTIC_UUID {
                    println!("    Found our characteristic!");
                    return Ok(Some(char));
                }
            }
        }
    }

    println!("    Not found!");

    Ok(None)
}

async fn exercise_characteristic(
    char: &RemoteCharacteristic,
    notify_val: Arc<Mutex<Vec<u8>>>,
) -> Result<()> {
    println!("    Characteristic flags: {:?}", char.flags().await?);

    if char.flags().await?.read {
        println!("    Reading characteristic value");
        let value = char.read().await?;
        println!("    Read value: {:x?}", &value);
    }

    println!("    Starting notification session");
    {
        let notify = char.notify().await?;
        pin_mut!(notify);
        loop {
            match timeout(Duration::from_secs(3), notify.next()).await {
                Ok(value) => match value {
                    Some(val) => {
                        println!("    Notification value: {:x?}", &val);
                        let mut value = notify_val.lock().await;
                        *value = val;
                    }
                    None => break,
                },
                Err(_) => {
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

#[tokio::main(flavor = "current_thread")]
async fn main() -> bluer::Result<()> {
    env_logger::init();
    let session = bluer::Session::new().await?;
    let adapter = session.default_adapter().await?;
    let agent_handle = session
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
            service_uuids: vec![SERVICE_UUID].into_iter().collect(),
            discoverable: Some(true),
            local_name: Some("Bridge".to_string()),
            ..Default::default()
        };

        let adv_handle = adapter.advertise(le_advertisement).await?;
        let value_notify = value.clone();
        let app = Application {
            services: vec![Service {
                uuid: SERVICE_UUID,
                primary: true,
                characteristics: vec![
                    Characteristic {
                        uuid: FEATURE_UUID,
                        read: Some(CharacteristicRead {
                            read: true,
                            fun: Box::new(move |_| async move { Ok(vec![0x00, 0x00]) }.boxed()),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                    Characteristic {
                        uuid: CHARACTERISTIC_UUID,
                        notify: Some(CharacteristicNotify {
                            notify: true,
                            method: CharacteristicNotifyMethod::Fun(Box::new(
                                move |mut notifier| {
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
                                                    if let Err(err) =
                                                        notifier.notify(value.to_vec()).await
                                                    {
                                                        println!("Notification error: {}", &err);
                                                        break;
                                                    }
                                                }
                                                sleep(Duration::from_millis(500)).await;
                                            }
                                            println!("Notification session stop");
                                        });
                                    }
                                    .boxed()
                                },
                            )),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }],
            ..Default::default()
        };

        let app_handle = adapter.serve_gatt_application(app).await?;

        let device = search_for_device(&adapter).await?;

        loop {
            if device.is_connected().await? {
                device.disconnect().await?;
            }

            match connect_device(&device).await {
                Ok(()) => (),
                Err(_) => continue,
            }

            let char = match find_rsc_characteristic(&device).await {
                Ok(res) => match res {
                    Some(char) => char,
                    None => break,
                },
                Err(_) => continue,
            };

            match exercise_characteristic(&char, value.clone()).await {
                _ => (),
            }
        }
    }

    Ok(())
}
