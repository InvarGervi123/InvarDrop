use super::*;
fn single(name: &str, bytes: &[u8]) -> Manifest {
    Manifest {
        tree: bundle::Manifest {
            name: name.into(),
            total: bytes.len() as u64,
            entries: vec![bundle::Entry {
                path: name.into(),
                kind: bundle::Kind::File,
                size: bytes.len() as u64,
            }],
        },
        hashes: vec![blake3::hash(bytes).to_hex().to_string()],
        folder: false,
    }
}
fn id() -> String {
    uuid::Uuid::new_v4().to_string()
}
#[test]
fn duplicate_names_are_isolated_and_retries_do_not_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    let downloads = dir.path().join("Configured Downloads");
    let source = dir.path().join("private.png");
    let bytes = vec![123; 1200 * 1024];
    fs::write(&source, &bytes).unwrap();
    let manifest = single("photo.png", &bytes);
    let first = id();
    let a = desktop(&downloads, &first, &source, &manifest).unwrap();
    let b = desktop(&downloads, &id(), &source, &manifest).unwrap();
    assert_ne!(a, b);
    assert_eq!(fs::read(&a).unwrap(), bytes);
    assert_eq!(fs::read(&b).unwrap(), bytes);
    assert!(a.starts_with(downloads.join("InvarDrop")));
    assert_eq!(desktop(&downloads, &first, &source, &manifest).unwrap(), a);
    fs::write(&a, b"user changed this file").unwrap();
    assert!(desktop(&downloads, &first, &source, &manifest).is_err());
    assert_eq!(fs::read(&a).unwrap(), b"user changed this file");
}
#[test]
fn corruption_and_write_failure_do_not_publish_or_lose_private_files() {
    let dir = tempfile::tempdir().unwrap();
    let downloads = dir.path().join("Downloads");
    let source = dir.path().join("private");
    fs::write(&source, b"changed").unwrap();
    assert!(desktop(&downloads, &id(), &source, &single("file", b"correct")).is_err());
    assert!(
        fs::read_dir(downloads.join("InvarDrop"))
            .unwrap()
            .next()
            .is_none()
    );
    assert_eq!(fs::read(&source).unwrap(), b"changed");
    assert!(!fs::read_dir(dir.path()).unwrap().any(|e| {
        e.unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".invardrop-save-")
    }));
    let mut output = FailingWriter;
    assert!(
        copy_verified(
            &source,
            &mut output,
            7,
            blake3::hash(b"changed").to_hex().as_str()
        )
        .is_err()
    );
}
struct FailingWriter;
impl Write for FailingWriter {
    fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("Disk full"))
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
#[test]
fn nested_and_empty_folders_and_zero_byte_files_are_preserved() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("private");
    fs::create_dir_all(source.join("nested/empty")).unwrap();
    fs::write(source.join("nested/file"), b"verified").unwrap();
    fs::write(source.join("zero"), b"").unwrap();
    let manifest = Manifest {
        tree: bundle::Manifest {
            name: "Folder".into(),
            total: 8,
            entries: vec![
                bundle::Entry {
                    path: "nested".into(),
                    kind: bundle::Kind::Directory,
                    size: 0,
                },
                bundle::Entry {
                    path: "nested/empty".into(),
                    kind: bundle::Kind::Directory,
                    size: 0,
                },
                bundle::Entry {
                    path: "nested/file".into(),
                    kind: bundle::Kind::File,
                    size: 8,
                },
                bundle::Entry {
                    path: "zero".into(),
                    kind: bundle::Kind::File,
                    size: 0,
                },
            ],
        },
        hashes: vec![
            "".into(),
            "".into(),
            blake3::hash(b"verified").to_hex().to_string(),
            blake3::hash(b"").to_hex().to_string(),
        ],
        folder: true,
    };
    manifest.validate().unwrap();
    let path = desktop(&dir.path().join("Downloads"), &id(), &source, &manifest).unwrap();
    verify(&path, &manifest).unwrap();
    assert!(path.join("nested/empty").is_dir());
    assert_eq!(fs::read(path.join("nested/file")).unwrap(), b"verified");
    assert_eq!(fs::metadata(path.join("zero")).unwrap().len(), 0);
}
#[cfg(unix)]
#[test]
fn destination_symlinks_and_receipt_collisions_fail_closed() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let downloads = dir.path().join("Downloads");
    fs::create_dir(&downloads).unwrap();
    let unrelated = dir.path().join("unrelated");
    fs::create_dir(&unrelated).unwrap();
    symlink(&unrelated, downloads.join("InvarDrop")).unwrap();
    let source = dir.path().join("source");
    fs::write(&source, b"abc").unwrap();
    assert!(desktop(&downloads, &id(), &source, &single("file", b"abc")).is_err());
    assert!(fs::read_dir(&unrelated).unwrap().next().is_none());
    fs::remove_file(downloads.join("InvarDrop")).unwrap();
    fs::create_dir(downloads.join("InvarDrop")).unwrap();
    let transfer = id();
    let occupied = downloads
        .join("InvarDrop")
        .join(format!("received-{transfer}"));
    fs::write(&occupied, b"existing").unwrap();
    assert!(desktop(&downloads, &transfer, &source, &single("file", b"abc")).is_err());
    assert_eq!(fs::read(occupied).unwrap(), b"existing");
}
#[tokio::test]
async fn invalid_metadata_and_ids_cannot_escape_downloads() {
    let dir = tempfile::tempdir().unwrap();
    let downloads = dir.path().join("Downloads");
    let target = Destination::Desktop(downloads.clone());
    let invalid = single("../escape", b"");
    assert!(target.publish(&id(), dir.path(), &invalid).await.is_err());
    assert!(
        target
            .publish("../escape", dir.path(), &single("safe", b""))
            .await
            .is_err()
    );
    assert!(!downloads.exists());
    assert!(
        Destination::Unavailable
            .publish(&id(), dir.path(), &single("safe", b""))
            .await
            .is_err()
    );
}
#[cfg(unix)]
#[test]
fn size_counters_above_four_gib_are_not_truncated_or_preallocated() {
    // A sparse source declares >4 GiB but the first failed write ends the copy.
    // No full-file allocation or multi-gigabyte read is needed for this regression.
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("large");
    File::create(&source)
        .unwrap()
        .set_len(4 * 1024 * 1024 * 1024 + 1)
        .unwrap();
    assert!(
        copy_verified(
            &source,
            &mut FailingWriter,
            4 * 1024 * 1024 * 1024 + 1,
            &"0".repeat(64)
        )
        .is_err()
    );
    assert_eq!(
        fs::metadata(source).unwrap().len(),
        4 * 1024 * 1024 * 1024 + 1
    );
}

#[cfg(unix)]
#[test]
fn staging_is_private_outside_downloads_and_cleaned_on_failure() {
    use std::os::unix::fs::PermissionsExt;
    let parent = tempfile::tempdir().unwrap();
    let path;
    {
        let staging = PrivateStaging::new(parent.path()).unwrap();
        path = staging.path().to_owned();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o700
        );
        fs::write(path.join("incomplete"), b"partial").unwrap();
    }
    assert!(!path.exists());
}
