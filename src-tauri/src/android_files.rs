//! SAF imports are immutable private snapshots, reused by the existing resume
//! catalog. Content URIs never masquerade as filesystem paths or cross IPC.
use crate::secure;
#[cfg(target_os = "android")]
use crate::secure::Service;
use std::path::{Path, PathBuf};

fn validate_imports(root: &Path, names: &[String]) -> Result<Vec<PathBuf>, String> {
    if names.len() > 32 {
        return Err("Select at most 32 files".into());
    }
    let mut seen = std::collections::HashSet::new();
    let mut total = 0u64;
    names
        .iter()
        .map(|name| {
            secure::files::validate_name(name).map_err(|e| e.to_string())?;
            if !seen.insert(name.to_lowercase()) {
                return Err("Selected filenames collide".into());
            }
            let path = root.join(name);
            let metadata = secure::bundle::regular_metadata(&path).map_err(|e| e.to_string())?;
            if !metadata.is_file() {
                return Err("Selected document is not a regular file".into());
            }
            total = total
                .checked_add(metadata.len())
                .ok_or("Selected files are too large")?;
            if total > 4 * 1024 * 1024 * 1024 {
                return Err("Selected files exceed the 4 GiB import limit".into());
            }
            Ok(path)
        })
        .collect()
}

#[cfg(target_os = "android")]
mod mobile {
    use super::*;
    use tauri::{
        Manager,
        plugin::{Builder, PluginHandle, TauriPlugin},
    };
    pub struct Picker {
        pub(crate) handle: PluginHandle<tauri::Wry>,
        selection: tokio::sync::Mutex<()>,
    }
    pub fn init() -> TauriPlugin<tauri::Wry> {
        Builder::new("android-files")
            .setup(|app, api| {
                let handle = api.register_android_plugin(
                    "io.github.invargervi.invardrop",
                    "FilePickerPlugin",
                )?;
                app.manage(Picker {
                    handle,
                    selection: tokio::sync::Mutex::new(()),
                });
                Ok(())
            })
            .build()
    }
    pub fn request_network_permission(app: &tauri::AppHandle) {
        let handle = app.state::<Picker>().handle.clone();
        tauri::async_runtime::spawn(async move {
            #[derive(serde::Deserialize)]
            struct Permission {
                granted: bool,
            }
            let granted = handle
                .run_mobile_plugin_async::<Permission>("networkPermission", ())
                .await
                .is_ok_and(|result| result.granted);
            if !granted {
                eprintln!("Android local network permission unavailable or denied");
            }
            crate::network_permission::NETWORK.resolve(granted);
        });
    }
    #[derive(serde::Deserialize)]
    struct Selection {
        names: Vec<String>,
    }
    #[derive(serde::Serialize)]
    struct Request {
        directory: String,
        multiple: bool,
    }
    pub async fn choose(picker: &Picker, service: &Service, id: &str) -> Result<(), String> {
        let _guard = picker
            .selection
            .try_lock()
            .map_err(|_| "A file selection is already in progress")?;
        service.can_send_file(id).map_err(|e| e.to_string())?;
        service.cleanup_prepared().await?;
        let snapshot = service.snapshot();
        let connection = snapshot
            .connections
            .iter()
            .find(|c| c.id == id)
            .ok_or("Connection is no longer available")?;
        let profile = Path::new(&snapshot.receive_directory)
            .parent()
            .ok_or("Invalid profile")?;
        let root = profile
            .canonicalize()
            .map_err(|_| "Private storage is unavailable")?
            .join("prepared");
        std::fs::create_dir_all(&root).map_err(|_| "Private storage is unavailable")?;
        secure::bundle::regular_metadata(&root).map_err(|e| e.to_string())?;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| "Private storage is unavailable")?;
        if std::fs::read_dir(&root)
            .map_err(|_| "Private storage is unavailable")?
            .take(8)
            .count()
            >= 8
        {
            return Err(
                "Prepared storage is full; complete or discard old resumable transfers".into(),
            );
        }
        let directory = tempfile::Builder::new()
            .prefix("documents-")
            .tempdir_in(&root)
            .map_err(|_| "Private storage is unavailable")?;
        let selected: Selection = picker
            .handle
            .run_mobile_plugin_async(
                "choose",
                Request {
                    directory: directory.path().to_string_lossy().into_owned(),
                    multiple: connection.supports_bundles,
                },
            )
            .await
            .map_err(|e| e.to_string())?;
        if selected.names.is_empty() {
            return Ok(());
        }
        let paths = validate_imports(directory.path(), &selected.names)?;
        // Sync the directory before a durable checkpoint can reference the copy.
        std::fs::File::open(directory.path())
            .and_then(|f| f.sync_all())
            .map_err(|_| "Cannot sync selected files")?;
        std::fs::File::open(&root)
            .and_then(|f| f.sync_all())
            .map_err(|_| "Cannot sync private storage")?;
        service.can_send_file(id).map_err(|e| e.to_string())?;
        // A transport error may still leave a durable source ticket. Keep copies
        // until the catalog no longer references them; never delete on send error.
        let _stable = directory.keep();
        if paths.len() == 1 {
            if connection.supports_resume {
                service
                    .send_resumable_file(id, &paths[0])
                    .await
                    .map_err(|e| e.to_string())
            } else {
                let source = secure::files::Source::open(&paths[0])
                    .await
                    .map_err(|e| e.to_string())?;
                service.send_file(id, source).map_err(|e| e.to_string())
            }
        } else {
            let source = secure::bundle::BundleSource::files(paths)
                .await
                .map_err(|e| e.to_string())?;
            if connection.supports_resume {
                service
                    .send_resumable_bundle(id, source)
                    .await
                    .map_err(|e| e.to_string())
            } else {
                service.send_bundle(id, source).map_err(|e| e.to_string())
            }
        }
    }
}
#[cfg(target_os = "android")]
pub use mobile::*;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn saf_names_cannot_escape_collide_or_import_directories() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["a.png", "A.png", "empty"] {
            std::fs::write(dir.path().join(name), b"").unwrap();
        }
        for names in [
            vec!["../outside"],
            vec!["/absolute"],
            vec!["a.png", "A.png"],
            vec!["CON"],
            vec!["a\\b"],
            vec!["."],
        ] {
            assert!(
                validate_imports(
                    dir.path(),
                    &names.into_iter().map(String::from).collect::<Vec<_>>()
                )
                .is_err()
            );
        }
        std::fs::create_dir(dir.path().join("folder")).unwrap();
        assert!(validate_imports(dir.path(), &["folder".into()]).is_err());
        assert!(validate_imports(dir.path(), &vec!["a.png".into(); 33]).is_err());
        assert_eq!(
            validate_imports(dir.path(), &["a.png".into(), "empty".into()])
                .unwrap()
                .len(),
            2
        );
    }
    #[cfg(unix)]
    #[test]
    fn saf_import_validation_rejects_symlinks_and_excess_actual_size() {
        let dir = tempfile::tempdir().unwrap();
        let outside = dir.path().join("source");
        std::fs::write(&outside, b"original").unwrap();
        std::os::unix::fs::symlink(&outside, dir.path().join("link")).unwrap();
        assert!(validate_imports(dir.path(), &["link".into()]).is_err());
        let file = std::fs::File::create(dir.path().join("large")).unwrap();
        file.set_len(4 * 1024 * 1024 * 1024 + 1).unwrap();
        assert!(validate_imports(dir.path(), &["large".into()]).is_err());
        assert_eq!(std::fs::read(outside).unwrap(), b"original");
    }
}
