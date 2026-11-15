pub mod discovery;
mod processing;
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

// Filesystem authority stays in Rust: IPC accepts only connection/transfer IDs.
// A source path can enter this command only through the native picker.
#[tauri::command]
async fn file_choose(
    id: String,
    processing: tauri::State<'_, processing::Processing>,
    service: tauri::State<'_, secure::Service>,
) -> Result<(), String> {
    service.can_send_file(&id).map_err(|e| e.to_string())?;
    let supports_bundles = service
        .snapshot()
        .connections
        .iter()
        .find(|c| c.id == id)
        .is_some_and(|c| c.supports_bundles);
    let dialog = rfd::AsyncFileDialog::new().set_title("Choose files to send");
    let selected = if supports_bundles {
        dialog.pick_files().await
    } else {
        dialog.pick_file().await.map(|file| vec![file])
    };
    let Some(files) = selected else {
        return Ok(());
    };
    if processing
        .select(
            &service,
            id.clone(),
            files.iter().map(|f| f.path().to_owned()).collect(),
            false,
        )
        .await?
    {
        return Ok(());
    }
    if files.len() == 1 {
        let source = secure::files::Source::open(files[0].path())
            .await
            .map_err(|e| e.to_string())?;
        if service
            .snapshot()
            .connections
            .iter()
            .any(|c| c.id == id && c.supports_resume)
        {
            service
                .send_resumable_file(&id, files[0].path())
                .await
                .map_err(|e| e.to_string())
        } else {
            service.send_file(&id, source).map_err(|e| e.to_string())
        }
    } else {
        let paths = files.into_iter().map(|f| f.path().to_owned()).collect();
        let source = secure::bundle::BundleSource::files(paths)
            .await
            .map_err(|e| e.to_string())?;
        if service
            .snapshot()
            .connections
            .iter()
            .any(|c| c.id == id && c.supports_resume)
        {
            service
                .send_resumable_bundle(&id, source)
                .await
                .map_err(|e| e.to_string())
        } else {
            service.send_bundle(&id, source).map_err(|e| e.to_string())
        }
    }
}
#[tauri::command]
async fn folder_choose(
    id: String,
    processing: tauri::State<'_, processing::Processing>,
    service: tauri::State<'_, secure::Service>,
) -> Result<(), String> {
    service.can_send_file(&id).map_err(|e| e.to_string())?;
    let Some(folder) = rfd::AsyncFileDialog::new()
        .set_title("Choose a folder to send")
        .pick_folder()
        .await
    else {
        return Ok(());
    };
    if processing
        .select(&service, id.clone(), vec![folder.path().to_owned()], true)
        .await?
    {
        return Ok(());
    }
    let source = secure::bundle::BundleSource::folder(folder.path())
        .await
        .map_err(|e| e.to_string())?;
    if service
        .snapshot()
        .connections
        .iter()
        .any(|c| c.id == id && c.supports_resume)
    {
        service
            .send_resumable_bundle(&id, source)
            .await
            .map_err(|e| e.to_string())
    } else {
        service.send_bundle(&id, source).map_err(|e| e.to_string())
    }
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
    let app = tauri::Builder::default()
        .manage(processing::Processing::default())
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
            app.manage(secure::Service::start(
                root,
                app.state::<Discovery>().snapshot().local_device.name,
                port,
                move |snapshot| {
                    let _ = handle.emit_to("main", secure::EVENT, snapshot);
                },
            ));
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
            secure_unblock,
            file_choose,
            processing_dismiss,
            processing_snapshot,
            processing_converter_status,
            processing_converter,
            processing_decide,
            processing_cancel,
            processing_extract,
            resume_transfer,
            resume_discard,
            folder_choose,
            file_respond,
            file_cancel
        ])
        .build(tauri::generate_context!())
        .expect("error while building InvarDrop");
    app.run(|handle, event| {
        if let tauri::RunEvent::Exit = event {
            handle.state::<processing::Processing>().shutdown();
            tauri::async_runtime::block_on(handle.state::<secure::Service>().stop());
            handle.state::<Discovery>().stop();
        }
    });
}
