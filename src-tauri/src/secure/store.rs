use super::{
    error::{Error, Result},
    identity::{Identity, hex, random, valid_fingerprint},
};
use ring::{aead, digest};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};
use zeroize::Zeroizing;

const MAGIC: &[u8] = b"IVDROP01";
const MAX_FILE: u64 = 64 * 1024;
const MAX_TRUSTED: usize = 64;
const MAX_REVOKED: usize = 256;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    identity: Identity,
    trusted: BTreeSet<String>,
    revoked: BTreeSet<String>,
}
/// Master key exists only in the OS credential store; disk state is authenticated ciphertext.
/// The lifetime lock prevents concurrent processes from silently overwriting trust changes.
pub struct Store {
    path: PathBuf,
    key: aead::LessSafeKey,
    record: Record,
    _lock: File,
}
impl Store {
    pub fn open(root: &Path) -> Result<Self> {
        if !root.is_absolute() {
            return Err(Error::Storage);
        }
        let (path, lock) = prepare(root)?;
        let root = root.canonicalize().map_err(|_| Error::Storage)?;
        let account =
            hex(digest::digest(&digest::SHA256, root.to_string_lossy().as_bytes()).as_ref());
        let entry = keyring::v1::Entry::new("io.github.invargervi.invardrop.identity-v1", &account)
            .map_err(|_| Error::Storage)?;
        let exists = path.try_exists().map_err(|_| Error::Storage)?;
        let master = match entry.get_secret() {
            Ok(bytes) if bytes.len() == 32 && exists => Zeroizing::new(bytes),
            Ok(_) => return Err(Error::Corrupt),
            Err(keyring::v1::Error::NoEntry) if !exists => {
                let bytes = Zeroizing::new(random::<32>()?.to_vec());
                entry.set_secret(&bytes).map_err(|_| Error::Storage)?;
                bytes
            }
            Err(keyring::v1::Error::NoEntry) => return Err(Error::Corrupt),
            Err(_) => return Err(Error::Storage),
        };
        Self::load(path, lock, &master, exists)
    }
    fn load(path: PathBuf, lock: File, master: &[u8], exists: bool) -> Result<Self> {
        let key = aead::LessSafeKey::new(
            aead::UnboundKey::new(&aead::CHACHA20_POLY1305, master).map_err(|_| Error::Corrupt)?,
        );
        let record = if exists {
            let mut bytes = Zeroizing::new(Vec::new());
            File::open(&path)
                .map_err(|_| Error::Storage)?
                .take(MAX_FILE + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| Error::Storage)?;
            if bytes.len() as u64 > MAX_FILE
                || bytes.len() < MAGIC.len() + 12 + 16
                || !bytes.starts_with(MAGIC)
            {
                return Err(Error::Corrupt);
            }
            let mut nonce = [0; 12];
            nonce.copy_from_slice(&bytes[MAGIC.len()..MAGIC.len() + 12]);
            let plaintext = key
                .open_in_place(
                    aead::Nonce::assume_unique_for_key(nonce),
                    aead::Aad::from(MAGIC),
                    &mut bytes[MAGIC.len() + 12..],
                )
                .map_err(|_| Error::Corrupt)?;
            serde_json::from_slice::<Record>(plaintext).map_err(|_| Error::Corrupt)?
        } else {
            Record {
                version: 1,
                identity: Identity::generate()?,
                trusted: BTreeSet::new(),
                revoked: BTreeSet::new(),
            }
        };
        if record.version != 1
            || record.trusted.len() > MAX_TRUSTED
            || record.revoked.len() > MAX_REVOKED
            || record
                .trusted
                .union(&record.revoked)
                .any(|id| !valid_fingerprint(id))
            || !record.trusted.is_disjoint(&record.revoked)
        {
            return Err(Error::Corrupt);
        }
        record.identity.validate()?;
        let store = Self {
            path,
            key,
            record,
            _lock: lock,
        };
        if !exists {
            store.save(&store.record.trusted, &store.record.revoked)?;
        }
        Ok(store)
    }
    pub fn identity(&self) -> &Identity {
        &self.record.identity
    }
    pub fn trusted(&self) -> Vec<String> {
        self.record.trusted.iter().cloned().collect()
    }
    pub fn revoked(&self) -> Vec<String> {
        self.record.revoked.iter().cloned().collect()
    }
    pub fn is_trusted(&self, id: &str) -> bool {
        self.record.trusted.contains(id) && !self.is_revoked(id)
    }
    pub fn is_revoked(&self, id: &str) -> bool {
        self.record.revoked.contains(id)
    }
    pub fn trust(&mut self, id: &str) -> Result<()> {
        if !valid_fingerprint(id) {
            return Err(Error::Identity);
        }
        if self.is_revoked(id) {
            return Err(Error::Revoked);
        }
        if id == self.identity().fingerprint()? {
            return Err(Error::Identity);
        }
        let mut trusted = self.record.trusted.clone();
        trusted.insert(id.into());
        if trusted.len() > MAX_TRUSTED {
            return Err(Error::Limit);
        }
        self.save(&trusted, &self.record.revoked)?;
        self.record.trusted = trusted;
        Ok(())
    }
    pub fn revoke(&mut self, id: &str) -> Result<()> {
        if !valid_fingerprint(id) {
            return Err(Error::Identity);
        }
        if self.record.revoked.len() >= MAX_REVOKED && !self.is_revoked(id) {
            return Err(Error::Limit);
        }
        self.record.trusted.remove(id);
        self.record.revoked.insert(id.into());
        // On write failure remain revoked in memory and return an explicit persistence error.
        self.save(&self.record.trusted, &self.record.revoked)
    }
    pub fn unblock(&mut self, id: &str) -> Result<()> {
        if !valid_fingerprint(id) {
            return Err(Error::Identity);
        }
        let mut revoked = self.record.revoked.clone();
        revoked.remove(id);
        self.save(&self.record.trusted, &revoked)?;
        self.record.revoked = revoked;
        Ok(())
    }
    fn save(&self, trusted: &BTreeSet<String>, revoked: &BTreeSet<String>) -> Result<()> {
        #[derive(Serialize)]
        struct View<'a> {
            version: u32,
            identity: &'a Identity,
            trusted: &'a BTreeSet<String>,
            revoked: &'a BTreeSet<String>,
        }
        let mut bytes = Zeroizing::new(
            serde_json::to_vec(&View {
                version: 1,
                identity: self.identity(),
                trusted,
                revoked,
            })
            .map_err(|_| Error::Storage)?,
        );
        let nonce = random::<12>()?;
        self.key
            .seal_in_place_append_tag(
                aead::Nonce::assume_unique_for_key(nonce),
                aead::Aad::from(MAGIC),
                &mut *bytes,
            )
            .map_err(|_| Error::Storage)?;
        if bytes.len() as u64 + 20 > MAX_FILE {
            return Err(Error::Limit);
        }
        let parent = self.path.parent().ok_or(Error::Storage)?;
        let mut tmp = tempfile::NamedTempFile::new_in(parent).map_err(|_| Error::Storage)?;
        tmp.write_all(MAGIC)
            .and_then(|_| tmp.write_all(&nonce))
            .and_then(|_| tmp.write_all(&bytes))
            .and_then(|_| tmp.as_file().sync_all())
            .map_err(|_| Error::Storage)?;
        tmp.persist(&self.path).map_err(|_| Error::Storage)?;
        #[cfg(unix)]
        File::open(parent)
            .and_then(|f| f.sync_all())
            .map_err(|_| Error::Storage)?;
        Ok(())
    }
    #[cfg(test)]
    pub(super) fn for_test(root: &Path, key: &[u8]) -> Result<Self> {
        let (path, lock) = prepare(root)?;
        let exists = path.exists();
        Self::load(path, lock, key, exists)
    }
}
fn prepare(root: &Path) -> Result<(PathBuf, File)> {
    fs::create_dir_all(root).map_err(|_| Error::Storage)?;
    if fs::symlink_metadata(root)
        .map_err(|_| Error::Storage)?
        .file_type()
        .is_symlink()
    {
        return Err(Error::Storage);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(root, fs::Permissions::from_mode(0o700)).map_err(|_| Error::Storage)?;
    }
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join("profile.lock"))
        .map_err(|_| Error::Storage)?;
    lock.try_lock().map_err(|_| Error::Busy)?;
    Ok((root.join("identity.enc"), lock))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn encrypted_identity_and_trust_survive_reopen_and_revoke() {
        let dir = tempfile::tempdir().unwrap();
        let master = random::<32>().unwrap();
        let peer = Identity::generate().unwrap().fingerprint().unwrap();
        let mut store = Store::for_test(dir.path(), &master).unwrap();
        let id = store.identity().fingerprint().unwrap();
        store.trust(&peer).unwrap();
        drop(store);
        let mut store = Store::for_test(dir.path(), &master).unwrap();
        assert!(store.identity().fingerprint().unwrap() == id);
        assert!(store.is_trusted(&peer));
        store.revoke(&peer).unwrap();
        drop(store);
        let mut store = Store::for_test(dir.path(), &master).unwrap();
        assert!(!store.is_trusted(&peer));
        assert!(store.is_revoked(&peer));
        assert!(matches!(store.trust(&peer), Err(Error::Revoked)));
        store.unblock(&peer).unwrap();
        assert!(!store.is_trusted(&peer));
        let bytes = fs::read(dir.path().join("identity.enc")).unwrap();
        assert!(!bytes.windows(11).any(|w| w == b"private_key"));
    }
    #[test]
    fn corruption_wrong_master_and_duplicate_profile_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let master = random::<32>().unwrap();
        let store = Store::for_test(dir.path(), &master).unwrap();
        assert!(matches!(
            Store::for_test(dir.path(), &master),
            Err(Error::Busy)
        ));
        drop(store);
        assert!(matches!(
            Store::for_test(dir.path(), &[0; 32]),
            Err(Error::Corrupt)
        ));
        let path = dir.path().join("identity.enc");
        let mut bytes = fs::read(&path).unwrap();
        let n = bytes.len() - 1;
        bytes[n] ^= 1;
        fs::write(path, bytes).unwrap();
        assert!(matches!(
            Store::for_test(dir.path(), &master),
            Err(Error::Corrupt)
        ));
    }
    #[test]
    fn failed_trust_write_does_not_authorize_and_failed_revocation_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let master = random::<32>().unwrap();
        let mut store = Store::for_test(dir.path(), &master).unwrap();
        let peer = Identity::generate().unwrap().fingerprint().unwrap();
        let path = store.path.clone();
        store.path = dir.path().join("missing").join("identity.enc");
        assert!(store.trust(&peer).is_err());
        assert!(!store.is_trusted(&peer));
        store.path = path.clone();
        store.trust(&peer).unwrap();
        store.path = dir.path().join("missing").join("identity.enc");
        assert!(store.revoke(&peer).is_err());
        assert!(!store.is_trusted(&peer));
        assert!(store.is_revoked(&peer));
        assert!(store.unblock(&peer).is_err());
        assert!(store.is_revoked(&peer));
        assert!(matches!(store.trust(&peer), Err(Error::Revoked)));
        drop(store);
        // A reported write failure must not be mistaken for a durable revocation.
        let mut store = Store::for_test(dir.path(), &master).unwrap();
        assert!(store.is_trusted(&peer));
        store.revoke(&peer).unwrap();
        store.unblock(&peer).unwrap();
        drop(store);
        let store = Store::for_test(dir.path(), &master).unwrap();
        assert!(!store.is_trusted(&peer));
        assert!(!store.is_revoked(&peer));
    }
    #[test]
    fn repeated_saves_use_fresh_nonces_and_preserve_identity() {
        let dir = tempfile::tempdir().unwrap();
        let master = random::<32>().unwrap();
        let peer = Identity::generate().unwrap().fingerprint().unwrap();
        let mut store = Store::for_test(dir.path(), &master).unwrap();
        let identity = store.identity().fingerprint().unwrap();
        store.trust(&peer).unwrap();
        let before = fs::read(&store.path).unwrap();
        store.trust(&peer).unwrap();
        let after = fs::read(&store.path).unwrap();
        assert!(before[MAGIC.len()..MAGIC.len() + 12] != after[MAGIC.len()..MAGIC.len() + 12]);
        assert!(before[MAGIC.len() + 12..] != after[MAGIC.len() + 12..]);
        drop(store);
        let store = Store::for_test(dir.path(), &master).unwrap();
        assert!(store.identity().fingerprint().unwrap() == identity);
        assert!(store.is_trusted(&peer));
    }
    #[test]
    fn truncated_oversized_and_unknown_header_files_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let master = random::<32>().unwrap();
        let store = Store::for_test(dir.path(), &master).unwrap();
        let path = store.path.clone();
        let original = fs::read(&path).unwrap();
        drop(store);
        for length in [0, MAGIC.len(), MAGIC.len() + 12, original.len() - 1] {
            fs::write(&path, &original[..length]).unwrap();
            assert!(matches!(
                Store::for_test(dir.path(), &master),
                Err(Error::Corrupt)
            ));
        }
        fs::write(&path, vec![0; MAX_FILE as usize + 1]).unwrap();
        assert!(matches!(
            Store::for_test(dir.path(), &master),
            Err(Error::Corrupt)
        ));
        let mut bad_header = original;
        bad_header[0] ^= 1;
        fs::write(&path, bad_header).unwrap();
        assert!(matches!(
            Store::for_test(dir.path(), &master),
            Err(Error::Corrupt)
        ));
    }
    #[cfg(unix)]
    #[test]
    fn unix_profile_and_ciphertext_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();
        let store = Store::for_test(dir.path(), &random::<32>().unwrap()).unwrap();
        assert_eq!(
            fs::metadata(dir.path()).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&store.path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
