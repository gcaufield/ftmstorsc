//! Connects to our Bluetooth GATT service and exercises the characteristic.

mod ble;

use bluer::agent::Agent;
use std::sync::Arc;
use tokio::sync::Mutex;

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

    println!(
        "Discovering on Bluetooth adapter {} with address {}\n",
        adapter.name(),
        adapter.address().await?
    );

    // Start Advertising
    let driver = ble::Driver::new(adapter).await?;
    let device = driver.search_for_device().await?;

    loop {
        if device.is_connected().await? {
            device.disconnect().await?;
        }

        match ble::connect_device(&device).await {
            Ok(()) => (),
            Err(_) => continue,
        }

        let char = match ble::find_treadmill_data(&device).await {
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

        match ble::exercise_characteristic(&device, &char, driver.current_speed.clone()).await {
            _ => (),
        }
    }

    Ok(())
}
