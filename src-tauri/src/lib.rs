pub mod discovery;

use discovery::{Discovery, Snapshot};
use tauri::{Emitter, Manager};

#[tauri::command]
fn discovery_snapshot(discovery: tauri::State<'_, Discovery>) -> Snapshot {
    discovery.snapshot()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .setup(|app| {
            let handle = app.handle().clone();
            app.manage(Discovery::start(move |snapshot| {
                // Emit only to our local main WebView, not arbitrary windows.
                if let Err(error) = handle.emit_to("main", discovery::EVENT_NAME, snapshot) {
                    eprintln!("Could not deliver discovery update: {error}");
                }
            }));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![discovery_snapshot])
        .build(tauri::generate_context!())
        .expect("error while building InvarDrop");
    app.run(|handle, event| {
        if let tauri::RunEvent::Exit = event {
            handle.state::<Discovery>().stop();
        }
    });
}
