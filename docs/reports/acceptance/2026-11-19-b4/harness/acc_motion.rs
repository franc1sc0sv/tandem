//! Acceptance helper (T-239): sends `UiCommand::SyntheticMotion` to a running agent over its
//! IPC and prints the reply. Usage: `acc_motion <data-dir> <dx> <dy> [count]`.

use std::path::PathBuf;
use std::time::Duration;

use tandem_proto::{Peer, UiCommand};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut a = std::env::args().skip(1);
    let dir = PathBuf::from(a.next().ok_or_else(|| anyhow::anyhow!("missing data dir"))?);
    let dx: i32 = a.next().unwrap_or_else(|| "10".into()).parse()?;
    let dy: i32 = a.next().unwrap_or_else(|| "0".into()).parse()?;
    let n: u32 = a.next().unwrap_or_else(|| "1".into()).parse()?;
    let mut c = tandem_hw::conn::connect(&dir, Peer::Ui, Duration::from_secs(5)).await?;
    for _ in 0..n {
        let r = c.request(UiCommand::SyntheticMotion { dx, dy }).await?;
        println!("{r:?}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    Ok(())
}
