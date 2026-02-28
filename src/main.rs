//! Connects to our Bluetooth GATT service and exercises the characteristic.

use bluer::{Adapter, AdapterEvent, Device, Result, Uuid, gatt::remote::Characteristic};
use futures::{StreamExt, pin_mut};
use std::time::Duration;
use tokio::time::timeout;

/// Service UUID for GATT example.
const SERVICE_UUID: Uuid = Uuid::from_u128(0x0000181400001000800000805F9B34FB);

/// Characteristic UUID for GATT example.
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

async fn find_rsc_characteristic(device: &Device) -> Result<Option<Characteristic>> {
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

async fn exercise_characteristic(char: &Characteristic) -> Result<()> {
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
    adapter.set_powered(true).await?;

    {
        println!(
            "Discovering on Bluetooth adapter {} with address {}\n",
            adapter.name(),
            adapter.address().await?
        );

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

            match exercise_characteristic(&char).await {
                _ => (),
            }
        }
    }

    Ok(())
}
