//! A single local draft populated through the platform picker. IPC never accepts paths.
use crate::secure::{
    Service,
    bundle::{self, BundleSource},
    files::{Source, validate_name},
};
use serde::Serialize;
use std::path::PathBuf;
#[cfg(target_os = "android")]
use tauri::Manager;

#[derive(Default)]
pub(crate) struct Composer(tokio::sync::Mutex<Option<Draft>>);

struct Draft {
    view: DraftView,
    paths: Vec<PathBuf>,
    #[cfg(target_os = "android")]
    documents: tempfile::TempDir,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DraftView {
    id: String,
    name: String,
    count: usize,
    folder: bool,
}

#[tauri::command]
pub(crate) async fn share_pick(
    folder: bool,
    composer: tauri::State<'_, Composer>,
    service: tauri::State<'_, Service>,
    app: tauri::AppHandle,
) -> Result<Option<DraftView>, String> {
    let mut slot = composer
        .0
        .try_lock()
        .map_err(|_| "A selection is already in use")?;
    #[cfg(target_os = "android")]
    let (paths, documents) = {
        if folder {
            return Err("Folder selection is unavailable on this device".into());
        }
        let picker = app.state::<crate::android_files::Picker>();
        let protected = slot.as_ref().map(|draft| draft.documents.path().to_owned());
        let Some(selection) = crate::android_files::choose(&picker, &service, protected).await?
        else {
            return Ok(None);
        };
        selection
    };
    #[cfg(not(target_os = "android"))]
    let paths = {
        let _ = (service, app);
        let dialog = rfd::AsyncFileDialog::new();
        if folder {
            dialog
                .set_title("Choose a folder to share")
                .pick_folder()
                .await
                .map(|file| vec![file.path().to_owned()])
        } else {
            dialog
                .set_title("Choose files to share")
                .pick_files()
                .await
                .map(|files| files.iter().map(|file| file.path().to_owned()).collect())
        }
    };
    #[cfg(not(target_os = "android"))]
    let Some(paths) = paths else {
        return Ok(None);
    };
    if paths.is_empty() {
        return Ok(None);
    }
    if paths.len() > bundle::MAX_ENTRIES {
        return Err("Too many files selected".into());
    }
    let name = if paths.len() == 1 {
        let name = paths[0]
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("File name is not safe or portable")?;
        validate_name(name).map_err(|error| error.to_string())?;
        name.to_owned()
    } else {
        format!("{} files", paths.len())
    };
    let view = DraftView {
        id: uuid::Uuid::new_v4().to_string(),
        name,
        count: paths.len(),
        folder,
    };
    *slot = Some(Draft {
        view: view.clone(),
        paths,
        #[cfg(target_os = "android")]
        documents,
    });
    Ok(Some(view))
}

#[tauri::command]
pub(crate) fn share_clear(id: String, composer: tauri::State<'_, Composer>) -> Result<(), String> {
    let mut slot = composer
        .0
        .try_lock()
        .map_err(|_| "A selection is already in use")?;
    if slot.as_ref().is_some_and(|draft| draft.view.id == id) {
        *slot = None;
    }
    Ok(())
}

#[tauri::command]
pub(crate) async fn share_send(
    id: String,
    draft_id: String,
    offer_conversion: bool,
    composer: tauri::State<'_, Composer>,
    service: tauri::State<'_, Service>,
    processing: tauri::State<'_, crate::processing::Processing>,
) -> Result<(), String> {
    let mut slot = composer
        .0
        .try_lock()
        .map_err(|_| "A selection is already in use")?;
    let draft = slot
        .as_mut()
        .filter(|draft| draft.view.id == draft_id)
        .ok_or("Choose files again")?;
    service
        .can_send_file(&id)
        .map_err(|error| error.to_string())?;
    let snapshot = service.snapshot();
    let connection = snapshot
        .connections
        .iter()
        .find(|c| c.id == id)
        .ok_or("Device is no longer available")?;
    let bundle = draft.view.folder || draft.paths.len() > 1;
    if bundle && !connection.supports_bundles {
        return Err("This device can receive one file at a time. Choose one file or update InvarDrop on the other device.".into());
    }
    #[cfg(not(target_os = "android"))]
    if processing.preparing_for(&id) {
        return Err("Finish the current file preparation before sending again.".into());
    }
    #[cfg(not(target_os = "android"))]
    if offer_conversion
        && processing
            .select(&service, id.clone(), draft.paths.clone(), draft.view.folder)
            .await?
    {
        *slot = None;
        return Ok(());
    }
    #[cfg(target_os = "android")]
    {
        let _ = (processing, offer_conversion);
        // A durable ticket may reference these copies even if submitting the offer fails.
        // Catalog-aware cleanup reclaims them after they are no longer needed.
        draft.documents.disable_cleanup(true);
    }
    let result = if bundle {
        let source = if draft.view.folder {
            BundleSource::folder(&draft.paths[0]).await
        } else {
            BundleSource::files(draft.paths.clone()).await
        }
        .map_err(|error| error.to_string())?;
        if connection.supports_resume {
            service.send_resumable_bundle(&id, source).await
        } else {
            service.send_bundle(&id, source)
        }
    } else if connection.supports_resume {
        service.send_resumable_file(&id, &draft.paths[0]).await
    } else {
        let source = Source::open(&draft.paths[0])
            .await
            .map_err(|error| error.to_string())?;
        service.send_file(&id, source)
    };
    result.map_err(|error| error.to_string())?;
    *slot = None;
    Ok(())
}
