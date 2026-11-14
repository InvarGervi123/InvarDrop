//! Native GUI test sender. No plaintext key/code logging or production bypass.
//! Usage: receiver_probe ABS_PROFILE SOURCE_FILE EXACT_RECEIVER_DISCOVERY_NAME
//! stdin: status | approve <code read from receiver UI> | send | quit
use invardrop_lib::{
    discovery::Discovery,
    secure::{Service, files::Source},
};
use std::{
    io::{self, BufRead, Write},
    path::PathBuf,
    sync::{Arc, atomic::AtomicU16},
    time::Duration,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 4 {
        return Err("Expected profile, source and exact receiver name".into());
    }
    let port = Arc::new(AtomicU16::new(0));
    let service = Service::start(
        PathBuf::from(&args[1]),
        "InvarDrop GUI test sender".into(),
        port.clone(),
        |_| {},
    );
    let discovery = Discovery::start_with_port(port, |_| {});
    let target = tauri::async_runtime::block_on(async {
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                if service.snapshot().status == "unavailable" {
                    return Err("Secure identity unavailable");
                }
                if service.snapshot().status == "ready"
                    && let Some(peer) = discovery
                        .snapshot()
                        .devices
                        .into_iter()
                        .find(|d| d.name == args[3] && d.can_connect)
                {
                    return Ok(peer);
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
    })?
    .map_err(io::Error::other)?;
    let id =
        tauri::async_runtime::block_on(async { service.connect(target.id, target.endpoints) })?;
    println!("ready");
    io::stdout().flush()?;
    for line in io::stdin().lock().lines() {
        let line = line?;
        let result: Result<(), Box<dyn std::error::Error>> = (|| {
            match line.as_str() {
                "status" => {
                    let snapshot = service.snapshot();
                    let c = snapshot
                        .connections
                        .iter()
                        .find(|c| c.id == id)
                        .ok_or("No connection")?;
                    // Deliberately exclude pairing code and file contents.
                    println!(
                        "{}",
                        serde_json::json!({"phase":c.phase,"transfer":c.transfer.as_ref().map(|t| &t.status)})
                    );
                }
                "send" => {
                    let source = tauri::async_runtime::block_on(Source::open(
                        std::path::Path::new(&args[2]),
                    ))?;
                    service.send_file(&id, source)?;
                    println!("sent");
                }
                "quit" => return Ok(()),
                _ if line.starts_with("approve ") => {
                    service.respond(&id, true, line[8..].into())?;
                    println!("approved");
                }
                _ => return Err("Unknown command".into()),
            }
            Ok(())
        })();
        if result.is_err() {
            println!("error");
        }
        io::stdout().flush()?;
        if line == "quit" {
            break;
        }
    }
    tauri::async_runtime::block_on(service.stop());
    discovery.stop();
    Ok(())
}
