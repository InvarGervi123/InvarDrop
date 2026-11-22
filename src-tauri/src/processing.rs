//! Explicit local processing stages; transport and verified receipts stay immutable.
use crate::secure::files::CommitGate;
use crate::secure::{
    Service,
    bundle::{self, BundleSource, Kind, Manifest},
    files::validate_name,
};
use serde::Serialize;
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
pub(crate) const EVENT: &str = "processing-updated";
const MAX_OUTPUT: u64 = 2 * 1024 * 1024 * 1024;
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct View {
    pub id: String,
    pub name: String,
    pub state: String,
    pub done: u64,
    pub total: u64,
    pub error: Option<String>,
    pub path: Option<String>,
}
struct Selection {
    connection: String,
    tree: Manifest,
    paths: Vec<PathBuf>,
    folder: bool,
}
struct Job {
    #[cfg(not(target_os = "android"))]
    connection: Option<String>,
    view: View,
    selection: Option<Selection>,
    cancel: Arc<CommitGate>,
}
#[derive(Default)]
pub struct Processing {
    jobs: Arc<Mutex<Vec<Job>>>,
    ffmpeg: Mutex<Option<PathBuf>>,
    preparation: tokio::sync::Mutex<()>,
    notify: Option<Arc<dyn Fn() + Send + Sync>>,
}
fn err() -> String {
    "Processing failed; original files were preserved".into()
}
fn io<T>(r: std::io::Result<T>) -> Result<T, String> {
    r.map_err(|_| err())
}
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}
fn ogg(path: &Path) -> bool {
    path.extension().and_then(|s| s.to_str()).is_some_and(|s| {
        ["ogg", "oga", "opus"]
            .iter()
            .any(|x| s.eq_ignore_ascii_case(x))
    })
}
fn check_ogg(path: &Path) -> Result<(), String> {
    let mut f = io(fs::File::open(path))?;
    let mut header = [0; 27];
    io(f.read_exact(&mut header))?;
    if &header[..4] != b"OggS" || header[4] != 0 || header[5] & 2 == 0 {
        return Err("Unsupported OGG audio".into());
    }
    let mut segments = vec![0; header[26] as usize];
    io(f.read_exact(&mut segments))?;
    let mut packet = [0; 8];
    io(f.read_exact(&mut packet))?;
    if &packet != b"OpusHead" && &packet[..7] != b"\x01vorbis" {
        return Err("Only OGG Vorbis and Opus audio are supported".into());
    }
    Ok(())
}
impl Processing {
    pub fn with_updates(notify: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            notify: Some(Arc::new(notify)),
            ..Self::default()
        }
    }

    fn changed(&self) {
        if let Some(notify) = &self.notify {
            notify();
        }
    }

    pub fn shutdown(&self) {
        for job in lock(&self.jobs).iter() {
            let _ = job.cancel.cancel();
        }
        let until = Instant::now() + Duration::from_secs(3);
        while Instant::now() < until
            && lock(&self.jobs)
                .iter()
                .any(|j| ["converting", "extracting"].contains(&j.view.state.as_str()))
        {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    pub fn views(&self) -> Vec<View> {
        lock(&self.jobs).iter().map(|j| j.view.clone()).collect()
    }
    pub fn converter_configured(&self) -> bool {
        // Report selection only; do not expose a local path or claim codec support.
        lock(&self.ffmpeg).is_some()
    }
    #[cfg(any(not(target_os = "android"), test))]
    pub fn configure(&self, path: PathBuf) -> Result<(), String> {
        if !path.is_absolute()
            || !bundle::regular_metadata(&path)
                .map_err(|_| err())?
                .is_file()
        {
            return Err(err());
        }
        #[cfg(windows)]
        if !path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("exe"))
        {
            return Err("Choose ffmpeg.exe, not a shell script".into());
        }
        *lock(&self.ffmpeg) = Some(path);
        Ok(())
    }
    pub fn dismiss(&self, id: &str) -> Result<(), String> {
        let mut jobs = lock(&self.jobs);
        let job = jobs.iter().find(|j| j.view.id == id).ok_or_else(err)?;
        if ["choice", "converting", "extracting", "sending"].contains(&job.view.state.as_str()) {
            return Err("Stop processing first".into());
        }
        jobs.retain(|j| j.view.id != id);
        drop(jobs);
        self.changed();
        Ok(())
    }
    pub fn cancel(&self, id: &str) -> Result<(), String> {
        let mut jobs = lock(&self.jobs);
        let job = jobs.iter_mut().find(|j| j.view.id == id).ok_or_else(err)?;
        job.cancel.cancel().map_err(|e| e.to_string())?;
        if job.view.state == "choice" {
            job.selection = None;
            job.view.state = "cancelled".into();
        }
        drop(jobs);
        self.changed();
        Ok(())
    }
    #[cfg(not(target_os = "android"))]
    pub fn preparing_for(&self, connection_id: &str) -> bool {
        lock(&self.jobs).iter().any(|job| {
            job.connection.as_deref() == Some(connection_id)
                && matches!(job.view.state.as_str(), "choice" | "converting" | "sending")
        })
    }

    #[cfg_attr(target_os = "android", allow(dead_code))]
    pub async fn select(
        &self,
        _service: &Service,
        connection: String,
        paths: Vec<PathBuf>,
        folder: bool,
    ) -> Result<bool, String> {
        let source = if folder {
            BundleSource::folder(&paths[0]).await
        } else {
            BundleSource::files(paths.clone()).await
        }
        .map_err(|e| e.to_string())?;
        let mut tree = source.manifest.clone();
        if !folder && paths.len() == 1 {
            tree.name = paths[0]
                .file_name()
                .ok_or_else(err)?
                .to_string_lossy()
                .into_owned();
        }
        let all = source.paths();
        let count = tree
            .entries
            .iter()
            .zip(&all)
            .filter(|(e, p)| e.kind == Kind::File && ogg(p))
            .count();
        let selected = Selection {
            connection,
            tree,
            paths: all,
            folder: folder || paths.len() > 1,
        };
        if count == 0 {
            return Ok(false);
        }
        let mut jobs = lock(&self.jobs);
        if jobs.len() >= 8 {
            return Err("Processing queue is full; dismiss finished entries".into());
        }
        jobs.push(Job {
            #[cfg(not(target_os = "android"))]
            connection: Some(selected.connection.clone()),
            view: View {
                id: uuid::Uuid::new_v4().to_string(),
                name: selected.tree.name.clone(),
                state: "choice".into(),
                done: 0,
                total: count as u64,
                error: None,
                path: None,
            },
            selection: Some(selected),
            cancel: Arc::new(CommitGate::default()),
        });
        drop(jobs);
        self.changed();
        Ok(true)
    }

    pub async fn extract(
        &self,
        service: &Service,
        connection: &str,
        transfer: &str,
    ) -> Result<(), String> {
        let snapshot = service.snapshot();
        let path = snapshot
            .connections
            .iter()
            .find(|c| c.id == connection)
            .and_then(|c| c.transfer.as_ref())
            .filter(|t| t.id == transfer && t.direction == "incoming" && t.status == "complete")
            .and_then(|t| t.path.clone())
            .ok_or_else(err)?;
        let path = PathBuf::from(path);
        if !path
            .extension()
            .is_some_and(|x| x.eq_ignore_ascii_case("zip"))
        {
            return Err("Choose a completed ZIP transfer".into());
        }
        let id = uuid::Uuid::new_v4().to_string();
        let cancel = Arc::new(CommitGate::default());
        {
            let mut jobs = lock(&self.jobs);
            if jobs.len() >= 8 {
                return Err("Processing queue full".into());
            }
            jobs.push(Job {
                #[cfg(not(target_os = "android"))]
                connection: None,
                view: View {
                    id: id.clone(),
                    name: path
                        .file_name()
                        .ok_or_else(err)?
                        .to_string_lossy()
                        .into_owned(),
                    state: "extracting".into(),
                    done: 0,
                    total: 0,
                    error: None,
                    path: None,
                },
                selection: None,
                cancel: cancel.clone(),
            });
        }
        self.changed();
        let jobs = self.jobs.clone();
        let notify = self.notify.clone();
        let mut last = Instant::now();
        let current = id.clone();
        let worker_cancel = cancel.clone();
        let result = tokio::task::spawn_blocking(move || {
            extract_zip(&path, &worker_cancel, |done, total| {
                if let Some(j) = lock(&jobs).iter_mut().find(|j| j.view.id == current) {
                    j.view.done = done;
                    j.view.total = total;
                }
                if last.elapsed() >= Duration::from_millis(250) {
                    last = Instant::now();
                    if let Some(notify) = &notify {
                        notify();
                    }
                }
            })
        })
        .await
        .map_err(|_| err())?;
        if let Some(j) = lock(&self.jobs).iter_mut().find(|j| j.view.id == id) {
            match &result {
                Ok(p) => {
                    j.view.state = "complete".into();
                    j.view.path = Some(p.to_string_lossy().into_owned());
                }
                Err(e) => {
                    j.view.state = if cancel.is_cancelled() {
                        "cancelled"
                    } else {
                        "failed"
                    }
                    .into();
                    j.view.error = Some(e.clone());
                }
            }
        }
        self.changed();
        result.map(|_| ())
    }
    pub async fn decide(&self, service: &Service, id: String, convert: bool) -> Result<(), String> {
        let binary = if convert {
            Some(
                lock(&self.ffmpeg)
                    .clone()
                    .ok_or("Choose a local FFmpeg executable first (not bundled)")?,
            )
        } else {
            None
        };
        let (selection, cancel) = {
            let mut jobs = lock(&self.jobs);
            let job = jobs
                .iter_mut()
                .find(|j| j.view.id == id && j.view.state == "choice")
                .ok_or_else(err)?;
            job.view.state = if convert { "converting" } else { "sending" }.into();
            (job.selection.take().ok_or_else(err)?, job.cancel.clone())
        };
        self.changed();
        let result = async {
            let Some(binary) = binary else {
                cancel.begin().map_err(|e| e.to_string())?;
                return send(service, selection, None).await;
            };
            let reservation = self.preparation.lock().await;
            if !lock(&self.jobs)
                .iter()
                .any(|j| j.view.state == "converting" && j.view.id != id)
            {
                service.cleanup_prepared().await?;
            }
            let profile = PathBuf::from(service.snapshot().receive_directory);
            let root = profile.parent().ok_or_else(err)?.join("prepared");
            io(fs::create_dir_all(&root))?;
            if io(fs::read_dir(&root))?.take(8).count() >= 8 {
                return Err(
                    "Prepared storage is full; complete or discard old resumable transfers".into(),
                );
            }
            let directory = io(tempfile::Builder::new().prefix("audio-").tempdir_in(&root))?;
            drop(reservation);
            let handoff = cancel.clone();
            let paths = selection.paths.clone();
            let tree = selection.tree.clone();
            let folder = selection.folder;
            let output = directory.path().join(&tree.name);
            let output_name = tree.name.clone();
            let shared_id = id.clone();
            let jobs = self.jobs.clone();
            let notify = self.notify.clone();
            let mut last = Instant::now();
            tokio::task::spawn_blocking(move || {
                convert_tree(
                    &binary,
                    &tree,
                    &paths,
                    &output,
                    folder,
                    &cancel,
                    move || {
                        if let Some(job) = lock(&jobs).iter_mut().find(|j| j.view.id == shared_id) {
                            job.view.done += 1;
                        }
                        if last.elapsed() >= Duration::from_millis(250) {
                            last = Instant::now();
                            if let Some(notify) = &notify {
                                notify();
                            }
                        }
                    },
                )
            })
            .await
            .map_err(|_| err())??;
            // Cancellation wins until the complete prepared batch is handed to transport.
            handoff.begin().map_err(|e| e.to_string())?;
            if let Some(job) = lock(&self.jobs).iter_mut().find(|j| j.view.id == id) {
                job.view.state = "sending".into();
            }
            self.changed();
            let stable = directory.keep().join(output_name);
            // Retain even on transport error: durable source tickets can reference this copy.
            send(service, selection, Some(stable)).await
        }
        .await;
        if let Some(job) = lock(&self.jobs).iter_mut().find(|j| j.view.id == id) {
            job.view.state = if result.is_ok() {
                "prepared"
            } else if job.cancel.is_cancelled() {
                "cancelled"
            } else {
                "failed"
            }
            .into();
            job.view.error = result.clone().err();
            if result.is_ok() {
                job.view.done = job.view.total;
            }
        }
        self.changed();
        result
    }
}
async fn send(
    service: &Service,
    selection: Selection,
    prepared: Option<PathBuf>,
) -> Result<(), String> {
    if selection.folder {
        let source = if let Some(root) = prepared {
            BundleSource::folder(&root).await
        } else {
            BundleSource::from_manifest_paths(selection.tree, selection.paths)
        }
        .map_err(|e| e.to_string())?;
        if service
            .snapshot()
            .connections
            .iter()
            .any(|c| c.id == selection.connection && c.supports_resume)
        {
            service
                .send_resumable_bundle(&selection.connection, source)
                .await
                .map_err(|e| e.to_string())
        } else {
            service
                .send_bundle(&selection.connection, source)
                .map_err(|e| e.to_string())
        }
    } else {
        let path = if let Some(root) = prepared {
            io(fs::read_dir(root))?
                .next()
                .ok_or_else(err)?
                .map_err(|_| err())?
                .path()
        } else {
            selection.paths.into_iter().next().ok_or_else(err)?
        };
        if service
            .snapshot()
            .connections
            .iter()
            .any(|c| c.id == selection.connection && c.supports_resume)
        {
            service
                .send_resumable_file(&selection.connection, &path)
                .await
                .map_err(|e| e.to_string())
        } else {
            let source = crate::secure::files::Source::open(&path)
                .await
                .map_err(|e| e.to_string())?;
            service
                .send_file(&selection.connection, source)
                .map_err(|e| e.to_string())
        }
    }
}
fn run(binary: &Path, args: &[&std::ffi::OsStr], cancel: &CommitGate) -> Result<(), String> {
    let mut child = io(Command::new(binary)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn())?;
    let began = Instant::now();
    loop {
        if cancel.is_cancelled() || began.elapsed() > Duration::from_secs(2 * 60 * 60) {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Processing cancelled or timed out".into());
        }
        if let Some(status) = io(child.try_wait())? {
            return if status.success() {
                Ok(())
            } else {
                Err("FFmpeg conversion failed; check the input and libmp3lame support".into())
            };
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
pub(crate) fn convert_tree(
    binary: &Path,
    tree: &Manifest,
    paths: &[PathBuf],
    out: &Path,
    _folder: bool,
    cancel: &CommitGate,
    mut progress: impl FnMut(),
) -> Result<(), String> {
    io(fs::create_dir(out))?;
    let mut names = std::collections::HashSet::new();
    let mut total = 0;
    for (entry, path) in tree.entries.iter().zip(paths) {
        if cancel.is_cancelled() {
            return Err("Processing cancelled".into());
        }
        for ancestor in path.ancestors() {
            bundle::regular_metadata(ancestor).map_err(|_| err())?;
        }
        let relative = if entry.kind == Kind::File && ogg(path) {
            let mut p = PathBuf::from(&entry.path);
            p.set_extension("mp3");
            p
        } else {
            PathBuf::from(&entry.path)
        };
        if !names.insert(relative.to_string_lossy().to_lowercase()) {
            return Err("Converted filename collision".into());
        }
        let destination = out.join(relative);
        if entry.kind == Kind::Directory {
            io(fs::create_dir(&destination))?;
            continue;
        }
        if ogg(path) {
            check_ogg(path)?;
            let before = hash_file(path, cancel)?;
            let args = [
                "-max_alloc".as_ref(),
                "67108864".as_ref(),
                "-threads".as_ref(),
                "1".as_ref(),
                "-nostdin".as_ref(),
                "-v".as_ref(),
                "error".as_ref(),
                "-xerror".as_ref(),
                "-protocol_whitelist".as_ref(),
                "file".as_ref(),
                "-f".as_ref(),
                "ogg".as_ref(),
                "-i".as_ref(),
                path.as_os_str(),
                "-map".as_ref(),
                "0:a:0".as_ref(),
                "-vn".as_ref(),
                "-map_metadata".as_ref(),
                "-1".as_ref(),
                "-c:a".as_ref(),
                "libmp3lame".as_ref(),
                "-b:a".as_ref(),
                "192k".as_ref(),
                "-fs".as_ref(),
                "1073741824".as_ref(),
                "-n".as_ref(),
                destination.as_os_str(),
            ];
            run(binary, &args, cancel)?;
            if hash_file(path, cancel)? != before {
                return Err("Source changed during conversion".into());
            }
            if io(fs::metadata(&destination))?.len() >= 1073741824 {
                return Err("Converted audio exceeds limit".into());
            }
            run(
                binary,
                &[
                    "-nostdin".as_ref(),
                    "-v".as_ref(),
                    "error".as_ref(),
                    "-xerror".as_ref(),
                    "-protocol_whitelist".as_ref(),
                    "file".as_ref(),
                    "-i".as_ref(),
                    destination.as_os_str(),
                    "-f".as_ref(),
                    "null".as_ref(),
                    "-".as_ref(),
                ],
                cancel,
            )?;
            progress();
        } else {
            if entry.size > MAX_OUTPUT - total {
                return Err("Prepared output exceeds limit".into());
            }
            let mut input = io(fs::File::open(path))?;
            let mut output = io(fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&destination))?;
            let mut buf = [0; 65536];
            let mut count = 0;
            loop {
                if cancel.is_cancelled() {
                    return Err("Processing cancelled".into());
                }
                let n = io(input.read(&mut buf))?;
                if n == 0 {
                    break;
                }
                count += n as u64;
                if count > entry.size {
                    return Err(err());
                }
                io(std::io::Write::write_all(&mut output, &buf[..n]))?;
            }
            if count != entry.size {
                return Err(err());
            }
            io(output.sync_all())?;
        }
        io(fs::OpenOptions::new().write(true).open(&destination))?
            .sync_all()
            .map_err(|_| err())?;
        total += io(fs::metadata(destination))?.len();
        if total > MAX_OUTPUT {
            return Err("Prepared output exceeds 2 GiB".into());
        }
    }
    sync_directories(out, cancel)?;
    Ok(())
}

fn hash_file(path: &Path, cancel: &CommitGate) -> Result<blake3::Hash, String> {
    let mut f = io(fs::File::open(path))?;
    let mut hash = blake3::Hasher::new();
    let mut buf = [0; 65536];
    loop {
        if cancel.is_cancelled() {
            return Err("Processing cancelled".into());
        }
        let n = io(f.read(&mut buf))?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    Ok(hash.finalize())
}

pub fn extract_zip(
    path: &Path,
    cancel: &CommitGate,
    mut progress: impl FnMut(u64, u64),
) -> Result<PathBuf, String> {
    use std::io::{Seek, SeekFrom};
    let metadata = bundle::regular_metadata(path).map_err(|_| err())?;
    if !metadata.is_file() || metadata.len() > 64 * 1024 * 1024 {
        return Err("ZIP must be a regular file no larger than 64 MiB".into());
    }
    let mut file = io(fs::File::open(path))?;
    let tail_len = metadata.len().min(65557) as usize;
    io(file.seek(SeekFrom::End(-(tail_len as i64))))?;
    let mut tail = vec![0; tail_len];
    io(file.read_exact(&mut tail))?;
    let start = tail
        .windows(4)
        .rposition(|b| b == b"PK\x05\x06")
        .ok_or("Invalid ZIP directory")?;
    if start + 22 > tail.len() {
        return Err(err());
    }
    let end = &tail[start..];
    let u16at = |i| u16::from_le_bytes([end[i], end[i + 1]]) as usize;
    let u32at = |i| u32::from_le_bytes(end[i..i + 4].try_into().unwrap()) as u64;
    if u16at(4) != 0
        || u16at(6) != 0
        || u16at(8) != u16at(10)
        || u16at(10) > 4096
        || u32at(12) > 1024 * 1024
        || u32at(16) + u32at(12) > metadata.len()
        || start + 22 + u16at(20) != tail.len()
    {
        return Err("ZIP64, multi-disk or excessive ZIP metadata is unsupported".into());
    }
    io(file.seek(SeekFrom::Start(0)))?;
    if tail[..start].windows(4).any(|b| b == b"PK\x06\x07") {
        return Err("ZIP64 is unsupported".into());
    }
    let mut zip = zip::ZipArchive::new(file).map_err(|_| err())?;
    if zip.len() != u16at(10) {
        return Err(err());
    }
    let mut names = std::collections::HashSet::new();
    let mut entries = std::collections::BTreeMap::<String, Kind>::new();
    let mut total = 0u64;
    for index in 0..zip.len() {
        if cancel.is_cancelled() {
            return Err("Extraction cancelled".into());
        }
        let item = zip.by_index(index).map_err(|_| err())?;
        let raw = std::str::from_utf8(item.name_raw()).map_err(|_| err())?;
        let name = if item.is_dir() {
            raw.strip_suffix('/').unwrap_or(raw)
        } else {
            raw
        };
        if name.len() > 1024 || name.split('/').count() > 16 {
            return Err("ZIP path limit exceeded".into());
        }
        for part in name.split('/') {
            validate_name(part).map_err(|_| err())?;
        }
        if !names.insert(name.to_lowercase()) {
            return Err("ZIP filename collision".into());
        }
        let mode = item.unix_mode().unwrap_or(0) & 0o170000;
        if item.is_dir() && item.size() != 0 || mode != 0 && (mode == 0o040000) != item.is_dir() {
            return Err("Inconsistent ZIP entry type".into());
        }
        if ![0, 0o100000, 0o040000].contains(&mode) {
            return Err("ZIP links and special files are forbidden".into());
        }
        if item.size() > 512 * 1024 * 1024
            || item.size() > item.compressed_size().saturating_mul(100).max(1024 * 1024)
        {
            return Err("ZIP expansion limit exceeded".into());
        }
        total = total.checked_add(item.size()).ok_or_else(err)?;
        if total > 1024 * 1024 * 1024 {
            return Err("ZIP total size exceeds 1 GiB".into());
        }
        let components: Vec<_> = name.split('/').collect();
        for depth in 1..components.len() {
            let p = components[..depth].join("/");
            if entries.get(&p) == Some(&Kind::File) {
                return Err(err());
            }
            entries.insert(p, Kind::Directory);
        }
        let kind = if item.is_dir() {
            Kind::Directory
        } else {
            Kind::File
        };
        if entries.get(name).is_some_and(|k| *k != kind) {
            return Err(err());
        }
        entries.insert(name.to_owned(), kind);
        if entries.len() > 4096 {
            return Err("ZIP entry limit exceeded".into());
        }
    }
    if entries
        .keys()
        .map(|n| n.to_lowercase())
        .collect::<std::collections::HashSet<_>>()
        .len()
        != entries.len()
    {
        return Err("ZIP directory collision".into());
    }
    let parent = path.parent().ok_or_else(err)?;
    let staging = io(tempfile::Builder::new()
        .prefix(".extract-")
        .tempdir_in(parent))?;
    let payload = staging.path().join("payload");
    io(fs::create_dir(&payload))?;
    for (name, kind) in &entries {
        if cancel.is_cancelled() {
            return Err("Extraction cancelled".into());
        }
        if *kind == Kind::Directory {
            io(fs::create_dir(payload.join(name)))?;
        }
    }
    let mut done = 0;
    for index in 0..zip.len() {
        if cancel.is_cancelled() {
            return Err("Extraction cancelled".into());
        }
        let mut item = zip.by_index(index).map_err(|_| err())?;
        if item.is_dir() {
            continue;
        }
        let name = std::str::from_utf8(item.name_raw())
            .map_err(|_| err())?
            .to_owned();
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut output = io(options.open(payload.join(name)))?;
        let mut count = 0;
        let mut buf = [0; 65536];
        loop {
            if cancel.is_cancelled() {
                return Err("Extraction cancelled".into());
            }
            let n = io(item.read(&mut buf))?;
            if n == 0 {
                break;
            }
            count += n as u64;
            if count > item.size() || done + n as u64 > total {
                return Err(err());
            }
            io(std::io::Write::write_all(&mut output, &buf[..n]))?;
            done += n as u64;
            progress(done, total);
        }
        if count != item.size() {
            return Err(err());
        }
        io(output.sync_all())?;
    }
    if cancel.is_cancelled() {
        return Err("Extraction cancelled".into());
    }
    sync_directories(&payload, cancel)?;
    let receipt = io(tempfile::Builder::new()
        .prefix("extracted-")
        .tempdir_in(parent))?;
    let destination = receipt.path().join("files");
    cancel.begin().map_err(|e| e.to_string())?;
    if let Err(error) = bundle::rename_noreplace(&payload, &destination) {
        if fs::symlink_metadata(&destination).is_ok() {
            let _ = receipt.keep();
        }
        return Err(error.to_string());
    }
    let _ = receipt.keep();
    Ok(destination)
}

fn sync_directories(root: &Path, cancel: &CommitGate) -> Result<(), String> {
    if cancel.is_cancelled() {
        return Err("Processing cancelled".into());
    }
    #[cfg(unix)]
    {
        for entry in io(fs::read_dir(root))? {
            let entry = io(entry)?;
            if io(entry.file_type())?.is_dir() {
                sync_directories(&entry.path(), cancel)?;
            }
        }
        io(fs::File::open(root))?.sync_all().map_err(|_| err())?;
        if let Some(parent) = root.parent() {
            io(fs::File::open(parent))?.sync_all().map_err(|_| err())?;
        }
    }
    #[cfg(not(unix))]
    let _ = root;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use zip::write::SimpleFileOptions;
    #[test]
    fn converter_selection_is_explicit_and_does_not_persist_across_sessions() {
        let processing = Processing::default();
        assert!(!processing.converter_configured());
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("ffmpeg.exe");
        fs::write(&path, b"test selection, not an executable").unwrap();
        processing.configure(path).unwrap();
        assert!(processing.converter_configured());
        assert!(
            processing
                .configure(directory.path().join("missing"))
                .is_err()
        );
        assert!(processing.converter_configured());
        assert!(!Processing::default().converter_configured());
    }
    fn archive(path: &Path, entries: &[(&str, &[u8])]) {
        let mut z = zip::ZipWriter::new(fs::File::create(path).unwrap());
        for (name, data) in entries {
            if name.ends_with('/') {
                z.add_directory(*name, SimpleFileOptions::default())
                    .unwrap();
            } else {
                z.start_file(*name, SimpleFileOptions::default()).unwrap();
                std::io::Write::write_all(&mut z, data).unwrap();
            }
        }
        z.finish().unwrap();
    }
    #[test]
    fn zip_nested_zero_and_atomic_receipts_preserve_original() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("input.zip");
        archive(
            &p,
            &[
                ("nested/empty/", b""),
                ("nested/file", b"hello"),
                ("zero", b""),
            ],
        );
        let before = fs::read(&p).unwrap();
        let out = extract_zip(&p, &CommitGate::default(), |_, _| {}).unwrap();
        assert_eq!(fs::read(out.join("nested/file")).unwrap(), b"hello");
        assert!(out.join("nested/empty").is_dir());
        assert_eq!(fs::metadata(out.join("zero")).unwrap().len(), 0);
        assert_eq!(fs::read(p).unwrap(), before);
    }
    #[test]
    fn zip_unsafe_names_duplicates_and_links_are_rejected() {
        for name in ["../escape", "/absolute", "C:/file", "a\\b", "CON", "a/../b"] {
            let d = tempfile::tempdir().unwrap();
            let p = d.path().join("input.zip");
            archive(&p, &[(name, b"x")]);
            assert!(extract_zip(&p, &CommitGate::default(), |_, _| {}).is_err());
            assert_eq!(fs::read_dir(d.path()).unwrap().count(), 1);
        }
    }
    #[test]
    fn zip_bomb_and_cancel_do_not_publish() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("input.zip");
        archive(&p, &[("bomb", &vec![0; 2 * 1024 * 1024])]);
        assert!(extract_zip(&p, &CommitGate::default(), |_, _| {}).is_err());
        archive(&p, &[("file", b"abc")]);
        let gate = CommitGate::default();
        gate.cancel().unwrap();
        assert!(extract_zip(&p, &gate, |_, _| {}).is_err());
        assert_eq!(fs::read_dir(d.path()).unwrap().count(), 1);
    }
    #[test]
    fn invalid_ogg_and_cancel_are_safe() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("bad.ogg");
        fs::write(&p, b"not audio").unwrap();
        assert!(check_ogg(&p).is_err());
        let gate = CommitGate::default();
        gate.cancel().unwrap();
        let m = Manifest {
            name: "batch".into(),
            entries: vec![bundle::Entry {
                path: "bad.ogg".into(),
                kind: Kind::File,
                size: 9,
            }],
            total: 9,
        };
        assert!(
            convert_tree(
                Path::new("missing"),
                &m,
                std::slice::from_ref(&p),
                &d.path().join("output"),
                false,
                &gate,
                || {}
            )
            .is_err()
        );
        assert_eq!(fs::read(p).unwrap(), b"not audio");
    }
    #[test]
    fn zip_links_case_collisions_and_corruption_never_publish() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("bad.zip");
        {
            let mut zip = zip::ZipWriter::new(fs::File::create(&p).unwrap());
            zip.add_symlink("link", "../target", SimpleFileOptions::default())
                .unwrap();
            zip.finish().unwrap();
        }
        assert!(extract_zip(&p, &CommitGate::default(), |_, _| {}).is_err());
        archive(&p, &[("A/file", b"x"), ("a/other", b"y")]);
        assert!(extract_zip(&p, &CommitGate::default(), |_, _| {}).is_err());
        archive(&p, &[("file", b"hello")]);
        let mut data = fs::read(&p).unwrap();
        let cd = data.windows(4).position(|b| b == b"PK\x01\x02").unwrap();
        data[cd + 16] ^= 0xff;
        fs::write(&p, &data).unwrap();
        assert!(extract_zip(&p, &CommitGate::default(), |_, _| {}).is_err());
        assert_eq!(fs::read(&p).unwrap(), data);
        assert_eq!(fs::read_dir(d.path()).unwrap().count(), 1);
    }
    #[test]
    fn zip_counts_and_depth_are_bounded_before_extraction() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("large.zip");
        let deep = format!("{}file", "dir/".repeat(17));
        archive(&p, &[(&deep, b"x")]);
        assert!(extract_zip(&p, &CommitGate::default(), |_, _| {}).is_err());
        archive(&p, &[("file", b"")]);
        let mut bytes = fs::read(&p).unwrap();
        let end = bytes.windows(4).rposition(|b| b == b"PK\x05\x06").unwrap();
        bytes[end + 8..end + 10].copy_from_slice(&4097u16.to_le_bytes());
        bytes[end + 10..end + 12].copy_from_slice(&4097u16.to_le_bytes());
        fs::write(&p, bytes).unwrap();
        assert!(extract_zip(&p, &CommitGate::default(), |_, _| {}).is_err());
        assert_eq!(fs::read_dir(d.path()).unwrap().count(), 1);
    }
    #[test]
    fn running_converter_cancellation_is_prompt() {
        let binary = PathBuf::from(
            std::env::var_os("INVARDROP_TEST_FFMPEG").unwrap_or_else(|| "ffmpeg".into()),
        );
        let gate = Arc::new(CommitGate::default());
        let cancel = gate.clone();
        let thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            cancel.cancel().unwrap();
        });
        let now = Instant::now();
        assert!(
            run(
                &binary,
                &[
                    "-nostdin".as_ref(),
                    "-v".as_ref(),
                    "error".as_ref(),
                    "-re".as_ref(),
                    "-f".as_ref(),
                    "lavfi".as_ref(),
                    "-i".as_ref(),
                    "sine=frequency=440:duration=60".as_ref(),
                    "-f".as_ref(),
                    "null".as_ref(),
                    "-".as_ref()
                ],
                &gate
            )
            .is_err()
        );
        thread.join().unwrap();
        assert!(now.elapsed() < Duration::from_secs(5));
    }
    #[test]
    fn extraction_cancel_during_copy_and_finalization_leaves_zip() {
        for during_commit in [false, true] {
            let d = tempfile::tempdir().unwrap();
            let p = d.path().join("input.zip");
            archive(&p, &[("file", b"hello")]);
            let original = fs::read(&p).unwrap();
            let gate = Arc::new(CommitGate::default());
            let cancel = gate.clone();
            if during_commit {
                gate.pause_before(move || {
                    cancel.cancel().unwrap();
                });
            }
            let result = extract_zip(&p, &gate, |_, _| {
                if !during_commit {
                    gate.cancel().unwrap();
                }
            });
            assert!(result.is_err());
            assert_eq!(fs::read(p).unwrap(), original);
            assert_eq!(fs::read_dir(d.path()).unwrap().count(), 1);
        }
    }
    #[test]
    fn extraction_finalization_collision_never_replaces_existing_data() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("input.zip");
        archive(&p, &[("file", b"hello")]);
        let root = d.path().to_owned();
        let gate = CommitGate::default();
        gate.pause_after(move || {
            let receipt = fs::read_dir(root)
                .unwrap()
                .map(|e| e.unwrap().path())
                .find(|p| {
                    p.file_name()
                        .unwrap()
                        .to_string_lossy()
                        .starts_with("extracted-")
                })
                .unwrap();
            fs::write(receipt.join("files"), b"existing").unwrap();
        });
        assert!(extract_zip(&p, &gate, |_, _| {}).is_err());
        let receipt = fs::read_dir(d.path())
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("extracted-")
            })
            .unwrap();
        assert_eq!(fs::read(receipt.join("files")).unwrap(), b"existing");
    }
    #[test]
    fn converter_failure_drops_private_batch_and_keeps_original() {
        let binary = PathBuf::from(
            std::env::var_os("INVARDROP_TEST_FFMPEG").unwrap_or_else(|| "ffmpeg".into()),
        );
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("bad.ogg");
        let mut header = vec![0; 27];
        header[..4].copy_from_slice(b"OggS");
        header[5] = 2;
        header[26] = 1;
        header.push(8);
        header.extend_from_slice(b"OpusHead");
        fs::write(&path, &header).unwrap();
        check_ogg(&path).unwrap();
        let tree = Manifest {
            name: "bad".into(),
            total: header.len() as u64,
            entries: vec![bundle::Entry {
                path: "bad.ogg".into(),
                kind: Kind::File,
                size: header.len() as u64,
            }],
        };
        {
            let batch = tempfile::tempdir_in(d.path()).unwrap();
            assert!(
                convert_tree(
                    &binary,
                    &tree,
                    std::slice::from_ref(&path),
                    &batch.path().join("payload"),
                    false,
                    &CommitGate::default(),
                    || {}
                )
                .is_err()
            );
        }
        assert_eq!(fs::read(path).unwrap(), header);
        assert_eq!(fs::read_dir(d.path()).unwrap().count(), 1);
    }
    #[test]
    fn actual_vorbis_and_opus_to_mp3_decode_and_preserve_source() {
        for codec in ["libvorbis", "libopus"] {
            let binary =
                std::env::var_os("INVARDROP_TEST_FFMPEG").unwrap_or_else(|| "ffmpeg".into());
            let binary = PathBuf::from(binary);
            let d = tempfile::tempdir().unwrap();
            let source = d.path().join("tone.ogg");
            assert!(
                Command::new(&binary)
                    .args([
                        "-v",
                        "error",
                        "-f",
                        "lavfi",
                        "-i",
                        "sine=frequency=440:duration=2",
                        "-c:a",
                        codec
                    ])
                    .arg(&source)
                    .status()
                    .unwrap()
                    .success(),
                "FFmpeg test dependency with libvorbis/libmp3lame is required"
            );
            check_ogg(&source).unwrap();
            let before = fs::read(&source).unwrap();
            let size = before.len() as u64;
            let tree = Manifest {
                name: "tone.ogg".into(),
                total: size,
                entries: vec![bundle::Entry {
                    path: "tone.ogg".into(),
                    kind: Kind::File,
                    size,
                }],
            };
            let out = d.path().join("prepared");
            convert_tree(
                &binary,
                &tree,
                std::slice::from_ref(&source),
                &out,
                false,
                &CommitGate::default(),
                || {},
            )
            .unwrap();
            assert_eq!(fs::read(&source).unwrap(), before);
            let mp3 = out.join("tone.mp3");
            assert!(mp3.is_file());
            let pcm = d.path().join("pcm");
            assert!(
                Command::new(binary)
                    .args(["-v", "error", "-i"])
                    .arg(mp3)
                    .args(["-f", "f32le", "-ac", "1", "-ar", "44100"])
                    .arg(&pcm)
                    .status()
                    .unwrap()
                    .success()
            );
            let bytes = fs::read(pcm).unwrap();
            let seconds = bytes.len() as f64 / 4.0 / 44100.0;
            assert!((1.9..2.1).contains(&seconds));
            let rms = (bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|b| f32::from_le_bytes(*b) as f64)
                .map(|v| v * v)
                .sum::<f64>()
                / (bytes.len() / 4) as f64)
                .sqrt();
            assert!((0.05..0.15).contains(&rms));
        }
    }
}
