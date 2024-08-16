use std::time::Duration;

use tokio_stream::StreamExt;

use btleplug::{
    api::{Central, CentralEvent, Manager as _, Peripheral, ScanFilter},
    platform::{Adapter, Manager, PeripheralId},
};
use clap::Parser;
use lighthouse::Error;
use uuid::Uuid;

#[derive(Clone, Copy)]
enum State {
    Off,
    On,
    Standby,
}

async fn get_central(manager: &Manager) -> Adapter {
    let adapters = manager.adapters().await.unwrap();
    adapters.into_iter().next().unwrap()
}

async fn v1ctrl(adapter: &Adapter, peripheral_id: &PeripheralId, name: &str, state: State) -> Result<(), Error> {
    let bsid = &name[(name.len() - 4)..];

    let aa = u8::from_str_radix(&bsid[0..2], 16).map_err(Error::Std)?;
    let bb = u8::from_str_radix(&bsid[2..4], 16).map_err(Error::Std)?;
    let cc = u8::from_str_radix(&bsid[4..6], 16).map_err(Error::Std)?;
    let dd = u8::from_str_radix(&bsid[6..8], 16).map_err(Error::Std)?;

    let cmd = match state {
        State::Off => vec![
            0x12, 0x02, 0x00, 0x01, dd, cc, bb, aa, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ],
        State::On => vec![
            0x12, 0x00, 0x00, 0x00, dd, cc, bb, aa, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        ],
        _ => {
            return Err(Error::Message(
                "V1: Unknown State {state}, Available: [OFF|ON]",
            ))
        }
    };

    const UUID: &str = "0000cb01-0000-1000-8000-00805f9b34fb";
    let uuid = Uuid::parse_str(UUID).map_err(Error::Uuid)?;

    lighthouse::write(adapter, peripheral_id, &cmd, &uuid).await?;
    Ok(())
}

async fn v2ctrl(adapter: &Adapter, peripheral_id: &PeripheralId, state: State) -> Result<(), Error> {
    let cmd = match state {
        State::Off => vec![0x00],
        State::On => vec![0x01],
        State::Standby => vec![0x02],
    };

    const UUID: &str = "00001525-1212-efde-1523-785feabcd124";
    let uuid = Uuid::parse_str(UUID).map_err(Error::Uuid)?;

    lighthouse::write(adapter, peripheral_id, &cmd, &uuid).await?;
    Ok(())
}

async fn get_peripherals(central: &Adapter, id: &PeripheralId, state: State) -> Result<Option<std::time::Instant>, Error> {
    let peripheral = central.peripheral(id).await?;
    let properties = peripheral.properties().await;
    if let Ok(Some(properties)) = properties {
        if let Some(name) =  properties.local_name {
            let time = std::time::Instant::now();
            if name.starts_with("HTC BS") {
                v1ctrl(central, id, &name, state).await?;
            } else if name.starts_with("LHB-") {
                v2ctrl(central, id, state).await?;
            }
            return Ok(Some(time));
        }
    }
    Ok(None)
}

#[derive(Debug, Parser)]
struct Args {
    /// V1: [OFF|ON] [BSID] | V2: [OFF|ON|STANDBY]
    #[arg(short, long)]
    state: String,

    /// V1: Basestation BSID
    #[arg(short, long)]
    bsid: Option<String>,
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let args = Args::parse();

    let state = match args.state.to_uppercase().as_str() {
        "OFF" => State::Off,
        "ON" => State::On,
        "STANDBY" => State::Standby,
        _ => {
            return Err(Error::Message(
                "Unknown State {state}, Available: [OFF|ON|STANDBY]",
            ))
        }
    };

    let manager = Manager::new().await.map_err(Error::Btle)?;

    let central = get_central(&manager).await;
    let mut events = central.events().await?;
    central.start_scan(ScanFilter::default()).await?;

    let (tx, mut rx) = tokio::sync::mpsc::channel(100);
    tokio::spawn(async move {
        while let Some(event) = events.next().await {
            if let CentralEvent::DeviceDiscovered(id) = event {
                let central = central.clone();
                let hoge = tokio::spawn(async move {
                    get_peripherals(&central, &id, state).await
                });
                let _ = tx.send(hoge).await;
            }
        }
    });
    let mut prev = std::time::Instant::now();
    let mut duration = Duration::from_secs(10);
    loop {
        let timeout = tokio::time::sleep(duration);
        tokio::select! {
            _ = timeout => {
                break;
            }
            ret = rx.recv() => {
                let ret = ret.unwrap();
                let res = ret.await.unwrap()?;
                if let Some(time) = res {
                    let elapsed = time.duration_since(prev);
                    duration = elapsed * 10;
                    prev = time;
                }
            }
        }
    }
    Ok(())
}
