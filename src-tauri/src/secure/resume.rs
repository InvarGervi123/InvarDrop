//! Authenticated, bounded restart checkpoints. All methods do blocking IO: call
//! from spawn_blocking, never from the UI thread. Data is synced before metadata.
use super::{
    bundle::{self, Kind},
    error::{Error, Result},
    files::{self, CHUNK_SIZE, CommitGate},
    store::Store,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

pub const CHECKPOINT_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_RECORDS: usize = 8;
pub const MAX_RESERVED: u64 = 2 * files::MAX_FILE_SIZE;
pub const MAX_AGE: u64 = 7 * 24 * 60 * 60;
const MAX_RECORD: u64 = 8 * 1024 * 1024;
const MAX_MANIFEST: usize = 512 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub tree: bundle::Manifest,
    #[serde(deserialize_with = "bounded_hashes")]
    pub hashes: Vec<String>,
    pub folder: bool,
}
fn bounded_hashes<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Vec<String>, D::Error> {
    struct Visitor;
    impl<'de> serde::de::Visitor<'de> for Visitor {
        type Value = Vec<String>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a bounded list of BLAKE3 hashes")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut sequence: A,
        ) -> std::result::Result<Self::Value, A::Error> {
            let mut hashes = Vec::new();
            while let Some(hash) = sequence.next_element::<String>()? {
                if hashes.len() == bundle::MAX_ENTRIES || hash.len() > 64 {
                    return Err(serde::de::Error::custom("hash list limit"));
                }
                hashes.push(hash);
            }
            Ok(hashes)
        }
    }
    deserializer.deserialize_seq(Visitor)
}
impl Manifest {
    pub fn validate(&self) -> Result<()> {
        self.tree.validate()?;
        if self.hashes.len() != self.tree.entries.len() {
            return Err(Error::Integrity);
        }
        if !self.folder
            && (self.tree.entries.len() != 1
                || self.tree.entries[0].kind != Kind::File
                || self.tree.entries[0].path != self.tree.name)
        {
            return Err(Error::Protocol);
        }
        for (entry, hash) in self.tree.entries.iter().zip(&self.hashes) {
            match entry.kind {
                Kind::Directory if !hash.is_empty() => return Err(Error::Integrity),
                Kind::File => {
                    parse_hash(hash)?;
                    if entry.size == 0 && *hash != empty_hash() {
                        return Err(Error::Integrity);
                    }
                }
                _ => {}
            }
        }
        if serde_json::to_vec(self).map_err(|_| Error::Corrupt)?.len() > MAX_MANIFEST {
            return Err(Error::Limit);
        }
        Ok(())
    }
    pub fn digest(&self) -> String {
        blake3::hash(&serde_json::to_vec(self).unwrap_or_default())
            .to_hex()
            .to_string()
    }
    fn start(&self) -> Position {
        let mut p = Position {
            entry: 0,
            offset: 0,
            hash: empty_hash(),
        };
        self.skip_dirs(&mut p);
        p
    }
    fn skip_dirs(&self, p: &mut Position) {
        while self
            .tree
            .entries
            .get(p.entry as usize)
            .is_some_and(|e| e.kind == Kind::Directory)
        {
            p.entry += 1;
        }
    }
    fn position(&self, p: &Position) -> Result<()> {
        parse_hash(&p.hash)?;
        if p.entry as usize > self.tree.entries.len() {
            return Err(Error::Protocol);
        }
        match self.tree.entries.get(p.entry as usize) {
            None if p.offset == 0 && p.hash == empty_hash() => Ok(()),
            Some(e)
                if e.kind == Kind::File
                    && p.offset <= e.size
                    && (p.offset == e.size || p.offset.is_multiple_of(CHECKPOINT_BYTES))
                    && (p.offset != 0 || p.hash == empty_hash())
                    && (p.offset != e.size || p.hash == self.hashes[p.entry as usize]) =>
            {
                Ok(())
            }
            _ => Err(Error::Integrity),
        }
    }
    pub fn verified(&self, p: &Position) -> u64 {
        self.tree
            .entries
            .iter()
            .take(p.entry as usize)
            .map(|e| e.size)
            .sum::<u64>()
            .saturating_add(p.offset)
            .min(self.tree.total)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Position {
    pub entry: u32,
    pub offset: u64,
    pub hash: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub id: String,
    pub peer: String,
    pub direction: String,
    pub manifest: Manifest,
    pub position: Position,
    pub updated: u64,
    pub completed: bool,
    epoch: u64,
    paths: Vec<PathBuf>,
    receipt: Option<String>,
}
impl Record {
    pub fn verified(&self) -> u64 {
        self.manifest.verified(&self.position)
    }
}
#[derive(Clone)]
pub struct Catalog {
    root: PathBuf,
    inbox: PathBuf,
    store: Arc<Mutex<Store>>,
    mutex: Arc<Mutex<()>>,
    active: Arc<Mutex<HashSet<String>>>,
}
struct Lease {
    id: String,
    active: Arc<Mutex<HashSet<String>>>,
}
impl Drop for Lease {
    fn drop(&mut self) {
        lock(&self.active).remove(&self.id);
    }
}
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}
fn now() -> Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .map_err(|_| Error::Storage)
}
fn valid_id(id: &str) -> Result<()> {
    if uuid::Uuid::parse_str(id).is_ok_and(|v| v.to_string() == id && v.get_version_num() == 4) {
        Ok(())
    } else {
        Err(Error::Protocol)
    }
}
fn empty_hash() -> String {
    blake3::hash(b"").to_hex().to_string()
}
fn parse_hash(s: &str) -> Result<blake3::Hash> {
    if s.len() != 64
        || s.bytes()
            .any(|b| !b.is_ascii_hexdigit() || b.is_ascii_uppercase())
    {
        return Err(Error::Integrity);
    }
    blake3::Hash::from_hex(s).map_err(|_| Error::Integrity)
}
fn private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path).map_err(|_| Error::FileIo)?;
    if !bundle::regular_metadata(path)?.is_dir() {
        return Err(Error::FileIo);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|_| Error::FileIo)?;
    }
    Ok(())
}
// Remove only an empty receipt on errors/cancellation. Never recursively delete
// a collision or an already published folder owned by the destination user.
struct EmptyReceipt(PathBuf);
impl Drop for EmptyReceipt {
    fn drop(&mut self) {
        let _ = fs::remove_dir(&self.0);
    }
}
fn sync_dir(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        File::open(path)
            .and_then(|f| f.sync_all())
            .map_err(|_| Error::FileIo)?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
fn regular_file(path: &Path, write: bool) -> Result<File> {
    for ancestor in path.ancestors() {
        bundle::regular_metadata(ancestor)?;
    }
    if !bundle::regular_metadata(path)?.is_file() {
        return Err(Error::FileIo);
    }
    let mut options = OpenOptions::new();
    options.read(true).write(write);
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options.open(path).map_err(|_| Error::FileIo)?;
    if !file.metadata().map_err(|_| Error::FileIo)?.is_file() {
        return Err(Error::FileIo);
    }
    Ok(file)
}
fn check_cancel(gate: Option<&CommitGate>) -> Result<()> {
    if let Some(gate) = gate {
        gate.check()?;
    }
    Ok(())
}
#[cfg(test)]
thread_local! { static HASH_CHUNK_HOOK:std::cell::RefCell<Option<Box<dyn FnOnce()>>> = std::cell::RefCell::new(None); }
#[cfg(test)]
fn after_hash_chunk() {
    HASH_CHUNK_HOOK.with(|hook| {
        if let Some(hook) = hook.borrow_mut().take() {
            hook();
        }
    });
}
fn hash_prefix(file: &mut File, size: u64, gate: Option<&CommitGate>) -> Result<blake3::Hasher> {
    check_cancel(gate)?;
    file.seek(SeekFrom::Start(0)).map_err(|_| Error::FileIo)?;
    let mut hasher = blake3::Hasher::new();
    let mut remaining = size;
    let mut buffer = [0; CHUNK_SIZE];
    while remaining > 0 {
        check_cancel(gate)?;
        let len = remaining.min(CHUNK_SIZE as u64) as usize;
        file.read_exact(&mut buffer[..len])
            .map_err(|_| Error::Integrity)?;
        hasher.update(&buffer[..len]);
        remaining -= len as u64;
        #[cfg(test)]
        after_hash_chunk();
    }
    Ok(hasher)
}
fn full_hash(path: &Path, size: u64) -> Result<String> {
    full_hash_checked(path, size, None)
}
fn full_hash_checked(path: &Path, size: u64, gate: Option<&CommitGate>) -> Result<String> {
    check_cancel(gate)?;
    let mut file = regular_file(path, false)?;
    if file.metadata().map_err(|_| Error::FileIo)?.len() != size {
        return Err(Error::FileChanged);
    }
    let hash = hash_prefix(&mut file, size, gate)?
        .finalize()
        .to_hex()
        .to_string();
    let mut extra = [0];
    if file.read(&mut extra).map_err(|_| Error::FileIo)? != 0 {
        return Err(Error::FileChanged);
    }
    Ok(hash)
}
impl Catalog {
    pub(crate) fn prepared_references(&self) -> Result<Vec<PathBuf>> {
        let _guard = lock(&self.mutex);
        let mut paths = Vec::new();
        for entry in fs::read_dir(&self.root)
            .map_err(|_| Error::Storage)?
            .take(64)
        {
            let entry = entry.map_err(|_| Error::Storage)?;
            let id = entry
                .file_name()
                .into_string()
                .map_err(|_| Error::Corrupt)?;
            let record = self.read(&id, None)?;
            paths.extend(record.paths);
        }
        Ok(paths)
    }

    /// root is the security profile directory, not the checkpoint subdirectory.
    pub fn new(root: PathBuf, store: Arc<Mutex<Store>>) -> Result<Self> {
        if !root.is_absolute() {
            return Err(Error::Storage);
        }
        let catalog = Self {
            root: root.join("resume-v1"),
            inbox: root.join("received"),
            store,
            mutex: Arc::new(Mutex::new(())),
            active: Arc::new(Mutex::new(HashSet::new())),
        };
        private_dir(&catalog.root)?;
        catalog.list()?;
        Ok(catalog)
    }
    fn directory(&self, id: &str) -> Result<PathBuf> {
        valid_id(id)?;
        Ok(self.root.join(id))
    }
    fn check(&self, record: &Record, peer: Option<&str>) -> Result<()> {
        valid_id(&record.id)?;
        record.manifest.validate()?;
        record.manifest.position(&record.position)?;
        if !matches!(record.direction.as_str(), "incoming" | "outgoing") {
            return Err(Error::Corrupt);
        }
        if peer.is_some_and(|p| p != record.peer) {
            return Err(Error::Identity);
        }
        if record.paths.len() > bundle::MAX_ENTRIES
            || record
                .paths
                .iter()
                .any(|p| !p.is_absolute() || p.to_string_lossy().len() > 1024)
        {
            return Err(Error::Limit);
        }
        if record.direction == "outgoing"
            && record.paths.len() != record.manifest.tree.entries.len()
        {
            return Err(Error::Corrupt);
        }
        if record.direction == "incoming" && !record.paths.is_empty() {
            return Err(Error::Corrupt);
        }
        if let Some(receipt) = &record.receipt {
            if !receipt.starts_with("received-") {
                return Err(Error::Corrupt);
            }
            valid_id(&receipt[9..])?;
        }
        if record.completed && (record.direction != "incoming" || record.receipt.is_none()) {
            return Err(Error::Corrupt);
        }
        let store = lock(&self.store);
        if !store.is_trusted(&record.peer) || record.epoch != store.resume_epoch() {
            return Err(Error::Revoked);
        }
        if now()?.saturating_sub(record.updated) > MAX_AGE
            || record.updated > now()?.saturating_add(300)
        {
            return Err(Error::Expired);
        }
        Ok(())
    }
    fn read(&self, id: &str, peer: Option<&str>) -> Result<Record> {
        let dir = self.directory(id)?;
        if !bundle::regular_metadata(&dir)?.is_dir() {
            return Err(Error::Corrupt);
        }
        let mut bytes = Vec::new();
        regular_file(&dir.join("checkpoint.enc"), false)?
            .take(MAX_RECORD + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Error::Storage)?;
        if bytes.len() as u64 > MAX_RECORD {
            return Err(Error::Limit);
        }
        let plain = lock(&self.store).open_resume(&bytes)?;
        let mut record: Record = serde_json::from_slice(&plain).map_err(|_| Error::Corrupt)?;
        if record.id != id {
            return Err(Error::Corrupt);
        }
        self.check(&record, peer)?;
        if record.direction == "incoming"
            && record.receipt.is_some()
            && !record.completed
            && !lock(&self.active).contains(id)
        {
            self.recover(&mut record)?;
        }
        Ok(record)
    }
    fn save(&self, record: &Record) -> Result<()> {
        self.check(record, None)?;
        let bytes = serde_json::to_vec(record).map_err(|_| Error::Storage)?;
        if bytes.len() as u64 > MAX_RECORD - 64 {
            return Err(Error::Limit);
        }
        let bytes = lock(&self.store).seal_resume(&bytes)?;
        let directory = self.directory(&record.id)?;
        let mut file = tempfile::NamedTempFile::new_in(&directory).map_err(|_| Error::Storage)?;
        file.write_all(&bytes)
            .and_then(|_| file.as_file().sync_all())
            .map_err(|_| Error::Storage)?;
        file.persist(directory.join("checkpoint.enc"))
            .map_err(|_| Error::Storage)?;
        sync_dir(&directory)
    }
    fn recover(&self, record: &mut Record) -> Result<()> {
        let dir = self.directory(&record.id)?;
        let payload = dir.join("payload");
        let source = self.publish_source(record, &payload)?;
        let destination = self.destination(record)?;
        if source.try_exists().map_err(|_| Error::FileIo)? {
            if record.completed {
                return Err(Error::Corrupt);
            }
            // No rename happened. Never remove an occupied destination.
            if let Some(receipt) = destination.parent() {
                let _ = fs::remove_dir(receipt);
            }
            record.receipt = None;
            self.save(record)?;
        } else {
            self.verify_published(record, &destination)?;
            if !record.completed {
                record.completed = true;
                record.position = Position {
                    entry: record.manifest.tree.entries.len() as u32,
                    offset: 0,
                    hash: empty_hash(),
                };
                self.save(record)?;
            }
        }
        Ok(())
    }
    fn publish_source(&self, record: &Record, payload: &Path) -> Result<PathBuf> {
        if record.manifest.folder {
            Ok(payload.to_owned())
        } else {
            Ok(payload.join(
                &record
                    .manifest
                    .tree
                    .entries
                    .first()
                    .ok_or(Error::Corrupt)?
                    .path,
            ))
        }
    }
    fn destination(&self, record: &Record) -> Result<PathBuf> {
        Ok(self
            .inbox
            .join(record.receipt.as_ref().ok_or(Error::Corrupt)?)
            .join(&record.manifest.tree.name))
    }
    fn verify_published(&self, record: &Record, path: &Path) -> Result<()> {
        if record.manifest.folder {
            verify_tree(&record.manifest, path, None)?;
        } else if full_hash(path, record.manifest.tree.total)? != record.manifest.hashes[0] {
            return Err(Error::Integrity);
        }
        Ok(())
    }
    fn clean_checkpoint_temps(&self, id: &str) -> Result<()> {
        let directory = self.directory(id)?;
        if !bundle::regular_metadata(&directory)?.is_dir() {
            return Err(Error::Corrupt);
        }
        let mut count = 0;
        for entry in fs::read_dir(&directory).map_err(|_| Error::Storage)? {
            count += 1;
            if count > 64 {
                return Err(Error::Limit);
            }
            let entry = entry.map_err(|_| Error::Storage)?;
            // NamedTempFile's atomic-write artifacts live only at this level;
            // source filenames are inside payload and are never considered.
            if entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with(".tmp"))
                && bundle::regular_metadata(&entry.path()).is_ok_and(|metadata| metadata.is_file())
            {
                fs::remove_file(entry.path()).map_err(|_| Error::Storage)?;
            }
        }
        Ok(())
    }
    pub fn list(&self) -> Result<Vec<Record>> {
        let _guard = lock(&self.mutex);
        let mut records = Vec::new();
        let mut count = 0;
        for entry in fs::read_dir(&self.root).map_err(|_| Error::Storage)? {
            count += 1;
            if count > 64 {
                return Err(Error::Limit);
            }
            let entry = entry.map_err(|_| Error::Storage)?;
            let Some(id) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if valid_id(&id).is_err() {
                continue;
            }
            if lock(&self.active).contains(&id) {
                continue;
            }
            match self
                .clean_checkpoint_temps(&id)
                .and_then(|_| self.read(&id, None))
            {
                Ok(record) => records.push(record),
                Err(_) => {
                    if bundle::regular_metadata(&entry.path()).is_ok_and(|m| m.is_dir()) {
                        fs::remove_dir_all(entry.path()).map_err(|_| Error::Storage)?;
                    }
                }
            }
        }
        if records.len() > MAX_RECORDS {
            return Err(Error::Limit);
        }
        Ok(records)
    }
    pub fn get(&self, peer: &str, id: &str) -> Result<Record> {
        let _guard = lock(&self.mutex);
        self.read(id, Some(peer))
    }
    pub fn completed(&self, peer: &str, id: &str) -> Result<Option<PathBuf>> {
        let record = self.get(peer, id)?;
        if record.completed {
            let destination = self.destination(&record)?;
            self.verify_published(&record, &destination)?;
            Ok(Some(destination))
        } else {
            Ok(None)
        }
    }
    fn capacity(&self, size: u64) -> Result<()> {
        // Caller holds the catalogue mutex; inspect only authenticated records.
        let mut count = 0;
        let mut total = size;
        for entry in fs::read_dir(&self.root)
            .map_err(|_| Error::Storage)?
            .take(MAX_RECORDS + 1)
        {
            let entry = entry.map_err(|_| Error::Storage)?;
            let Some(id) = entry.file_name().to_str().map(str::to_owned) else {
                return Err(Error::Corrupt);
            };
            let record = self.read(&id, None)?;
            count += 1;
            total = total
                .checked_add(record.manifest.tree.total)
                .ok_or(Error::Limit)?;
        }
        if count >= MAX_RECORDS || total > MAX_RESERVED {
            return Err(Error::Limit);
        }
        Ok(())
    }
    fn lease(&self, id: &str) -> Result<Lease> {
        if !lock(&self.active).insert(id.into()) {
            return Err(Error::Busy);
        }
        Ok(Lease {
            id: id.into(),
            active: self.active.clone(),
        })
    }
    pub fn prepare(
        &self,
        peer: &str,
        tree: &bundle::Manifest,
        paths: Vec<PathBuf>,
        folder: bool,
    ) -> Result<Record> {
        tree.validate()?;
        if paths.len() != tree.entries.len()
            || paths
                .iter()
                .any(|p| !p.is_absolute() || p.to_string_lossy().len() > 1024)
        {
            return Err(Error::Limit);
        }
        let epoch = {
            let store = lock(&self.store);
            if !store.is_trusted(peer) {
                return Err(Error::Revoked);
            }
            store.resume_epoch()
        };
        let mut hashes = Vec::with_capacity(paths.len());
        for (entry, path) in tree.entries.iter().zip(&paths) {
            hashes.push(if entry.kind == Kind::Directory {
                for ancestor in path.ancestors() {
                    bundle::regular_metadata(ancestor)?;
                }
                if !bundle::regular_metadata(path)?.is_dir() {
                    return Err(Error::FileChanged);
                }
                String::new()
            } else {
                full_hash(path, entry.size)?
            });
        }
        let manifest = Manifest {
            tree: tree.clone(),
            hashes,
            folder,
        };
        manifest.validate()?;
        let record = Record {
            id: uuid::Uuid::new_v4().to_string(),
            peer: peer.into(),
            direction: "outgoing".into(),
            position: manifest.start(),
            manifest,
            updated: now()?,
            completed: false,
            epoch,
            paths,
            receipt: None,
        };
        let _guard = lock(&self.mutex);
        self.capacity(record.manifest.tree.total)?;
        let dir = self.directory(&record.id)?;
        fs::create_dir(&dir).map_err(|_| Error::Storage)?;
        if let Err(error) = self.save(&record) {
            let _ = fs::remove_dir_all(dir);
            return Err(error);
        }
        sync_dir(&self.root)?;
        Ok(record)
    }
    #[cfg(test)]
    pub fn accept(&self, peer: &str, id: &str, manifest: Manifest) -> Result<Receiver> {
        self.accept_cancellable(peer, id, manifest, Arc::new(CommitGate::default()))
    }
    pub fn accept_cancellable(
        &self,
        peer: &str,
        id: &str,
        manifest: Manifest,
        gate: Arc<CommitGate>,
    ) -> Result<Receiver> {
        gate.check()?;
        manifest.validate()?;
        valid_id(id)?;
        {
            let _guard = lock(&self.mutex);
            let dir = self.directory(id)?;
            if dir.try_exists().map_err(|_| Error::Storage)? {
                let record = self.read(id, Some(peer))?;
                if record.direction != "incoming"
                    || record.manifest.digest() != manifest.digest()
                    || record.completed
                {
                    return Err(Error::Integrity);
                }
            } else {
                self.capacity(manifest.tree.total)?;
                let epoch = {
                    let store = lock(&self.store);
                    if !store.is_trusted(peer) {
                        return Err(Error::Revoked);
                    }
                    store.resume_epoch()
                };
                let record = Record {
                    id: id.into(),
                    peer: peer.into(),
                    direction: "incoming".into(),
                    position: manifest.start(),
                    manifest,
                    updated: now()?,
                    completed: false,
                    epoch,
                    paths: Vec::new(),
                    receipt: None,
                };
                private_dir(&dir)?;
                if let Err(error) = self
                    .create_payload_checked(&record, Some(&gate))
                    .and_then(|_| self.save(&record))
                {
                    let _ = fs::remove_dir_all(dir);
                    return Err(error);
                }
                sync_dir(&self.root)?;
            }
        }
        self.receiver_checked(peer, id, Some(&gate))
    }
    fn create_payload(&self, record: &Record) -> Result<()> {
        self.create_payload_checked(record, None)
    }
    fn create_payload_checked(&self, record: &Record, gate: Option<&CommitGate>) -> Result<()> {
        let payload = self.directory(&record.id)?.join("payload");
        private_dir(&payload)?;
        for entry in &record.manifest.tree.entries {
            check_cancel(gate)?;
            if entry.kind == Kind::Directory {
                fs::create_dir(payload.join(&entry.path)).map_err(|_| Error::FileIo)?;
            }
        }
        // Directory names must be durable before a checkpoint can refer to
        // files inside them. Windows has no portable directory-fsync API here.
        for entry in record
            .manifest
            .tree
            .entries
            .iter()
            .rev()
            .filter(|e| e.kind == Kind::Directory)
        {
            check_cancel(gate)?;
            sync_dir(&payload.join(&entry.path))?;
        }
        sync_dir(&payload)
    }
    #[cfg(test)]
    pub fn sender(&self, peer: &str, id: &str, position: Position) -> Result<Sender> {
        self.sender_cancellable(peer, id, position, Arc::new(CommitGate::default()))
    }
    pub fn sender_cancellable(
        &self,
        peer: &str,
        id: &str,
        position: Position,
        gate: Arc<CommitGate>,
    ) -> Result<Sender> {
        gate.check()?;
        let _guard = lock(&self.mutex);
        let record = self.read(id, Some(peer))?;
        if record.direction != "outgoing" {
            return Err(Error::Protocol);
        }
        record.manifest.position(&position)?;
        let lease = self.lease(id)?;
        for (index, (entry, path)) in record
            .manifest
            .tree
            .entries
            .iter()
            .zip(&record.paths)
            .enumerate()
        {
            gate.check()?;
            if entry.kind == Kind::File
                && full_hash_checked(path, entry.size, Some(&gate))?
                    != record.manifest.hashes[index]
            {
                return Err(Error::FileChanged);
            }
            if entry.kind == Kind::Directory {
                for ancestor in path.ancestors() {
                    bundle::regular_metadata(ancestor)?;
                }
                if !bundle::regular_metadata(path)?.is_dir() {
                    return Err(Error::FileChanged);
                }
            }
        }
        let mut current = None;
        if let Some(path) = record.paths.get(position.entry as usize) {
            let mut file = regular_file(path, false)?;
            let hasher = hash_prefix(&mut file, position.offset, Some(&gate))?;
            if hasher.finalize().to_hex().as_str() != position.hash {
                return Err(Error::Integrity);
            }
            current = Some(Current {
                file,
                hasher,
                offset: position.offset,
            });
        }
        Ok(Sender {
            record,
            position,
            current,
            pending: false,
            _lease: lease,
        })
    }
    #[cfg(test)]
    pub fn receiver(&self, peer: &str, id: &str) -> Result<Receiver> {
        self.receiver_checked(peer, id, None)
    }
    fn receiver_checked(
        &self,
        peer: &str,
        id: &str,
        gate: Option<&CommitGate>,
    ) -> Result<Receiver> {
        check_cancel(gate)?;
        let _guard = lock(&self.mutex);
        let record = self.read(id, Some(peer))?;
        if record.direction != "incoming" || record.completed {
            return Err(Error::Protocol);
        }
        let lease = self.lease(id)?;
        let payload = self.directory(id)?.join("payload");
        validate_checkpoint(&record, &payload, gate)?;
        let current = if record.position.offset > 0 {
            let entry = &record.manifest.tree.entries[record.position.entry as usize];
            let mut file = regular_file(&payload.join(&entry.path), true)?;
            file.set_len(record.position.offset)
                .map_err(|_| Error::FileIo)?;
            let hasher = hash_prefix(&mut file, record.position.offset, gate)?;
            Some(Current {
                file,
                hasher,
                offset: record.position.offset,
            })
        } else {
            None
        };
        // Remove unverified files after the contiguous checkpoint. They can have
        // been created before a crash but cannot be reused as verified data.
        for (index, entry) in record.manifest.tree.entries.iter().enumerate() {
            if entry.kind == Kind::File
                && index >= record.position.entry as usize
                && (index != record.position.entry as usize || record.position.offset == 0)
            {
                let path = payload.join(&entry.path);
                if path.try_exists().map_err(|_| Error::FileIo)? {
                    bundle::regular_metadata(&path)?;
                    fs::remove_file(path).map_err(|_| Error::FileIo)?;
                }
            }
        }
        Ok(Receiver {
            catalog: self.clone(),
            position: record.position.clone(),
            record,
            current,
            _lease: lease,
        })
    }
    pub fn note_position(&self, peer: &str, id: &str, position: Position) -> Result<()> {
        let _guard = lock(&self.mutex);
        let mut record = self.read(id, Some(peer))?;
        if record.direction != "outgoing" {
            return Err(Error::Protocol);
        }
        record.manifest.position(&position)?;
        record.position = position;
        record.updated = now()?;
        self.save(&record)
    }
    pub fn cancel(&self, peer: &str, id: &str) -> Result<()> {
        let _guard = lock(&self.mutex);
        if lock(&self.active).contains(id) {
            return Err(Error::Busy);
        }
        let mut record = self.read(id, Some(peer))?;
        if record.completed {
            return Err(Error::CommitStarted);
        }
        if record.direction == "incoming" {
            let payload = self.directory(id)?.join("payload");
            if payload.try_exists().map_err(|_| Error::FileIo)? {
                fs::remove_dir_all(payload).map_err(|_| Error::FileIo)?;
            }
            self.create_payload(&record)?;
        }
        record.position = record.manifest.start();
        record.updated = now()?;
        record.receipt = None;
        self.save(&record)
    }
    pub fn discard(&self, peer: &str, id: &str) -> Result<()> {
        let _guard = lock(&self.mutex);
        if lock(&self.active).contains(id) {
            return Err(Error::Busy);
        }
        let _ = self.read(id, Some(peer))?;
        fs::remove_dir_all(self.directory(id)?).map_err(|_| Error::Storage)?;
        sync_dir(&self.root)
    }
    pub fn mark_complete(&self, peer: &str, id: &str) -> Result<()> {
        self.discard(peer, id)
    }
}
fn validate_checkpoint(record: &Record, payload: &Path, gate: Option<&CommitGate>) -> Result<()> {
    check_cancel(gate)?;
    if !bundle::regular_metadata(payload)?.is_dir() {
        return Err(Error::Corrupt);
    }
    for (index, entry) in record.manifest.tree.entries.iter().enumerate() {
        check_cancel(gate)?;
        let path = payload.join(&entry.path);
        if entry.kind == Kind::Directory {
            if !bundle::regular_metadata(&path)?.is_dir() {
                return Err(Error::Corrupt);
            }
            continue;
        }
        if index < record.position.entry as usize {
            if full_hash_checked(&path, entry.size, gate)? != record.manifest.hashes[index] {
                return Err(Error::Integrity);
            }
        } else if index == record.position.entry as usize && record.position.offset > 0 {
            let mut file = regular_file(&path, false)?;
            if file.metadata().map_err(|_| Error::FileIo)?.len() < record.position.offset
                || hash_prefix(&mut file, record.position.offset, gate)?
                    .finalize()
                    .to_hex()
                    .as_str()
                    != record.position.hash
            {
                return Err(Error::Integrity);
            }
        }
    }
    Ok(())
}
fn verify_tree(manifest: &Manifest, payload: &Path, gate: Option<&CommitGate>) -> Result<()> {
    check_cancel(gate)?;
    if !bundle::regular_metadata(payload)?.is_dir() {
        return Err(Error::Integrity);
    }
    let mut actual = HashSet::new();
    fn walk(
        root: &Path,
        relative: &str,
        actual: &mut HashSet<String>,
        gate: Option<&CommitGate>,
    ) -> Result<()> {
        for entry in fs::read_dir(root.join(relative)).map_err(|_| Error::FileIo)? {
            check_cancel(gate)?;
            if actual.len() >= bundle::MAX_ENTRIES {
                return Err(Error::Limit);
            }
            let entry = entry.map_err(|_| Error::FileIo)?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| Error::InvalidName)?;
            files::validate_name(&name)?;
            let path = if relative.is_empty() {
                name
            } else {
                format!("{relative}/{name}")
            };
            if path.split('/').count() > bundle::MAX_DEPTH {
                return Err(Error::Limit);
            }
            let metadata = bundle::regular_metadata(&entry.path())?;
            actual.insert(path.clone());
            if metadata.is_dir() {
                walk(root, &path, actual, gate)?;
            }
        }
        Ok(())
    }
    walk(payload, "", &mut actual, gate)?;
    if actual.len() != manifest.tree.entries.len() {
        return Err(Error::Integrity);
    }
    for (index, entry) in manifest.tree.entries.iter().enumerate() {
        if !actual.contains(&entry.path) {
            return Err(Error::Integrity);
        }
        let path = payload.join(&entry.path);
        if entry.kind == Kind::Directory {
            if !bundle::regular_metadata(&path)?.is_dir() {
                return Err(Error::Integrity);
            }
        } else if full_hash_checked(&path, entry.size, gate)? != manifest.hashes[index] {
            return Err(Error::Integrity);
        }
    }
    Ok(())
}
struct Current {
    file: File,
    hasher: blake3::Hasher,
    offset: u64,
}
pub enum Event {
    Data(Vec<u8>),
    Checkpoint(Position),
    FileEnd { index: u32, hash: String },
    Done,
}
pub struct Sender {
    pub record: Record,
    position: Position,
    current: Option<Current>,
    pending: bool,
    _lease: Lease,
}
impl Sender {
    pub fn next(&mut self) -> Result<Event> {
        if self.pending {
            self.pending = false;
            return Ok(Event::Checkpoint(self.position.clone()));
        }
        let Some(entry) = self
            .record
            .manifest
            .tree
            .entries
            .get(self.position.entry as usize)
        else {
            return Ok(Event::Done);
        };
        if self.current.is_none() {
            let file = regular_file(&self.record.paths[self.position.entry as usize], false)?;
            self.current = Some(Current {
                file,
                hasher: blake3::Hasher::new(),
                offset: 0,
            });
        }
        let current = self.current.as_mut().ok_or(Error::FileIo)?;
        if current.offset == entry.size {
            let mut extra = [0];
            if current.file.read(&mut extra).map_err(|_| Error::FileIo)? != 0 {
                return Err(Error::FileChanged);
            }
            let hash = current.hasher.finalize().to_hex().to_string();
            if hash != self.record.manifest.hashes[self.position.entry as usize] {
                return Err(Error::FileChanged);
            }
            let event = Event::FileEnd {
                index: self.position.entry,
                hash,
            };
            self.current = None;
            self.position.entry += 1;
            self.position.offset = 0;
            self.position.hash = empty_hash();
            self.record.manifest.skip_dirs(&mut self.position);
            return Ok(event);
        }
        let mut data = vec![0; (entry.size - current.offset).min(CHUNK_SIZE as u64) as usize];
        current
            .file
            .read_exact(&mut data)
            .map_err(|_| Error::FileChanged)?;
        current.hasher.update(&data);
        current.offset += data.len() as u64;
        self.position.offset = current.offset;
        self.position.hash = current.hasher.finalize().to_hex().to_string();
        if current.offset % CHECKPOINT_BYTES == 0 || current.offset == entry.size {
            self.pending = true;
        }
        Ok(Event::Data(data))
    }
    pub fn position(&self) -> Position {
        self.position.clone()
    }
}
pub struct Receiver {
    catalog: Catalog,
    pub record: Record,
    position: Position,
    current: Option<Current>,
    _lease: Lease,
}
impl Receiver {
    fn open(&mut self) -> Result<()> {
        if self.current.is_some() {
            return Ok(());
        }
        let entry = self
            .record
            .manifest
            .tree
            .entries
            .get(self.position.entry as usize)
            .ok_or(Error::Protocol)?;
        let path = self
            .catalog
            .directory(&self.record.id)?
            .join("payload")
            .join(&entry.path);
        let mut options = OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(path).map_err(|_| Error::FileIo)?;
        self.current = Some(Current {
            file,
            hasher: blake3::Hasher::new(),
            offset: 0,
        });
        Ok(())
    }
    pub fn write(&mut self, data: &[u8]) -> Result<()> {
        if data.is_empty() || data.len() > CHUNK_SIZE {
            return Err(Error::Protocol);
        }
        self.open()?;
        let current = self.current.as_mut().ok_or(Error::Protocol)?;
        let size = self.record.manifest.tree.entries[self.position.entry as usize].size;
        let next = current
            .offset
            .checked_add(data.len() as u64)
            .ok_or(Error::Integrity)?;
        if next > size {
            return Err(Error::Integrity);
        }
        // Require a prefix proof at least every checkpoint interval: a peer cannot fill unbounded
        // uncheckpointed staging while avoiding verification and persistence.
        if next.saturating_sub(self.record.position.offset) > CHECKPOINT_BYTES {
            return Err(Error::Limit);
        }
        current.file.write_all(data).map_err(|_| Error::FileIo)?;
        current.hasher.update(data);
        current.offset = next;
        self.position.offset = next;
        self.position.hash = current.hasher.finalize().to_hex().to_string();
        Ok(())
    }
    pub fn checkpoint(&mut self, position: Position) -> Result<()> {
        self.record.manifest.position(&position)?;
        if position != self.position
            || position.entry == self.record.position.entry
                && position.offset <= self.record.position.offset
        {
            return Err(Error::Integrity);
        }
        let entry = self
            .record
            .manifest
            .tree
            .entries
            .get(position.entry as usize)
            .ok_or(Error::Protocol)?;
        if position.offset == entry.size
            && position.hash != self.record.manifest.hashes[position.entry as usize]
        {
            return Err(Error::Integrity);
        }
        let relative = entry.path.clone();
        self.open()?;
        self.current
            .as_ref()
            .ok_or(Error::Protocol)?
            .file
            .sync_all()
            .map_err(|_| Error::FileIo)?;
        let path = self
            .catalog
            .directory(&self.record.id)?
            .join("payload")
            .join(relative);
        sync_dir(path.parent().ok_or(Error::FileIo)?)?;
        self.record.position = position;
        self.record.updated = now()?;
        let _guard = lock(&self.catalog.mutex);
        self.catalog.save(&self.record)
    }
    pub fn file_end(&mut self, index: u32, hash: &str) -> Result<()> {
        self.open()?;
        if index != self.position.entry || hash != self.record.manifest.hashes[index as usize] {
            return Err(Error::Integrity);
        }
        let current = self.current.as_ref().ok_or(Error::Protocol)?;
        if current.offset != self.record.manifest.tree.entries[index as usize].size
            || current.hasher.finalize() != parse_hash(hash)?
        {
            return Err(Error::Integrity);
        }
        current.file.sync_all().map_err(|_| Error::FileIo)?;
        let path = self
            .catalog
            .directory(&self.record.id)?
            .join("payload")
            .join(&self.record.manifest.tree.entries[index as usize].path);
        sync_dir(path.parent().ok_or(Error::FileIo)?)?;
        self.current = None;
        self.position.entry += 1;
        self.position.offset = 0;
        self.position.hash = empty_hash();
        self.record.manifest.skip_dirs(&mut self.position);
        self.record.position = self.position.clone();
        self.record.updated = now()?;
        let _guard = lock(&self.catalog.mutex);
        self.catalog.save(&self.record)
    }
    pub fn finish(mut self, gate: Arc<CommitGate>, on_commit: impl FnOnce()) -> Result<PathBuf> {
        gate.check()?;
        if self.current.is_some()
            || self.position.entry as usize != self.record.manifest.tree.entries.len()
        {
            return Err(Error::Integrity);
        }
        let payload = self.catalog.directory(&self.record.id)?.join("payload");
        verify_tree(&self.record.manifest, &payload, Some(&gate))?;
        gate.check()?;
        for entry in self.record.manifest.tree.entries.iter().rev() {
            gate.check()?;
            let path = payload.join(&entry.path);
            if entry.kind == Kind::Directory {
                sync_dir(&path)?;
            } else {
                regular_file(&path, true)?
                    .sync_all()
                    .map_err(|_| Error::FileIo)?;
            }
        }
        sync_dir(&payload)?;
        private_dir(&self.catalog.inbox)?;
        let receipt = format!("received-{}", uuid::Uuid::new_v4());
        let receipt_path = self.catalog.inbox.join(&receipt);
        fs::create_dir(&receipt_path).map_err(|_| Error::FileIo)?;
        let _empty_receipt = EmptyReceipt(receipt_path.clone());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&receipt_path, fs::Permissions::from_mode(0o700))
                .map_err(|_| Error::FileIo)?;
        }
        self.record.receipt = Some(receipt);
        self.record.updated = now()?;
        {
            let _guard = lock(&self.catalog.mutex);
            self.catalog.save(&self.record)?;
        }
        let destination = self.catalog.destination(&self.record)?;
        let source = self.catalog.publish_source(&self.record, &payload)?;
        gate.begin()?;
        on_commit();
        bundle::rename_noreplace(&source, &destination).map_err(|_| Error::FileIo)?;
        sync_dir(&receipt_path)?;
        sync_dir(&self.catalog.inbox)?;
        self.record.completed = true;
        self.record.position = self.position.clone();
        self.record.updated = now()?;
        {
            let _guard = lock(&self.catalog.mutex);
            self.catalog.save(&self.record)?;
        }
        Ok(destination)
    }
    pub fn position(&self) -> Position {
        self.position.clone()
    }
    pub fn verified(&self) -> u64 {
        self.record.verified()
    }
}

