#[cfg(any(target_os = "android", test))]
mod android_files;
#[cfg_attr(not(any(target_os = "linux", target_os = "android")), allow(dead_code))]
mod bluetooth;
pub mod discovery;
#[cfg(any(target_os = "android", test))]
mod network_permission;
mod processing;
pub mod secure;
mod share;

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
    bluetooth: tauri::State<'_, Arc<bluetooth::Controller>>,
) -> Result<String, String> {
    let peer = discovery
        .snapshot()
        .devices
        .into_iter()
        .find(|d| d.id == id && d.can_connect);
    if peer.is_none() && !bluetooth.has_peer(&id) {
        return Err("Device is no longer available".into());
    }
    service
        .connect_nearby(id, peer.map(|p| p.endpoints).unwrap_or_default())
        .map_err(|e| e.to_string())
}
#[tauri::command]
fn bluetooth_snapshot(
    bluetooth: tauri::State<'_, Arc<bluetooth::Controller>>,
) -> bluetooth::Snapshot {
    bluetooth.snapshot()
}
#[tauri::command]
async fn bluetooth_enable(
    bluetooth: tauri::State<'_, Arc<bluetooth::Controller>>,
) -> Result<(), String> {
    bluetooth.inner().enable(true).await
}
#[tauri::command]
async fn bluetooth_start(
    bluetooth: tauri::State<'_, Arc<bluetooth::Controller>>,
) -> Result<(), String> {
    bluetooth.inner().enable(false).await
}
#[tauri::command]
fn bluetooth_scan(bluetooth: tauri::State<'_, Arc<bluetooth::Controller>>) {
    bluetooth.scan();
}
#[tauri::command]
async fn bluetooth_disable(
    bluetooth: tauri::State<'_, Arc<bluetooth::Controller>>,
    service: tauri::State<'_, secure::Service>,
) -> Result<(), String> {
    service.disconnect_bluetooth();
    bluetooth.disable().await;
    Ok(())
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

#[tauri::command]
fn processing_dismiss(
    id: String,
    processing: tauri::State<'_, processing::Processing>,
) -> Result<(), String> {
    processing.dismiss(&id)
}
#[tauri::command]
fn processing_snapshot(
    processing: tauri::State<'_, processing::Processing>,
) -> Vec<processing::View> {
    processing.views()
}
#[tauri::command]
fn processing_converter_status(processing: tauri::State<'_, processing::Processing>) -> bool {
    processing.converter_configured()
}
#[cfg(not(target_os = "android"))]
#[tauri::command]
async fn processing_converter(
    processing: tauri::State<'_, processing::Processing>,
) -> Result<(), String> {
    if let Some(file) = rfd::AsyncFileDialog::new()
        .set_title("Choose a trusted FFmpeg executable (optional)")
        .pick_file()
        .await
    {
        processing.configure(file.path().to_owned())?;
    }
    Ok(())
}
#[cfg(target_os = "android")]
#[tauri::command]
async fn processing_converter() -> Result<(), String> {
    Err("Selecting an external FFmpeg executable is not supported on Android".into())
}
#[tauri::command]
async fn processing_decide(
    id: String,
    convert: bool,
    processing: tauri::State<'_, processing::Processing>,
    service: tauri::State<'_, secure::Service>,
) -> Result<(), String> {
    processing.decide(&service, id, convert).await
}
#[tauri::command]
fn processing_cancel(
    id: String,
    processing: tauri::State<'_, processing::Processing>,
) -> Result<(), String> {
    processing.cancel(&id)
}
#[tauri::command]
async fn processing_extract(
    connection_id: String,
    transfer_id: String,
    processing: tauri::State<'_, processing::Processing>,
    service: tauri::State<'_, secure::Service>,
) -> Result<(), String> {
    processing
        .extract(&service, &connection_id, &transfer_id)
        .await
}
#[tauri::command]
async fn resume_transfer(
    id: String,
    transfer_id: String,
    service: tauri::State<'_, secure::Service>,
) -> Result<(), String> {
    service
        .resume_transfer(&id, &transfer_id)
        .await
        .map_err(|e| e.to_string())
}
#[tauri::command]
async fn resume_discard(
    transfer_id: String,
    service: tauri::State<'_, secure::Service>,
) -> Result<(), String> {
    service
        .discard_resume(&transfer_id)
        .await
        .map_err(|e| e.to_string())
}
#[tauri::command]
async fn file_respond(
    id: String,
    transfer_id: String,
    accept: bool,
    service: tauri::State<'_, secure::Service>,
) -> Result<(), String> {
    service
        .respond_file(&id, &transfer_id, accept)
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn file_cancel(
    id: String,
    transfer_id: String,
    service: tauri::State<'_, secure::Service>,
) -> Result<(), String> {
    service
        .cancel_file(&id, &transfer_id)
        .map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default();
    #[cfg(target_os = "android")]
    let builder = builder.plugin(android_files::init());
    #[cfg(target_os = "android")]
    let builder = builder.plugin(bluetooth::android_plugin());
    let app = builder
        .manage(share::Composer::default())
        .setup(|app| {
            let handle = app.handle().clone();
            app.manage(processing::Processing::with_updates(move || {
                let _ = handle.emit_to("main", processing::EVENT, ());
            }));
            #[cfg(target_os = "android")]
            android_files::request_network_permission(app.handle());
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
            app.manage(secure::Service::start_configured(
                root,
                Some(secure::downloads::Destination::for_app(app.handle())),
                app.state::<Discovery>().snapshot().local_device.name,
                port,
                move |snapshot| {
                    let _ = handle.emit_to("main", secure::EVENT, snapshot);
                },
            ));
            let local = app.state::<Discovery>().snapshot().local_device;
            let handle = app.handle().clone();
            let bluetooth = bluetooth::Controller::new(
                local.id,
                local.name,
                #[cfg(target_os = "android")]
                app.handle().clone(),
                move |snapshot| {
                    let _ = handle.emit_to("main", bluetooth::EVENT, snapshot);
                },
            );
            app.state::<secure::Service>()
                .attach_bluetooth(bluetooth.clone());
            app.manage(bluetooth);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            discovery_snapshot,
            bluetooth_snapshot,
            bluetooth_enable,
            bluetooth_start,
            bluetooth_scan,
            bluetooth_disable,
            security_snapshot,
            secure_connect,
            secure_respond,
            secure_cancel,
            secure_reconnect,
            secure_revoke,
            secure_unblock,
            share::share_pick,
            share::share_clear,
            share::share_send,
            processing_dismiss,
            processing_snapshot,
            processing_converter_status,
            processing_converter,
            processing_decide,
            processing_cancel,
            processing_extract,
            resume_transfer,
            resume_discard,
            file_respond,
            file_cancel
        ])
        .build(tauri::generate_context!())
        .expect("error while building InvarDrop");
    app.run(|handle, event| {
        if let tauri::RunEvent::Exit = event {
            handle.state::<processing::Processing>().shutdown();
            tauri::async_runtime::block_on(handle.state::<Arc<bluetooth::Controller>>().disable());
            tauri::async_runtime::block_on(handle.state::<secure::Service>().stop());
            handle.state::<Discovery>().stop();
        }
    });
}
