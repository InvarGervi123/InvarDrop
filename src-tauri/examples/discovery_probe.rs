//! Real-network diagnostic (no fake peers): cargo run --example discovery_probe -- 90
//! Run in two independent processes on the same host or on separate LAN devices.
use invardrop_lib::discovery::Discovery;
use std::time::{Duration, Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let seconds = std::env::args()
        .nth(1)
        .map(|s| s.parse::<u64>())
        .transpose()?
        .unwrap_or(90);
    let started = Instant::now();
    let discovery = Discovery::start(move |snapshot| {
        println!("{}s: {:?}", started.elapsed().as_secs(), snapshot);
    });
    println!("Local session: {:?}", discovery.snapshot().local_device);
    std::thread::sleep(Duration::from_secs(seconds));
    discovery.stop();
    Ok(())
}
