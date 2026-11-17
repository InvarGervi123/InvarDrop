//! The existing profile master key, wrapped by a non-exportable Android Keystore
//! AES key. Only authenticated ciphertext is written to the profile directory.
use super::{
    error::{Error, Result},
    identity::random,
};
#[cfg(unix)]
use std::fs::File;
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::Path,
};
use zeroize::Zeroizing;

const MAGIC: &[u8; 8] = b"IVDMKEY1";
const IV_LEN: usize = 12;
const ENVELOPE_LEN: usize = MAGIC.len() + IV_LEN + 32 + 16;
const FILE_NAME: &str = "master.android.enc";

trait ProtectedKey {
    fn exists(&self) -> Result<bool>;
    fn create(&self) -> Result<()>;
    /// Return the randomly generated IV followed by AES-GCM ciphertext/tag.
    fn encrypt(&self, master: &[u8], aad: &[u8]) -> Result<Vec<u8>>;
    fn decrypt(&self, sealed: &[u8], aad: &[u8]) -> Result<Zeroizing<Vec<u8>>>;
}

#[cfg(target_os = "android")]
#[path = "android_keystore.rs"]
mod native;

#[cfg(target_os = "android")]
pub(super) fn open(
    root: &Path,
    account: &str,
    identity_exists: bool,
) -> Result<Zeroizing<Vec<u8>>> {
    let key = native::Keystore::new(account);
    load(root, account, identity_exists, &key)
}

// Called while Store holds its lifetime profile lock, including key creation.
fn load(
    root: &Path,
    account: &str,
    identity_exists: bool,
    key: &impl ProtectedKey,
) -> Result<Zeroizing<Vec<u8>>> {
    let path = root.join(FILE_NAME);
    let wrapped_exists = match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => true,
        Ok(_) => return Err(Error::Corrupt),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(_) => return Err(Error::Storage),
    };
    let key_exists = key.exists()?;
    let fresh = classify(identity_exists, wrapped_exists, key_exists)?;
    let mut aad = b"io.github.invargervi.invardrop.identity-v1:".to_vec();
    aad.extend_from_slice(account.as_bytes());
    aad.extend_from_slice(MAGIC);
    if fresh {
        key.create()?;
        let master = Zeroizing::new(random::<32>()?.to_vec());
        let sealed = key.encrypt(&master, &aad)?;
        if sealed.len() + MAGIC.len() != ENVELOPE_LEN {
            return Err(Error::Storage);
        }
        let mut file = tempfile::NamedTempFile::new_in(root).map_err(|_| Error::Storage)?;
        file.write_all(MAGIC)
            .and_then(|_| file.write_all(&sealed))
            .and_then(|_| file.as_file().sync_all())
            .map_err(|_| Error::Storage)?;
        // Never replace a file that appeared concurrently, even on first use.
        file.persist_noclobber(&path).map_err(|_| Error::Storage)?;
        #[cfg(unix)]
        File::open(root)
            .and_then(|dir| dir.sync_all())
            .map_err(|_| Error::Storage)?;
        return Ok(master);
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(target_os = "android")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut sealed = Vec::new();
    options
        .open(path)
        .map_err(|_| Error::Storage)?
        .take(ENVELOPE_LEN as u64 + 1)
        .read_to_end(&mut sealed)
        .map_err(|_| Error::Storage)?;
    if sealed.len() != ENVELOPE_LEN || !sealed.starts_with(MAGIC) {
        return Err(Error::Corrupt);
    }
    let master = key.decrypt(&sealed[MAGIC.len()..], &aad)?;
    if master.len() != 32 {
        return Err(Error::Corrupt);
    }
    Ok(master)
}