#[cfg(test)]
mod tests {
    use super::super::{bundle::Entry, identity::Identity};
    use super::*;
    fn setup(root: &Path, peer: &str) -> (Catalog, Arc<Mutex<Store>>) {
        let mut store = Store::for_test(root, &[37; 32]).unwrap();
        store.trust(peer).unwrap();
        let store = Arc::new(Mutex::new(store));
        (Catalog::new(root.to_owned(), store.clone()).unwrap(), store)
    }
    fn peer() -> String {
        Identity::generate().unwrap().fingerprint().unwrap()
    }
    fn fixture(path: &Path, size: u64) {
        let mut file = File::create(path).unwrap();
        let data = [0xa7; CHUNK_SIZE];
        let mut remaining = size;
        while remaining > 0 {
            let len = remaining.min(CHUNK_SIZE as u64) as usize;
            file.write_all(&data[..len]).unwrap();
            remaining -= len as u64;
        }
    }
    fn tree(name: &str, size: u64) -> bundle::Manifest {
        bundle::Manifest {
            name: name.into(),
            total: size,
            entries: vec![Entry {
                path: name.into(),
                kind: Kind::File,
                size,
            }],
        }
    }
    fn finish(sender: &mut Sender, receiver: &mut Receiver) -> u64 {
        let mut sent = 0;
        loop {
            match sender.next().unwrap() {
                Event::Data(data) => {
                    sent += data.len() as u64;
                    receiver.write(&data).unwrap();
                }
                Event::Checkpoint(position) => receiver.checkpoint(position).unwrap(),
                Event::FileEnd { index, hash } => receiver.file_end(index, &hash).unwrap(),
                Event::Done => break,
            }
        }
        sent
    }
    fn through_checkpoint(sender: &mut Sender, receiver: &mut Receiver) -> Position {
        loop {
            match sender.next().unwrap() {
                Event::Data(data) => receiver.write(&data).unwrap(),
                Event::Checkpoint(position) => {
                    receiver.checkpoint(position.clone()).unwrap();
                    return position;
                }
                _ => panic!("Expected a partial checkpoint"),
            }
        }
    }
    #[test]
    fn single_file_survives_restart_and_does_not_resend_verified_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let peer = peer();
        let (send, _) = setup(&dir.path().join("sender"), &peer);
        let (receive, store) = setup(&dir.path().join("receiver"), &peer);
        let source = dir.path().join("large.bin");
        let size = CHECKPOINT_BYTES + 123;
        fixture(&source, size);
        let ticket = send
            .prepare(&peer, &tree("large.bin", size), vec![source.clone()], false)
            .unwrap();
        let mut receiver = receive
            .accept(&peer, &ticket.id, ticket.manifest.clone())
            .unwrap();
        let mut sender = send.sender(&peer, &ticket.id, receiver.position()).unwrap();
        let position = through_checkpoint(&mut sender, &mut receiver);
        assert_eq!(position.offset, CHECKPOINT_BYTES);
        assert_eq!(receiver.verified(), CHECKPOINT_BYTES);
        let Event::Data(tail) = sender.next().unwrap() else {
            panic!("Expected unverified tail")
        };
        receiver.write(&tail).unwrap();
        drop(receiver);
        drop(sender);
        let restarted = Catalog::new(dir.path().join("receiver"), store).unwrap();
        let mut receiver = restarted.receiver(&peer, &ticket.id).unwrap();
        assert_eq!(receiver.position(), position);
        let mut sender = send.sender(&peer, &ticket.id, position).unwrap();
        assert_eq!(finish(&mut sender, &mut receiver), 123);
        drop(sender);
        let destination = receiver
            .finish(Arc::new(CommitGate::default()), || {})
            .unwrap();
        assert_eq!(
            full_hash(&destination, size).unwrap(),
            full_hash(&source, size).unwrap()
        );
        assert_eq!(
            restarted.completed(&peer, &ticket.id).unwrap(),
            Some(destination.clone())
        );
        assert!(
            restarted
                .accept(&peer, &ticket.id, ticket.manifest.clone())
                .is_err()
        );
        restarted.mark_complete(&peer, &ticket.id).unwrap();
        assert!(restarted.list().unwrap().is_empty());
        assert!(destination.exists());
    }
    #[test]
    fn nested_empty_and_zero_entries_survive_multiple_interruptions() {
        let dir = tempfile::tempdir().unwrap();
        let peer = peer();
        let (send, _) = setup(&dir.path().join("sender"), &peer);
        let (receive, store) = setup(&dir.path().join("receiver"), &peer);
        let source = dir.path().join("Folder");
        fs::create_dir_all(source.join("nested/empty")).unwrap();
        let size = 2 * CHECKPOINT_BYTES + 17;
        fixture(&source.join("nested/data"), size);
        fixture(&source.join("zero"), 0);
        let tree = bundle::Manifest {
            name: "Folder".into(),
            total: size,
            entries: vec![
                Entry {
                    path: "nested".into(),
                    kind: Kind::Directory,
                    size: 0,
                },
                Entry {
                    path: "nested/data".into(),
                    kind: Kind::File,
                    size,
                },
                Entry {
                    path: "nested/empty".into(),
                    kind: Kind::Directory,
                    size: 0,
                },
                Entry {
                    path: "zero".into(),
                    kind: Kind::File,
                    size: 0,
                },
            ],
        };
        let paths = tree.entries.iter().map(|e| source.join(&e.path)).collect();
        let ticket = send.prepare(&peer, &tree, paths, true).unwrap();
        let mut receiver = receive
            .accept(&peer, &ticket.id, ticket.manifest.clone())
            .unwrap();
        let mut sender = send.sender(&peer, &ticket.id, receiver.position()).unwrap();
        let first = through_checkpoint(&mut sender, &mut receiver);
        drop(receiver);
        drop(sender);
        let restarted = Catalog::new(dir.path().join("receiver"), store.clone()).unwrap();
        let mut receiver = restarted.receiver(&peer, &ticket.id).unwrap();
        let mut sender = send.sender(&peer, &ticket.id, first).unwrap();
        let second = through_checkpoint(&mut sender, &mut receiver);
        assert_eq!(second.offset, 2 * CHECKPOINT_BYTES);
        drop(receiver);
        drop(sender);
        let restarted = Catalog::new(dir.path().join("receiver"), store).unwrap();
        let mut receiver = restarted.receiver(&peer, &ticket.id).unwrap();
        let mut sender = send.sender(&peer, &ticket.id, second).unwrap();
        assert_eq!(finish(&mut sender, &mut receiver), 17);
        drop(sender);
        let received = receiver
            .finish(Arc::new(CommitGate::default()), || {})
            .unwrap();
        assert!(received.join("nested/empty").is_dir());
        assert_eq!(fs::metadata(received.join("zero")).unwrap().len(), 0);
        assert_eq!(
            full_hash(&received.join("nested/data"), size).unwrap(),
            ticket.manifest.hashes[1]
        );
    }
    #[test]
    fn changed_source_invalid_prefix_and_corrupted_partial_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let peer = peer();
        let (send, _) = setup(&dir.path().join("sender"), &peer);
        let (receive, _) = setup(&dir.path().join("receiver"), &peer);
        let source = dir.path().join("large");
        let size = CHECKPOINT_BYTES + 19;
        fixture(&source, size);
        let ticket = send
            .prepare(&peer, &tree("large", size), vec![source.clone()], false)
            .unwrap();
        for position in [
            Position {
                entry: 0,
                offset: size + 1,
                hash: empty_hash(),
            },
            Position {
                entry: 0,
                offset: 1,
                hash: empty_hash(),
            },
            Position {
                entry: 0,
                offset: CHECKPOINT_BYTES,
                hash: empty_hash(),
            },
        ] {
            assert!(send.sender(&peer, &ticket.id, position).is_err());
        }
        let mut receiver = receive
            .accept(&peer, &ticket.id, ticket.manifest.clone())
            .unwrap();
        let mut sender = send.sender(&peer, &ticket.id, receiver.position()).unwrap();
        let position = through_checkpoint(&mut sender, &mut receiver);
        drop(sender);
        drop(receiver);
        let path = receive.directory(&ticket.id).unwrap().join("payload/large");
        let mut file = OpenOptions::new().write(true).open(&path).unwrap();
        file.write_all(b"changed").unwrap();
        drop(file);
        assert!(matches!(
            receive.receiver(&peer, &ticket.id),
            Err(Error::Integrity)
        ));
        let mut file = OpenOptions::new().write(true).open(&source).unwrap();
        file.seek(SeekFrom::Start(CHECKPOINT_BYTES + 1)).unwrap();
        file.write_all(b"x").unwrap();
        drop(file);
        assert!(matches!(
            send.sender(&peer, &ticket.id, position),
            Err(Error::FileChanged)
        ));
    }
    #[test]
    fn cancel_cleans_payload_keeps_ticket_while_discard_removes_it() {
        let dir = tempfile::tempdir().unwrap();
        let peer = peer();
        let (send, _) = setup(&dir.path().join("sender"), &peer);
        let (receive, _) = setup(&dir.path().join("receiver"), &peer);
        let source = dir.path().join("file");
        fixture(&source, 3);
        let ticket = send
            .prepare(&peer, &tree("file", 3), vec![source], false)
            .unwrap();
        let mut receiver = receive
            .accept(&peer, &ticket.id, ticket.manifest.clone())
            .unwrap();
        receiver.write(b"abc").unwrap();
        assert_eq!(receive.cancel(&peer, &ticket.id), Err(Error::Busy));
        drop(receiver);
        receive.cancel(&peer, &ticket.id).unwrap();
        assert_eq!(receive.list().unwrap().len(), 1);
        assert!(
            !receive
                .directory(&ticket.id)
                .unwrap()
                .join("payload/file")
                .exists()
        );
        assert_eq!(receive.get(&peer, &ticket.id).unwrap().verified(), 0);
        receive.discard(&peer, &ticket.id).unwrap();
        assert!(receive.list().unwrap().is_empty());
        assert!(!receive.directory(&ticket.id).unwrap().exists());
    }
    #[test]
    fn corrupt_missing_and_expired_checkpoints_are_never_reused() {
        let dir = tempfile::tempdir().unwrap();
        let peer = peer();
        let (catalog, store) = setup(&dir.path().join("receiver"), &peer);
        let manifest = Manifest {
            tree: tree("file", 0),
            hashes: vec![empty_hash()],
            folder: false,
        };
        for scenario in 0..3 {
            let id = uuid::Uuid::new_v4().to_string();
            drop(catalog.accept(&peer, &id, manifest.clone()).unwrap());
            let checkpoint = catalog.directory(&id).unwrap().join("checkpoint.enc");
            match scenario {
                0 => {
                    let mut bytes = fs::read(&checkpoint).unwrap();
                    bytes[20] ^= 1;
                    fs::write(&checkpoint, bytes).unwrap();
                }
                1 => {
                    fs::remove_file(&checkpoint).unwrap();
                }
                _ => {
                    let mut record = catalog.get(&peer, &id).unwrap();
                    record.updated = now().unwrap() - MAX_AGE - 1;
                    let plain = serde_json::to_vec(&record).unwrap();
                    let bytes = lock(&store).seal_resume(&plain).unwrap();
                    fs::write(&checkpoint, bytes).unwrap();
                }
            }
            assert!(catalog.receiver(&peer, &id).is_err());
            assert!(catalog.list().unwrap().is_empty());
            assert!(!catalog.directory(&id).unwrap().exists());
        }
    }
    #[test]
    fn peer_manifest_binding_and_revocation_are_enforced() {
        let dir = tempfile::tempdir().unwrap();
        let peer = peer();
        let other = super::super::identity::Identity::generate()
            .unwrap()
            .fingerprint()
            .unwrap();
        let (catalog, store) = setup(&dir.path().join("receiver"), &peer);
        lock(&store).trust(&other).unwrap();
        let manifest = Manifest {
            tree: tree("file", 0),
            hashes: vec![empty_hash()],
            folder: false,
        };
        let id = uuid::Uuid::new_v4().to_string();
        drop(catalog.accept(&peer, &id, manifest.clone()).unwrap());
        assert!(catalog.get(&other, &id).is_err());
        assert!(catalog.accept(&other, &id, manifest.clone()).is_err());
        let mut changed = manifest.clone();
        changed.tree.name = "different".into();
        changed.tree.entries[0].path = "different".into();
        assert!(catalog.accept(&peer, &id, changed).is_err());
        lock(&store).revoke(&peer).unwrap();
        lock(&store).unblock(&peer).unwrap();
        lock(&store).trust(&peer).unwrap();
        assert!(catalog.receiver(&peer, &id).is_err());
        assert!(catalog.list().unwrap().is_empty());
    }
    #[test]
    fn record_limits_and_persistent_bytes_are_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let peer = peer();
        let (catalog, _) = setup(&dir.path().join("receiver"), &peer);
        let manifest = Manifest {
            tree: tree("file", 0),
            hashes: vec![empty_hash()],
            folder: false,
        };
        for _ in 0..MAX_RECORDS {
            let id = uuid::Uuid::new_v4().to_string();
            drop(catalog.accept(&peer, &id, manifest.clone()).unwrap());
        }
        assert_eq!(catalog.list().unwrap().len(), MAX_RECORDS);
        assert!(matches!(
            catalog.accept(&peer, &uuid::Uuid::new_v4().to_string(), manifest),
            Err(Error::Limit)
        ));
    }
    #[test]
    fn checkpoint_offset_and_hash_must_match_actual_received_data() {
        let dir = tempfile::tempdir().unwrap();
        let peer = peer();
        let (catalog, _) = setup(&dir.path().join("receiver"), &peer);
        let manifest = Manifest {
            tree: tree("file", 3),
            hashes: vec![blake3::hash(b"abc").to_hex().to_string()],
            folder: false,
        };
        let id = uuid::Uuid::new_v4().to_string();
        let mut receiver = catalog.accept(&peer, &id, manifest).unwrap();
        receiver.write(b"abc").unwrap();
        let mut position = receiver.position();
        position.hash = empty_hash();
        assert_eq!(receiver.checkpoint(position), Err(Error::Integrity));
        let position = receiver.position();
        receiver.checkpoint(position.clone()).unwrap();
        assert_eq!(receiver.checkpoint(position), Err(Error::Integrity));
        receiver
            .file_end(0, &blake3::hash(b"abc").to_hex())
            .unwrap();
        drop(receiver);
        assert_eq!(catalog.get(&peer, &id).unwrap().verified(), 3);
    }
    #[test]
    fn publication_journal_recovers_after_rename_and_never_publishes_twice() {
        let dir = tempfile::tempdir().unwrap();
        let peer = peer();
        let (catalog, _) = setup(&dir.path().join("receiver"), &peer);
        let manifest = Manifest {
            tree: tree("file", 3),
            hashes: vec![blake3::hash(b"abc").to_hex().to_string()],
            folder: false,
        };
        let id = uuid::Uuid::new_v4().to_string();
        let mut receiver = catalog.accept(&peer, &id, manifest.clone()).unwrap();
        receiver.write(b"abc").unwrap();
        receiver.file_end(0, &manifest.hashes[0]).unwrap();
        let mut record = receiver.record.clone();
        drop(receiver);
        private_dir(&catalog.inbox).unwrap();
        record.receipt = Some(format!("received-{}", uuid::Uuid::new_v4()));
        let destination = catalog.destination(&record).unwrap();
        fs::create_dir(destination.parent().unwrap()).unwrap();
        catalog.save(&record).unwrap();
        let source = catalog.directory(&id).unwrap().join("payload/file");
        bundle::rename_noreplace(&source, &destination).unwrap();
        assert_eq!(
            catalog.completed(&peer, &id).unwrap(),
            Some(destination.clone())
        );
        assert!(catalog.get(&peer, &id).unwrap().completed);
        assert!(catalog.accept(&peer, &id, manifest).is_err());
        assert_eq!(fs::read(&destination).unwrap(), b"abc");
        assert_eq!(fs::read_dir(&catalog.inbox).unwrap().count(), 1);
    }
}

