pub mod discovery;
pub mod secure;

use discovery::{Discovery, Snapshot};
use std::sync::{Arc, atomic::AtomicU16};
use tauri::{Emitter, Manager};

#[tauri::command]
fn discovery_snapshot(discovery: tauri::State<'_, Discovery>) -> Snapshot {
    discovery.snapshot()
}
#[tauri::command]
fn security_snapshot(service: tauri::State<'_, secure::Service>) -> secure::Snapshot {
    service.snapshot()
}
#[tauri::command]
async fn secure_connect(
    id: String,
    discovery: tauri::State<'_, Discovery>,
    service: tauri::State<'_, secure::Service>,
) -> Result<String, String> {
    let peer = discovery
        .snapshot()
        .devices
        .into_iter()
        .find(|d| d.id == id && d.can_connect)
        .ok_or("Device is no longer available")?;
    service
        .connect(peer.id, peer.endpoints)
        .map_err(|e| e.to_string())
}
#[tauri::command]
async fn secure_respond(
    id: String,
    approve: bool,
    code: String,
    service: tauri::State<'_, secure::Service>,
) -> Result<(), String> {
    service
        .respond(&id, approve, code)
        .map_err(|e| e.to_string())
}
#[tauri::command]
async fn secure_cancel(
    id: String,
    service: tauri::State<'_, secure::Service>,
) -> Result<(), String> {
    service.cancel(&id).map_err(|e| e.to_string())
}
#[tauri::command]
async fn secure_reconnect(
    id: String,
    service: tauri::State<'_, secure::Service>,
) -> Result<String, String> {
    service.reconnect(&id).map_err(|e| e.to_string())
}
#[tauri::command]
async fn secure_revoke(
    id: String,
    service: tauri::State<'_, secure::Service>,
) -> Result<(), String> {
    service.revoke(id).await.map_err(|e| e.to_string())
}
#[tauri::command]
async fn secure_unblock(
    id: String,
    service: tauri::State<'_, secure::Service>,
) -> Result<(), String> {
    service.unblock(id).await.map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .setup(|app| {
            let port = Arc::new(AtomicU16::new(0));
            let handle = app.handle().clone();
            app.manage(Discovery::start_with_port(port.clone(), move |snapshot| {
                let _ = handle.emit_to("main", discovery::EVENT_NAME, snapshot);
            }));
            // A separate profile enables independent native processes without sharing keys/locks.
            let root = std::env::var_os("INVARDROP_PROFILE")
                .map(std::path::PathBuf::from)
                .unwrap_or(app.path().app_data_dir()?.join("security-v1"));
            let handle = app.handle().clone();
            app.manage(secure::Service::start(root, port, move |snapshot| {
                let _ = handle.emit_to("main", secure::EVENT, snapshot);
            }));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            discovery_snapshot,
            security_snapshot,
            secure_connect,
            secure_respond,
            secure_cancel,
            secure_reconnect,
            secure_revoke,
            secure_unblock
        ])
        .build(tauri::generate_context!())
        .expect("error while building InvarDrop");
    app.run(|handle, event| {
        if let tauri::RunEvent::Exit = event {
            tauri::async_runtime::block_on(handle.state::<secure::Service>().stop());
            handle.state::<Discovery>().stop();
        }
    });
}
