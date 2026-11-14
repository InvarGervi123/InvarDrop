//! Bounded folder manifests and one-file-at-a-time IO. No remote absolute paths.
use super::{
    error::{Error, Result},
    files::{self, CHUNK_SIZE, CommitGate, MAX_FILE_SIZE, Source},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};
use tempfile::TempDir;
use tokio::io::AsyncWriteExt;

pub const MAX_ENTRIES: usize = 4096;
pub const MAX_DEPTH: usize = 16;
pub const MAX_METADATA: usize = 240 * 1024;
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    File,
    Directory,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub path: String,
    pub kind: Kind,
    pub size: u64,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub name: String,
    #[serde(deserialize_with = "bounded_entries")]
    pub entries: Vec<Entry>,
    pub total: u64,
}
fn bounded_entries<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Vec<Entry>, D::Error> {
    struct Visitor;
    impl<'de> serde::de::Visitor<'de> for Visitor {
        type Value = Vec<Entry>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a bounded entry list")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut sequence: A,
        ) -> std::result::Result<Self::Value, A::Error> {
            let mut entries = Vec::new();
            while let Some(entry) = sequence.next_element()? {
                if entries.len() == MAX_ENTRIES {
                    return Err(serde::de::Error::custom("entry limit"));
                }
                entries.push(entry);
            }
            Ok(entries)
        }
    }
    deserializer.deserialize_seq(Visitor)
}
impl Manifest {
    pub fn validate(&self) -> Result<()> {
        files::validate_name(&self.name)?;
        if self.entries.len() > MAX_ENTRIES || self.total > MAX_FILE_SIZE {
            return Err(Error::Limit);
        }
        let mut seen = BTreeMap::<String, (&str, &Kind)>::new();
        let mut previous = "";
        let mut total = 0u64;
        for entry in &self.entries {
            if entry.path.len() > 1024 || entry.path.split('/').count() > MAX_DEPTH {
                return Err(Error::Limit);
            }
            for component in entry.path.split('/') {
                files::validate_name(component)?;
            }
            if entry.path.as_str() <= previous {
                return Err(Error::InvalidName);
            }
            previous = &entry.path;
            if let Some((parent, _)) = entry.path.rsplit_once('/') {
                match seen.get(&parent.to_lowercase()) {
                    Some((exact, Kind::Directory)) if *exact == parent => {}
                    _ => return Err(Error::InvalidName),
                }
            }
            if seen
                .insert(entry.path.to_lowercase(), (&entry.path, &entry.kind))
                .is_some()
            {
                return Err(Error::InvalidName);
            }
            if entry.kind == Kind::Directory && entry.size != 0 {
                return Err(Error::Protocol);
            }
            if entry.size > MAX_FILE_SIZE {
                return Err(Error::Limit);
            }
            total = total.checked_add(entry.size).ok_or(Error::Limit)?;
            if total > MAX_FILE_SIZE {
                return Err(Error::Limit);
            }
        }
        if total != self.total {
            return Err(Error::Integrity);
        }
        if serde_json::to_vec(self).map_err(|_| Error::Protocol)?.len() > MAX_METADATA {
            return Err(Error::Limit);
        }
        Ok(())
    }
    pub fn file_count(&self) -> u32 {
        self.entries.iter().filter(|e| e.kind == Kind::File).count() as u32
    }
}
// Reject all Windows reparse points too (junctions need not be ordinary symlinks).
pub(crate) fn regular_metadata(path: &Path) -> Result<fs::Metadata> {
    let metadata = fs::symlink_metadata(path).map_err(|_| Error::FileIo)?;
    if metadata.file_type().is_symlink() {
        return Err(Error::FileIo);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(Error::FileIo);
        }
    }
    if !metadata.is_file() && !metadata.is_dir() {
        return Err(Error::FileIo);
    }
    Ok(metadata)
}
fn safe_source(path: &Path) -> Result<()> {
    // Ancestors can have changed since the initial enumeration.
    for ancestor in path.ancestors() {
        regular_metadata(ancestor)?;
    }
    Ok(())
}
pub struct BundleSource {
    pub manifest: Manifest,
    paths: Vec<PathBuf>,
    index: usize,
    current: Option<Source>,
}
pub enum Event {
    Data(Vec<u8>),
    FileEnd { index: u32, hash: String },
    Done,
}
impl BundleSource {
    pub(crate) fn paths(&self) -> Vec<PathBuf> {
        self.paths.clone()
    }
    pub async fn folder(path: &Path) -> Result<Self> {
        let root = path.to_owned();
        tokio::task::spawn_blocking(move || {
            safe_source(&root)?;
            if !regular_metadata(&root)?.is_dir() {
                return Err(Error::FileIo);
            }
            let name = root
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or(Error::InvalidName)?
                .to_owned();
            files::validate_name(&name)?;
            let mut entries = Vec::new();
            fn walk(
                root: &Path,
                relative: &str,
                entries: &mut Vec<Entry>,
                depth: usize,
            ) -> Result<()> {
                for item in fs::read_dir(root.join(relative)).map_err(|_| Error::FileIo)? {
                    if entries.len() >= MAX_ENTRIES || depth > MAX_DEPTH {
                        return Err(Error::Limit);
                    }
                    let item = item.map_err(|_| Error::FileIo)?;
                    let name = item
                        .file_name()
                        .into_string()
                        .map_err(|_| Error::InvalidName)?;
                    files::validate_name(&name)?;
                    let path = if relative.is_empty() {
                        name
                    } else {
                        format!("{relative}/{name}")
                    };
                    if path.len() > 1024 {
                        return Err(Error::Limit);
                    }
                    let metadata = regular_metadata(&item.path())?;
                    let directory = metadata.is_dir();
                    entries.push(Entry {
                        path: path.clone(),
                        kind: if directory {
                            Kind::Directory
                        } else {
                            Kind::File
                        },
                        size: if directory { 0 } else { metadata.len() },
                    });
                    if directory {
                        walk(root, &path, entries, depth + 1)?;
                    }
                }
                Ok(())
            }
            walk(&root, "", &mut entries, 1)?;
            entries.sort_by(|a, b| a.path.cmp(&b.path));
            let paths = entries.iter().map(|e| root.join(&e.path)).collect();
            Self::new(name, entries, paths)
        })
        .await
        .map_err(|_| Error::FileIo)?
    }
    pub async fn files(paths: Vec<PathBuf>) -> Result<Self> {
        tokio::task::spawn_blocking(move || {
            if paths.is_empty() || paths.len() > MAX_ENTRIES {
                return Err(Error::Limit);
            }
            let mut items = Vec::new();
            for path in paths {
                safe_source(&path)?;
                let metadata = regular_metadata(&path)?;
                if !metadata.is_file() {
                    return Err(Error::FileIo);
                }
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .ok_or(Error::InvalidName)?
                    .to_owned();
                items.push((
                    Entry {
                        path: name,
                        kind: Kind::File,
                        size: metadata.len(),
                    },
                    path,
                ));
            }
            items.sort_by(|a, b| a.0.path.cmp(&b.0.path));
            let (entries, paths) = items.into_iter().unzip();
            Self::new("InvarDrop files".into(), entries, paths)
        })
        .await
        .map_err(|_| Error::FileIo)?
    }
    fn new(name: String, entries: Vec<Entry>, paths: Vec<PathBuf>) -> Result<Self> {
        let total = entries
            .iter()
            .try_fold(0u64, |sum, e| sum.checked_add(e.size).ok_or(Error::Limit))?;
        let manifest = Manifest {
            name,
            entries,
            total,
        };
        manifest.validate()?;
        Ok(Self {
            manifest,
            paths,
            index: 0,
            current: None,
        })
    }
    pub async fn next(&mut self) -> Result<Event> {
        while self.index < self.manifest.entries.len() {
            let entry = &self.manifest.entries[self.index];
            if entry.kind == Kind::Directory {
                self.index += 1;
                continue;
            }
            if self.current.is_none() {
                safe_source(&self.paths[self.index])?;
                let source = Source::open(&self.paths[self.index]).await?;
                if source.size != entry.size {
                    return Err(Error::FileChanged);
                }
                self.current = Some(source);
            }
            let source = self.current.as_mut().ok_or(Error::FileIo)?;
            if let Some(data) = source.next_chunk().await? {
                return Ok(Event::Data(data));
            }
            let event = Event::FileEnd {
                index: self.index as u32,
                hash: source.digest(),
            };
            self.current = None;
            self.index += 1;
            return Ok(event);
        }
        Ok(Event::Done)
    }
}
struct OpenFile {
    file: tokio::fs::File,
    received: u64,
    hasher: blake3::Hasher,
}
pub struct BundleSink {
    current: Option<OpenFile>, // closed before TempDir cleanup on Windows
    staging: TempDir,
    manifest: Manifest,
    index: usize,
    received: u64,
}
impl BundleSink {
    pub async fn create(inbox: &Path, manifest: Manifest) -> Result<Self> {
        manifest.validate()?;
        if !inbox.is_absolute() {
            return Err(Error::FileIo);
        }
        let inbox = inbox.to_owned();
        let copy = manifest.clone();
        let staging = tokio::task::spawn_blocking(move || {
            fs::create_dir_all(&inbox).map_err(|_| Error::FileIo)?;
            if !regular_metadata(&inbox)?.is_dir() {
                return Err(Error::FileIo);
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&inbox, fs::Permissions::from_mode(0o700))
                    .map_err(|_| Error::FileIo)?;
            }
            let directory = private_dir(&inbox, ".incoming-")?;
            fs::create_dir(directory.path().join("payload")).map_err(|_| Error::FileIo)?;
            for entry in &copy.entries {
                if entry.kind == Kind::Directory {
                    fs::create_dir(directory.path().join("payload").join(&entry.path))
                        .map_err(|_| Error::FileIo)?;
                }
            }
            Ok(directory)
        })
        .await
        .map_err(|_| Error::FileIo)??;
        Ok(Self {
            current: None,
            staging,
            manifest,
            index: 0,
            received: 0,
        })
    }
    fn skip_directories(&mut self) {
        while self
            .manifest
            .entries
            .get(self.index)
            .is_some_and(|e| e.kind == Kind::Directory)
        {
            self.index += 1;
        }
    }
    async fn open(&mut self) -> Result<()> {
        self.skip_directories();
        let entry = self
            .manifest
            .entries
            .get(self.index)
            .ok_or(Error::Protocol)?;
        if self.current.is_none() {
            let mut options = tokio::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            options.mode(0o600);
            let file = options
                .open(self.staging.path().join("payload").join(&entry.path))
                .await
                .map_err(|_| Error::FileIo)?;
            self.current = Some(OpenFile {
                file,
                received: 0,
                hasher: blake3::Hasher::new(),
            });
        }
        Ok(())
    }
    pub async fn write(&mut self, data: &[u8]) -> Result<()> {
        if data.is_empty() || data.len() > CHUNK_SIZE {
            return Err(Error::Protocol);
        }
        self.open().await?;
        let current = self.current.as_mut().ok_or(Error::Protocol)?;
        let next = current
            .received
            .checked_add(data.len() as u64)
            .ok_or(Error::Integrity)?;
        if next > self.manifest.entries[self.index].size {
            return Err(Error::Integrity);
        }
        current
            .file
            .write_all(data)
            .await
            .map_err(|_| Error::FileIo)?;
        current.file.flush().await.map_err(|_| Error::FileIo)?;
        current.hasher.update(data);
        current.received = next;
        self.received += data.len() as u64;
        Ok(())
    }
    pub async fn file_end(&mut self, index: u32, hash: &str) -> Result<()> {
        self.open().await?;
        if self.index != index as usize {
            return Err(Error::Protocol);
        }
        let current = self.current.take().ok_or(Error::Protocol)?;
        let hash = blake3::Hash::from_hex(hash).map_err(|_| Error::Integrity)?;
        if current.received != self.manifest.entries[self.index].size
            || current.hasher.finalize() != hash
        {
            return Err(Error::Integrity);
        }
        // Durability IO belongs to whole-operation finalization. Keeping fsync
        // out of this frame handler lets large files continue exchanging pings.
        drop(current.file.into_std().await);
        self.index += 1;
        Ok(())
    }
    pub async fn finish(
        mut self,
        gate: Arc<CommitGate>,
        on_commit: impl FnOnce() + Send + 'static,
    ) -> Result<PathBuf> {
        gate.check()?;
        self.skip_directories();
        if self.current.is_some()
            || self.index != self.manifest.entries.len()
            || self.received != self.manifest.total
        {
            return Err(Error::Integrity);
        }
        tokio::task::spawn_blocking(move || {
            gate.check()?;
            for entry in self
                .manifest
                .entries
                .iter()
                .filter(|e| e.kind == Kind::File)
            {
                gate.check()?;
                fs::OpenOptions::new()
                    .write(true)
                    .open(self.staging.path().join("payload").join(&entry.path))
                    .and_then(|f| f.sync_all())
                    .map_err(|_| Error::FileIo)?;
            }
            #[cfg(unix)]
            {
                for entry in self
                    .manifest
                    .entries
                    .iter()
                    .rev()
                    .filter(|e| e.kind == Kind::Directory)
                {
                    gate.check()?;
                    fs::File::open(self.staging.path().join("payload").join(&entry.path))
                        .and_then(|f| f.sync_all())
                        .map_err(|_| Error::FileIo)?;
                }
                fs::File::open(self.staging.path().join("payload"))
                    .and_then(|f| f.sync_all())
                    .map_err(|_| Error::FileIo)?;
            }
            let inbox = self.staging.path().parent().ok_or(Error::FileIo)?;
            let receipt = private_dir(inbox, "received-")?;
            let destination = receipt.path().join(&self.manifest.name);
            gate.begin()?;
            on_commit();
            if rename_noreplace(&self.staging.path().join("payload"), &destination).is_err() {
                if fs::symlink_metadata(&destination).is_ok() {
                    let _ = receipt.keep();
                }
                return Err(Error::FileIo);
            }
            // Preserve the verified published folder even if a subsequent durability
            // sync fails; report the error rather than a false success.
            let receipt = receipt.keep();
            #[cfg(unix)]
            {
                fs::File::open(&receipt)
                    .and_then(|f| f.sync_all())
                    .map_err(|_| Error::FileIo)?;
                fs::File::open(inbox)
                    .and_then(|f| f.sync_all())
                    .map_err(|_| Error::FileIo)?;
            }
            #[cfg(not(unix))]
            let _ = receipt;
            Ok(destination)
        })
        .await
        .map_err(|_| Error::FileIo)?
    }
}
fn private_dir(inbox: &Path, prefix: &str) -> Result<TempDir> {
    let mut builder = tempfile::Builder::new();
    builder.prefix(prefix);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(fs::Permissions::from_mode(0o700));
    }
    builder.tempdir_in(inbox).map_err(|_| Error::FileIo)
}
pub(crate) fn rename_noreplace(from: &Path, to: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        use std::{ffi::CString, os::unix::ffi::OsStrExt};
        let from = CString::new(from.as_os_str().as_bytes())?;
        let to = CString::new(to.as_os_str().as_bytes())?;
        // SAFETY: NUL-terminated paths remain alive for this synchronous call.
        if unsafe {
            libc::renameat2(
                libc::AT_FDCWD,
                from.as_ptr(),
                libc::AT_FDCWD,
                to.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        } == 0
        {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let from: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
        let to: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
        // SAFETY: valid NUL-terminated UTF-16; flags=0 forbids replacement/copy.
        if unsafe {
            windows_sys::Win32::Storage::FileSystem::MoveFileExW(from.as_ptr(), to.as_ptr(), 0)
        } != 0
        {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        let _ = (from, to);
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "atomic no-replace directory rename unavailable",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn manifest(entries: Vec<Entry>) -> Manifest {
        Manifest {
            name: "Folder".into(),
            total: entries.iter().map(|e| e.size).sum(),
            entries,
        }
    }
    fn entry(path: &str, kind: Kind, size: u64) -> Entry {
        Entry {
            path: path.into(),
            kind,
            size,
        }
    }
    fn empty(path: &Path) {
        assert!(!path.exists() || fs::read_dir(path).unwrap().next().is_none());
    }
    async fn copy(mut source: BundleSource, inbox: &Path) -> PathBuf {
        let mut sink = BundleSink::create(inbox, source.manifest.clone())
            .await
            .unwrap();
        loop {
            match source.next().await.unwrap() {
                Event::Data(data) => {
                    assert!(data.len() <= CHUNK_SIZE);
                    sink.write(&data).await.unwrap();
                }
                Event::FileEnd { index, hash } => sink.file_end(index, &hash).await.unwrap(),
                Event::Done => break,
            }
        }
        sink.finish(Arc::new(CommitGate::default()), || {})
            .await
            .unwrap()
    }
    #[test]
    fn malicious_manifests_reject_traversal_collisions_and_inconsistent_metadata() {
        for path in [
            "../evil",
            "/absolute",
            "C:/x",
            "a\\b",
            "a//b",
            "./x",
            "CON.txt",
            "a.",
            "x\u{202e}txt",
        ] {
            assert!(
                manifest(vec![entry(path, Kind::File, 0)])
                    .validate()
                    .is_err(),
                "{path}"
            );
        }
        for entries in [
            vec![entry("a", Kind::File, 0), entry("a", Kind::File, 0)],
            vec![entry("A", Kind::File, 0), entry("a", Kind::File, 0)],
            vec![entry("dir/a", Kind::File, 0)],
            vec![entry("a", Kind::File, 0), entry("a/b", Kind::File, 0)],
            vec![entry("a", Kind::Directory, 1)],
            vec![entry("A", Kind::Directory, 0), entry("a/b", Kind::File, 0)],
            vec![entry("z", Kind::File, 0), entry("a", Kind::File, 0)],
        ] {
            assert!(manifest(entries).validate().is_err());
        }
        let mut m = manifest(vec![entry("a", Kind::File, 5)]);
        m.total = 4;
        assert!(m.validate().is_err());
        let m = manifest(vec![
            entry("a", Kind::File, MAX_FILE_SIZE),
            entry("b", Kind::File, 1),
        ]);
        assert!(m.validate().is_err());
        assert!(
            manifest(vec![entry(&"x".repeat(201), Kind::File, 0)])
                .validate()
                .is_err()
        );
    }
    #[test]
    fn count_depth_metadata_and_deserializer_are_bounded() {
        let entries: Vec<_> = (0..MAX_ENTRIES)
            .map(|i| entry(&format!("f{i:04}"), Kind::File, 0))
            .collect();
        let mut m = manifest(entries);
        m.validate().unwrap();
        assert_eq!(m.file_count(), MAX_ENTRIES as u32);
        m.entries.push(entry("z", Kind::File, 0));
        assert!(m.validate().is_err());
        let json = serde_json::to_vec(&m).unwrap();
        assert!(serde_json::from_slice::<Manifest>(&json).is_err());
        let mut entries = Vec::new();
        let mut path = String::new();
        for _ in 0..MAX_DEPTH {
            if !path.is_empty() {
                path.push('/');
            }
            path.push('d');
            entries.push(entry(&path, Kind::Directory, 0));
        }
        let mut m = manifest(entries);
        m.validate().unwrap();
        m.entries
            .push(entry(&format!("{path}/file"), Kind::File, 0));
        assert!(m.validate().is_err());
        let entries = (0..MAX_ENTRIES)
            .map(|i| entry(&format!("f{i:04}{}", "x".repeat(150)), Kind::File, 0))
            .collect();
        assert!(manifest(entries).validate().is_err());
    }
    #[tokio::test]
    async fn nested_empty_zero_multifile_and_duplicate_receipts_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("Folder");
        fs::create_dir_all(folder.join("nested/empty")).unwrap();
        fs::write(folder.join("nested/zero"), []).unwrap();
        fs::write(folder.join("data"), vec![7; CHUNK_SIZE * 3 + 9]).unwrap();
        let inbox = dir.path().join("inbox");
        let first = copy(BundleSource::folder(&folder).await.unwrap(), &inbox).await;
        let second = copy(BundleSource::folder(&folder).await.unwrap(), &inbox).await;
        assert_ne!(first, second);
        assert!(first.join("nested/empty").is_dir());
        assert_eq!(fs::metadata(first.join("nested/zero")).unwrap().len(), 0);
        assert_eq!(
            fs::read(first.join("data")).unwrap(),
            fs::read(folder.join("data")).unwrap()
        );
        let source = BundleSource::files(vec![folder.join("data"), folder.join("nested/zero")])
            .await
            .unwrap();
        assert_eq!(source.manifest.file_count(), 2);
        let flat = copy(source, &inbox).await;
        assert!(flat.join("zero").is_file());
        let empty_root = dir.path().join("Empty");
        fs::create_dir(&empty_root).unwrap();
        let received = copy(BundleSource::folder(&empty_root).await.unwrap(), &inbox).await;
        assert!(received.is_dir());
        empty(&received);
    }
    #[tokio::test]
    async fn many_empty_files_do_not_need_many_open_handles() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("many");
        fs::create_dir(&folder).unwrap();
        for i in 0..256 {
            fs::write(folder.join(format!("f{i:04}")), []).unwrap();
        }
        let source = BundleSource::folder(&folder).await.unwrap();
        assert_eq!(source.manifest.file_count(), 256);
        let received = copy(source, &dir.path().join("inbox")).await;
        assert_eq!(fs::read_dir(received).unwrap().count(), 256);
    }
    #[tokio::test]
    async fn corruption_wrong_order_truncation_and_drop_remove_whole_operation() {
        let dir = tempfile::tempdir().unwrap();
        let inbox = dir.path().join("inbox");
        let m = manifest(vec![entry("a", Kind::File, 3), entry("b", Kind::File, 0)]);
        let good = blake3::hash(b"abc").to_hex().to_string();
        for scenario in 0..6 {
            let mut sink = BundleSink::create(&inbox, m.clone()).await.unwrap();
            if scenario == 0 {
                assert!(sink.write(b"abcd").await.is_err());
            } else {
                sink.write(b"abc").await.unwrap();
                match scenario {
                    1 => assert!(sink.file_end(1, &good).await.is_err()),
                    2 => assert!(
                        sink.file_end(0, &blake3::hash(b"bad").to_hex())
                            .await
                            .is_err()
                    ),
                    3 => {
                        sink.file_end(0, &good).await.unwrap();
                        assert!(
                            sink.finish(Arc::new(CommitGate::default()), || {})
                                .await
                                .is_err()
                        );
                        empty(&inbox);
                        continue;
                    }
                    4 => {
                        sink.file_end(0, &good).await.unwrap();
                        let gate = Arc::new(CommitGate::default());
                        gate.cancel().unwrap();
                        assert!(sink.finish(gate, || {}).await.is_err());
                        empty(&inbox);
                        continue;
                    }
                    _ => {}
                }
            }
            drop(sink);
            empty(&inbox);
        }
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancellation_before_and_after_publication_has_one_winner() {
        let dir = tempfile::tempdir().unwrap();
        let inbox = dir.path().join("inbox");
        for before in [true, false] {
            let sink = BundleSink::create(&inbox, manifest(vec![])).await.unwrap();
            let gate = Arc::new(CommitGate::default());
            let (at, reached) = tokio::sync::oneshot::channel();
            let (release, wait) = std::sync::mpsc::channel();
            let hook = move || {
                let _ = at.send(());
                let _ = wait.recv_timeout(std::time::Duration::from_secs(10));
            };
            if before {
                gate.pause_before(hook);
            } else {
                gate.pause_after(hook);
            }
            let other = gate.clone();
            let task = tokio::spawn(async move { sink.finish(other, || {}).await });
            reached.await.unwrap();
            let cancel = gate.cancel();
            release.send(()).unwrap();
            let result = task.await.unwrap();
            if before {
                cancel.unwrap();
                assert!(matches!(result, Err(Error::Cancelled)));
                empty(&inbox);
            } else {
                assert_eq!(cancel, Err(Error::CommitStarted));
                assert!(result.unwrap().is_dir());
            }
        }
        assert!(fs::read_dir(&inbox).unwrap().all(|e| {
            !e.unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".incoming-")
        }));
    }
    #[test]
    fn atomic_directory_rename_never_replaces_files_or_directories() {
        let dir = tempfile::tempdir().unwrap();
        for kind in 0..3 {
            let from = dir.path().join(format!("from{kind}"));
            fs::create_dir(&from).unwrap();
            fs::write(from.join("new"), b"new").unwrap();
            let to = dir.path().join(format!("to{kind}"));
            if kind == 0 {
                fs::write(&to, b"existing").unwrap();
            } else {
                fs::create_dir(&to).unwrap();
                if kind == 2 {
                    fs::write(to.join("existing"), b"existing").unwrap();
                }
            }
            assert!(rename_noreplace(&from, &to).is_err());
            assert!(from.join("new").exists());
            if kind == 0 {
                assert_eq!(fs::read(to).unwrap(), b"existing");
            } else {
                assert!(!to.join("new").exists());
            }
        }
        let from = dir.path().join("success");
        fs::create_dir(&from).unwrap();
        let to = dir.path().join("published");
        rename_noreplace(&from, &to).unwrap();
        assert!(to.is_dir());
        assert!(!from.exists());
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn symlinks_special_files_and_changed_ancestors_are_rejected() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("source");
        fs::create_dir(&folder).unwrap();
        let other = dir.path().join("other");
        fs::write(&other, b"original").unwrap();
        symlink(&other, folder.join("link")).unwrap();
        assert!(BundleSource::folder(&folder).await.is_err());
        assert!(
            BundleSource::files(vec![folder.join("link")])
                .await
                .is_err()
        );
        fs::remove_file(folder.join("link")).unwrap();
        fs::create_dir(folder.join("sub")).unwrap();
        fs::write(folder.join("sub/file"), b"original").unwrap();
        let mut source = BundleSource::folder(&folder).await.unwrap();
        fs::rename(folder.join("sub"), folder.join("old")).unwrap();
        symlink(folder.join("old"), folder.join("sub")).unwrap();
        assert!(source.next().await.is_err());
        let inbox = dir.path().join("inbox");
        symlink(&folder, &inbox).unwrap();
        assert!(BundleSink::create(&inbox, manifest(vec![])).await.is_err());
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn sparse_file_beyond_four_gib_streams_with_u64_bounded_chunks() {
        use std::os::unix::fs::MetadataExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("large.bin");
        let file = fs::File::create(&path).unwrap();
        let size = 4 * 1024 * 1024 * 1024u64 + 17;
        file.set_len(size).unwrap();
        assert!(file.metadata().unwrap().blocks() * 512 < size / 100);
        let mut source = Source::open(&path).await.unwrap();
        assert_eq!(source.size, size);
        let mut count = 0u64;
        while let Some(chunk) = source.next_chunk().await.unwrap() {
            assert!(chunk.len() <= CHUNK_SIZE);
            assert!(chunk.iter().all(|b| *b == 0));
            count += chunk.len() as u64;
        }
        assert_eq!(count, size);
        assert!(count > u32::MAX as u64);
    }
}

#[cfg(test)]
mod additional_tests {
    use super::*;
    #[tokio::test]
    async fn flat_name_collisions_and_size_changes_since_enumeration_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("a")).unwrap();
        fs::create_dir(dir.path().join("b")).unwrap();
        let a = dir.path().join("a/file");
        let b = dir.path().join("b/file");
        fs::write(&a, b"x").unwrap();
        fs::write(&b, b"y").unwrap();
        assert!(BundleSource::files(vec![a.clone(), b]).await.is_err());
        let mut source = BundleSource::folder(&dir.path().join("a")).await.unwrap();
        fs::write(a, b"longer").unwrap();
        assert!(matches!(source.next().await, Err(Error::FileChanged)));
    }
    #[test]
    fn large_manifest_sizes_remain_u64_and_negative_sizes_are_invalid() {
        let size = 4 * 1024 * 1024 * 1024u64 + 17;
        let m = Manifest {
            name: "Large".into(),
            entries: vec![Entry {
                path: "file".into(),
                kind: Kind::File,
                size,
            }],
            total: size,
        };
        m.validate().unwrap();
        let json = serde_json::to_string(&m).unwrap();
        let decoded: Manifest = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.total, size);
        assert_eq!(decoded.entries[0].size, size);
        assert!(serde_json::from_str::<Manifest>(&json.replace(&size.to_string(), "-1")).is_err());
        assert!(
            serde_json::from_str::<Manifest>(
                r#"{"name":"X","total":0,"entries":[{"path":"x","kind":"symlink","size":0}]}"#
            )
            .is_err()
        );
    }
}