#[cfg(test)]
mod boundary_tests {
    use super::super::{bundle::Entry, identity::Identity};
    use super::*;
    fn fixture() -> (tempfile::TempDir, Catalog, String, String, Manifest) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("receiver");
        let peer = Identity::generate().unwrap().fingerprint().unwrap();
        let mut store = Store::for_test(&root, &[29; 32]).unwrap();
        store.trust(&peer).unwrap();
        let catalog = Catalog::new(root, Arc::new(Mutex::new(store))).unwrap();
        let manifest = Manifest {
            tree: bundle::Manifest {
                name: "file".into(),
                total: 3,
                entries: vec![Entry {
                    path: "file".into(),
                    kind: Kind::File,
                    size: 3,
                }],
            },
            hashes: vec![blake3::hash(b"abc").to_hex().to_string()],
            folder: false,
        };
        let id = uuid::Uuid::new_v4().to_string();
        (dir, catalog, peer, id, manifest)
    }
    #[test]
    fn cancellation_gate_preserves_verified_checkpoint_before_publication() {
        for before in [true, false] {
            let (_dir, catalog, peer, id, manifest) = fixture();
            let mut receiver = catalog.accept(&peer, &id, manifest.clone()).unwrap();
            receiver.write(b"abc").unwrap();
            receiver.file_end(0, &manifest.hashes[0]).unwrap();
            let gate = Arc::new(CommitGate::default());
            let (at, reached) = std::sync::mpsc::channel();
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
            let task = std::thread::spawn(move || receiver.finish(other, || {}));
            reached
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            let cancelled = gate.cancel();
            release.send(()).unwrap();
            let result = task.join().unwrap();
            if before {
                cancelled.unwrap();
                assert_eq!(result, Err(Error::Cancelled));
                assert!(fs::read_dir(&catalog.inbox).unwrap().next().is_none());
                assert_eq!(catalog.get(&peer, &id).unwrap().verified(), 3);
                assert!(!catalog.get(&peer, &id).unwrap().completed);
                let receiver = catalog.receiver(&peer, &id).unwrap();
                assert!(
                    receiver
                        .finish(Arc::new(CommitGate::default()), || {})
                        .unwrap()
                        .is_file()
                );
            } else {
                assert_eq!(cancelled, Err(Error::CommitStarted));
                assert_eq!(fs::read(result.unwrap()).unwrap(), b"abc");
            }
        }
    }
    #[test]
    fn no_replace_collision_preserves_destination_and_checkpoint() {
        let (_dir, catalog, peer, id, manifest) = fixture();
        let mut receiver = catalog.accept(&peer, &id, manifest.clone()).unwrap();
        receiver.write(b"abc").unwrap();
        receiver.file_end(0, &manifest.hashes[0]).unwrap();
        let gate = Arc::new(CommitGate::default());
        let observer = catalog.clone();
        let peer2 = peer.clone();
        let id2 = id.clone();
        let (destination_tx, destination_rx) = std::sync::mpsc::channel();
        gate.pause_before(move || {
            let record = observer.get(&peer2, &id2).unwrap();
            let destination = observer.destination(&record).unwrap();
            fs::write(&destination, b"existing").unwrap();
            destination_tx.send(destination).unwrap();
        });
        assert_eq!(receiver.finish(gate, || {}), Err(Error::FileIo));
        let destination = destination_rx.recv().unwrap();
        assert_eq!(fs::read(destination).unwrap(), b"existing");
        assert_eq!(catalog.get(&peer, &id).unwrap().verified(), 3);
        assert!(!catalog.get(&peer, &id).unwrap().completed);
    }
    #[test]
    fn total_reservation_limits_need_no_large_allocation() {
        let (_dir, catalog, peer, _, _) = fixture();
        let size = files::MAX_FILE_SIZE;
        let manifest = Manifest {
            tree: bundle::Manifest {
                name: "large".into(),
                total: size,
                entries: vec![Entry {
                    path: "large".into(),
                    kind: Kind::File,
                    size,
                }],
            },
            hashes: vec![empty_hash()],
            folder: false,
        };
        let barrier = Arc::new(std::sync::Barrier::new(4));
        let jobs: Vec<_> = (0..4)
            .map(|_| {
                let barrier = barrier.clone();
                let catalog = catalog.clone();
                let peer = peer.clone();
                let manifest = manifest.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    catalog
                        .accept(&peer, &uuid::Uuid::new_v4().to_string(), manifest)
                        .map(drop)
                })
            })
            .collect();
        let results: Vec<_> = jobs.into_iter().map(|job| job.join().unwrap()).collect();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 2);
        assert!(
            results
                .iter()
                .filter(|result| result.is_err())
                .all(|result| *result == Err(Error::Limit))
        );
        assert_eq!(catalog.list().unwrap().len(), 2);
    }
    #[test]
    fn manifest_hash_parser_is_bounded_during_deserialization() {
        let (_dir, _, _, _, manifest) = fixture();
        let mut value = serde_json::to_value(manifest).unwrap();
        value["hashes"] = serde_json::json!(vec![""; bundle::MAX_ENTRIES + 1]);
        assert!(serde_json::from_value::<Manifest>(value).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn symlink_sources_and_staging_files_are_rejected() {
        use std::os::unix::fs::symlink;
        let (dir, catalog, peer, id, manifest) = fixture();
        let source = dir.path().join("source");
        fs::write(&source, b"abc").unwrap();
        let link = dir.path().join("link");
        symlink(&source, &link).unwrap();
        assert!(
            catalog
                .prepare(&peer, &manifest.tree, vec![link], false)
                .is_err()
        );
        let mut receiver = catalog.accept(&peer, &id, manifest).unwrap();
        receiver.write(b"abc").unwrap();
        let path = catalog.directory(&id).unwrap().join("payload/file");
        drop(receiver);
        fs::remove_file(&path).unwrap();
        symlink(&source, &path).unwrap();
        assert!(catalog.receiver(&peer, &id).is_err());
        assert_eq!(fs::read(source).unwrap(), b"abc");

        // A folder containing only empty directories still has to reject a
        // changed ancestor link; no file open would otherwise detect it.
        let folder = dir.path().join("Folder");
        fs::create_dir_all(folder.join("empty")).unwrap();
        let tree = bundle::Manifest {
            name: "Folder".into(),
            total: 0,
            entries: vec![Entry {
                path: "empty".into(),
                kind: Kind::Directory,
                size: 0,
            }],
        };
        let ticket = catalog
            .prepare(&peer, &tree, vec![folder.join("empty")], true)
            .unwrap();
        let old = dir.path().join("Folder-old");
        fs::rename(&folder, &old).unwrap();
        symlink(&old, &folder).unwrap();
        assert!(catalog.sender(&peer, &ticket.id, ticket.position).is_err());
    }
}

#[cfg(test)]
mod malformed_data_tests {
    use super::super::{bundle::Entry, identity::Identity};
    use super::*;
    #[test]
    fn eof_checkpoint_cannot_certify_data_that_disagrees_with_original_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let peer = Identity::generate().unwrap().fingerprint().unwrap();
        let mut store = Store::for_test(dir.path(), &[41; 32]).unwrap();
        store.trust(&peer).unwrap();
        let catalog = Catalog::new(dir.path().to_owned(), Arc::new(Mutex::new(store))).unwrap();
        let manifest = Manifest {
            tree: bundle::Manifest {
                name: "data".into(),
                total: 3,
                entries: vec![Entry {
                    path: "data".into(),
                    kind: Kind::File,
                    size: 3,
                }],
            },
            hashes: vec![blake3::hash(b"abc").to_hex().to_string()],
            folder: false,
        };
        let id = uuid::Uuid::new_v4().to_string();
        let mut receiver = catalog.accept(&peer, &id, manifest).unwrap();
        receiver.write(b"bad").unwrap();
        let position = receiver.position();
        assert_eq!(receiver.checkpoint(position), Err(Error::Integrity));
        assert_eq!(receiver.record.verified(), 0);
        assert_eq!(
            receiver.file_end(u32::MAX, &blake3::hash(b"bad").to_hex()),
            Err(Error::Integrity)
        );
        assert!(
            receiver
                .finish(Arc::new(CommitGate::default()), || {})
                .is_err()
        );
        assert!(!catalog.inbox.exists());
        assert_eq!(catalog.get(&peer, &id).unwrap().verified(), 0);
    }
}