// Existing identity, wrapping key and envelope must agree. Incomplete bootstrap,
// restored backups without their Keystore key and orphaned credentials fail closed.
fn classify(identity: bool, wrapped: bool, key: bool) -> Result<bool> {
    match (identity, wrapped, key) {
        (false, false, false) => Ok(true),
        (true, true, true) => Ok(false),
        _ => Err(Error::Corrupt),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::aead;
    use std::cell::Cell;

    struct TestKey {
        present: Cell<bool>,
        creates: Cell<usize>,
        bytes: [u8; 32],
    }
    impl TestKey {
        fn new() -> Self {
            Self {
                present: Cell::new(false),
                creates: Cell::new(0),
                bytes: random().unwrap(),
            }
        }
        fn cipher(&self) -> aead::LessSafeKey {
            aead::LessSafeKey::new(aead::UnboundKey::new(&aead::AES_256_GCM, &self.bytes).unwrap())
        }
    }
    impl ProtectedKey for TestKey {
        fn exists(&self) -> Result<bool> {
            Ok(self.present.get())
        }
        fn create(&self) -> Result<()> {
            assert!(!self.present.replace(true));
            self.creates.set(self.creates.get() + 1);
            Ok(())
        }
        fn encrypt(&self, master: &[u8], aad: &[u8]) -> Result<Vec<u8>> {
            let iv = random::<12>()?;
            let mut ciphertext = master.to_vec();
            self.cipher()
                .seal_in_place_append_tag(
                    aead::Nonce::assume_unique_for_key(iv),
                    aead::Aad::from(aad),
                    &mut ciphertext,
                )
                .unwrap();
            Ok([iv.as_slice(), ciphertext.as_slice()].concat())
        }
        fn decrypt(&self, sealed: &[u8], aad: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
            let iv: [u8; 12] = sealed[..12].try_into().unwrap();
            let mut ciphertext = Zeroizing::new(sealed[12..].to_vec());
            let plain = self
                .cipher()
                .open_in_place(
                    aead::Nonce::assume_unique_for_key(iv),
                    aead::Aad::from(aad),
                    &mut ciphertext,
                )
                .map_err(|_| Error::Corrupt)?;
            Ok(Zeroizing::new(plain.to_vec()))
        }
    }
    #[test]
    fn only_fresh_or_complete_state_is_accepted() {
        for identity in [false, true] {
            for wrapped in [false, true] {
                for key in [false, true] {
                    assert_eq!(
                        classify(identity, wrapped, key).is_ok(),
                        identity == wrapped && wrapped == key
                    );
                }
            }
        }
    }
    #[test]
    fn master_reopens_and_preserves_identity_trust_and_resume_keys() {
        let root = tempfile::tempdir().unwrap();
        let key = TestKey::new();
        let master = load(root.path(), "profile", false, &key).unwrap();
        let mut store = super::super::store::Store::for_test(root.path(), &master).unwrap();
        let identity = store.identity().fingerprint().unwrap();
        let peer = super::super::identity::Identity::generate()
            .unwrap()
            .fingerprint()
            .unwrap();
        store.trust(&peer).unwrap();
        let checkpoint = store.seal_resume(b"verified recovery").unwrap();
        drop(store);
        let again = load(root.path(), "profile", true, &key).unwrap();
        assert!(master.as_slice() == again.as_slice());
        assert_eq!(key.creates.get(), 1);
        let store = super::super::store::Store::for_test(root.path(), &again).unwrap();
        assert_eq!(store.identity().fingerprint().unwrap(), identity);
        assert!(store.is_trusted(&peer));
        assert_eq!(
            store.open_resume(&checkpoint).unwrap(),
            b"verified recovery"
        );
        let sealed = fs::read(root.path().join(FILE_NAME)).unwrap();
        assert_eq!(sealed.len(), ENVELOPE_LEN);
        assert!(!sealed.windows(32).any(|b| b == master.as_slice()));
    }
    #[test]
    fn damaged_or_rebound_envelope_never_rotates_keys() {
        let root = tempfile::tempdir().unwrap();
        let key = TestKey::new();
        load(root.path(), "profile", false, &key).unwrap();
        let path = root.path().join(FILE_NAME);
        let sealed = fs::read(&path).unwrap();
        assert!(matches!(
            load(root.path(), "another profile", true, &key),
            Err(Error::Corrupt)
        ));
        for bytes in [vec![], vec![0; ENVELOPE_LEN + 1], {
            let mut b = sealed.clone();
            b[ENVELOPE_LEN - 1] ^= 1;
            b
        }] {
            fs::write(&path, bytes).unwrap();
            assert!(matches!(
                load(root.path(), "profile", true, &key),
                Err(Error::Corrupt)
            ));
        }
        assert_eq!(key.creates.get(), 1);
    }
    #[test]
    fn missing_key_or_file_and_orphaned_bootstrap_fail_closed() {
        let root = tempfile::tempdir().unwrap();
        let key = TestKey::new();
        load(root.path(), "profile", false, &key).unwrap();
        assert!(matches!(
            load(root.path(), "profile", false, &key),
            Err(Error::Corrupt)
        ));
        key.present.set(false);
        assert!(matches!(
            load(root.path(), "profile", true, &key),
            Err(Error::Corrupt)
        ));
        key.present.set(true);
        fs::remove_file(root.path().join(FILE_NAME)).unwrap();
        assert!(matches!(
            load(root.path(), "profile", true, &key),
            Err(Error::Corrupt)
        ));
        assert_eq!(key.creates.get(), 1);
    }
    #[test]
    fn unavailable_keystore_does_not_create_profile_material() {
        struct Unavailable;
        impl ProtectedKey for Unavailable {
            fn exists(&self) -> Result<bool> {
                Err(Error::Storage)
            }
            fn create(&self) -> Result<()> {
                panic!("Unavailable backend must not create a key")
            }
            fn encrypt(&self, _: &[u8], _: &[u8]) -> Result<Vec<u8>> {
                unreachable!()
            }
            fn decrypt(&self, _: &[u8], _: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
                unreachable!()
            }
        }
        let root = tempfile::tempdir().unwrap();
        assert!(matches!(
            load(root.path(), "profile", false, &Unavailable),
            Err(Error::Storage)
        ));
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
    }
    #[test]
    fn concurrent_envelope_publication_is_never_overwritten() {
        struct Collision<'a> {
            key: &'a TestKey,
            root: &'a Path,
        }
        impl ProtectedKey for Collision<'_> {
            fn exists(&self) -> Result<bool> {
                self.key.exists()
            }
            fn create(&self) -> Result<()> {
                self.key.create()
            }
            fn encrypt(&self, bytes: &[u8], aad: &[u8]) -> Result<Vec<u8>> {
                let sealed = self.key.encrypt(bytes, aad)?;
                fs::write(self.root.join(FILE_NAME), b"existing data").unwrap();
                Ok(sealed)
            }
            fn decrypt(&self, bytes: &[u8], aad: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
                self.key.decrypt(bytes, aad)
            }
        }
        let root = tempfile::tempdir().unwrap();
        let key = TestKey::new();
        assert!(matches!(
            load(
                root.path(),
                "profile",
                false,
                &Collision {
                    key: &key,
                    root: root.path()
                }
            ),
            Err(Error::Storage)
        ));
        assert_eq!(
            fs::read(root.path().join(FILE_NAME)).unwrap(),
            b"existing data"
        );
        assert!(matches!(
            load(root.path(), "profile", false, &key),
            Err(Error::Corrupt)
        ));
        assert_eq!(key.creates.get(), 1);
    }
    #[cfg(unix)]
    #[test]
    fn symlink_and_directory_envelopes_are_rejected() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let key = TestKey::new();
        let outside = root.path().join("outside");
        fs::write(&outside, b"untouched").unwrap();
        let path = root.path().join(FILE_NAME);
        symlink(&outside, &path).unwrap();
        assert!(matches!(
            load(root.path(), "profile", false, &key),
            Err(Error::Corrupt)
        ));
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert!(matches!(
            load(root.path(), "profile", false, &key),
            Err(Error::Corrupt)
        ));
        assert_eq!(fs::read(outside).unwrap(), b"untouched");
        assert_eq!(key.creates.get(), 0);
    }
}