#[cfg(test)]
mod resource_failure_tests {
    use super::super::{bundle::Entry, identity::Identity};
    use super::*;
    fn receiver(root: &Path, size: u64) -> (Catalog, String, String, Receiver) {
        let peer = Identity::generate().unwrap().fingerprint().unwrap();
        let mut store = Store::for_test(root, &[43; 32]).unwrap();
        store.trust(&peer).unwrap();
        let catalog = Catalog::new(root.to_owned(), Arc::new(Mutex::new(store))).unwrap();
        let manifest = Manifest {
            tree: bundle::Manifest {
                name: "data".into(),
                total: size,
                entries: vec![Entry {
                    path: "data".into(),
                    kind: Kind::File,
                    size,
                }],
            },
            hashes: vec![blake3::hash(b"expected").to_hex().to_string()],
            folder: false,
        };
        let id = uuid::Uuid::new_v4().to_string();
        let receiver = catalog.accept(&peer, &id, manifest).unwrap();
        (catalog, peer, id, receiver)
    }
    #[test]
    fn uncheckpointed_disk_tail_is_bounded_and_removed_on_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let (catalog, peer, id, mut receiver) =
            receiver(dir.path(), CHECKPOINT_BYTES + CHUNK_SIZE as u64);
        let block = [9; CHUNK_SIZE];
        for _ in 0..CHECKPOINT_BYTES / CHUNK_SIZE as u64 {
            receiver.write(&block).unwrap();
        }
        assert_eq!(receiver.write(&block), Err(Error::Limit));
        assert_eq!(receiver.record.verified(), 0);
        let path = catalog.directory(&id).unwrap().join("payload/data");
        assert_eq!(fs::metadata(&path).unwrap().len(), CHECKPOINT_BYTES);
        drop(receiver);
        let reopened = catalog.receiver(&peer, &id).unwrap();
        assert_eq!(reopened.record.verified(), 0);
        assert!(!path.exists());
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn enospc_during_payload_write_does_not_advance_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let (catalog, peer, id, mut receiver) = receiver(dir.path(), 3);
        receiver.open().unwrap();
        receiver.current.as_mut().unwrap().file =
            OpenOptions::new().write(true).open("/dev/full").unwrap();
        assert_eq!(receiver.write(b"abc"), Err(Error::FileIo));
        assert_eq!(receiver.position().offset, 0);
        assert_eq!(receiver.record.verified(), 0);
        drop(receiver);
        let reopened = catalog.receiver(&peer, &id).unwrap();
        assert_eq!(reopened.position().offset, 0);
        assert_eq!(reopened.record.verified(), 0);
    }
}

#[cfg(test)]
mod crashed_metadata_tests {
    use super::super::{bundle::Entry, identity::Identity};
    use super::*;
    #[test]
    fn restart_removes_abandoned_atomic_metadata_temps_without_touching_payload() {
        let dir = tempfile::tempdir().unwrap();
        let peer = Identity::generate().unwrap().fingerprint().unwrap();
        let mut store = Store::for_test(dir.path(), &[47; 32]).unwrap();
        store.trust(&peer).unwrap();
        let store = Arc::new(Mutex::new(store));
        let catalog = Catalog::new(dir.path().to_owned(), store.clone()).unwrap();
        let manifest = Manifest {
            tree: bundle::Manifest {
                name: ".tmp-user-file".into(),
                total: 3,
                entries: vec![Entry {
                    path: ".tmp-user-file".into(),
                    kind: Kind::File,
                    size: 3,
                }],
            },
            hashes: vec![blake3::hash(b"abc").to_hex().to_string()],
            folder: false,
        };
        let id = uuid::Uuid::new_v4().to_string();
        let mut receiver = catalog.accept(&peer, &id, manifest).unwrap();
        receiver.write(b"abc").unwrap();
        let temporary = catalog
            .directory(&id)
            .unwrap()
            .join(".tmp-crashed-checkpoint");
        fs::write(&temporary, b"incomplete ciphertext").unwrap();
        let payload = catalog
            .directory(&id)
            .unwrap()
            .join("payload/.tmp-user-file");
        // Refresh while an IO lease is live must not delete its temporary file.
        catalog.list().unwrap();
        assert!(temporary.exists());
        drop(receiver);
        let restarted = Catalog::new(dir.path().to_owned(), store).unwrap();
        assert!(!temporary.exists());
        assert_eq!(fs::read(&payload).unwrap(), b"abc");
        assert_eq!(restarted.list().unwrap().len(), 1);
        assert_eq!(restarted.get(&peer, &id).unwrap().verified(), 0);
    }
}

#[cfg(test)]
mod cooperative_cancel_tests {
    use super::super::{bundle::Entry, identity::Identity};
    use super::*;
    fn interrupt_after_one_chunk(gate: Arc<CommitGate>) {
        HASH_CHUNK_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                gate.cancel().unwrap();
            }))
        });
    }
    #[test]
    fn cancellation_interrupts_source_and_checkpoint_rehash_without_advancing_progress() {
        let dir = tempfile::tempdir().unwrap();
        let peer = Identity::generate().unwrap().fingerprint().unwrap();
        let mut store = Store::for_test(dir.path(), &[53; 32]).unwrap();
        store.trust(&peer).unwrap();
        let catalog = Catalog::new(dir.path().to_owned(), Arc::new(Mutex::new(store))).unwrap();
        let path = dir.path().join("source");
        let size = CHECKPOINT_BYTES + 7;
        let file = File::create(&path).unwrap();
        file.set_len(size).unwrap();
        drop(file);
        let tree = bundle::Manifest {
            name: "source".into(),
            total: size,
            entries: vec![Entry {
                path: "source".into(),
                kind: Kind::File,
                size,
            }],
        };
        let ticket = catalog.prepare(&peer, &tree, vec![path], false).unwrap();
        let gate = Arc::new(CommitGate::default());
        interrupt_after_one_chunk(gate.clone());
        assert!(matches!(
            catalog.sender_cancellable(&peer, &ticket.id, ticket.position.clone(), gate),
            Err(Error::Cancelled)
        ));
        assert_eq!(catalog.get(&peer, &ticket.id).unwrap().verified(), 0);
        // A cancelled worker releases its lease, so another verified attempt is possible.
        let mut sender = catalog.sender(&peer, &ticket.id, ticket.position).unwrap();
        let incoming_id = uuid::Uuid::new_v4().to_string();
        let mut receiver = catalog
            .accept(&peer, &incoming_id, ticket.manifest.clone())
            .unwrap();
        loop {
            match sender.next().unwrap() {
                Event::Data(data) => receiver.write(&data).unwrap(),
                Event::Checkpoint(position) => {
                    receiver.checkpoint(position).unwrap();
                    break;
                }
                _ => panic!("Expected checkpoint"),
            }
        }
        let saved = receiver.position();
        drop(receiver);
        drop(sender);
        let gate = Arc::new(CommitGate::default());
        interrupt_after_one_chunk(gate.clone());
        assert!(matches!(
            catalog.accept_cancellable(&peer, &incoming_id, ticket.manifest, gate),
            Err(Error::Cancelled)
        ));
        assert_eq!(catalog.get(&peer, &incoming_id).unwrap().position, saved);
        let receiver = catalog.receiver(&peer, &incoming_id).unwrap();
        assert_eq!(receiver.verified(), CHECKPOINT_BYTES);
    }
    #[test]
    fn cancellation_during_final_integrity_rehash_never_starts_publication() {
        let dir = tempfile::tempdir().unwrap();
        let peer = Identity::generate().unwrap().fingerprint().unwrap();
        let mut store = Store::for_test(dir.path(), &[59; 32]).unwrap();
        store.trust(&peer).unwrap();
        let catalog = Catalog::new(dir.path().to_owned(), Arc::new(Mutex::new(store))).unwrap();
        let bytes = vec![7; CHUNK_SIZE * 2 + 3];
        let size = bytes.len() as u64;
        let manifest = Manifest {
            tree: bundle::Manifest {
                name: "file".into(),
                total: size,
                entries: vec![Entry {
                    path: "file".into(),
                    kind: Kind::File,
                    size,
                }],
            },
            hashes: vec![blake3::hash(&bytes).to_hex().to_string()],
            folder: false,
        };
        let id = uuid::Uuid::new_v4().to_string();
        let mut receiver = catalog.accept(&peer, &id, manifest.clone()).unwrap();
        for chunk in bytes.chunks(CHUNK_SIZE) {
            receiver.write(chunk).unwrap();
        }
        receiver.file_end(0, &manifest.hashes[0]).unwrap();
        let gate = Arc::new(CommitGate::default());
        interrupt_after_one_chunk(gate.clone());
        let committed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let called = committed.clone();
        assert_eq!(
            receiver.finish(gate, move || called
                .store(true, std::sync::atomic::Ordering::SeqCst)),
            Err(Error::Cancelled)
        );
        assert!(!committed.load(std::sync::atomic::Ordering::SeqCst));
        assert!(!catalog.inbox.exists());
        assert_eq!(catalog.get(&peer, &id).unwrap().verified(), size);
        assert!(!catalog.get(&peer, &id).unwrap().completed);
    }
}
