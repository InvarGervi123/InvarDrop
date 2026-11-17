//! Actual TLS, separate identities/profiles, and isolated Downloads destinations.
use super::*;
use crate::secure::downloads::Destination;
async fn receiving(root: &Path, downloads: &Path, key: &[u8]) -> Service {
    start_with_destination(
        root,
        Duration::from_secs(10),
        key,
        Arc::new(|_| {}),
        Some(Destination::Desktop(downloads.to_owned())),
    )
    .await
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn downloads_require_approval_and_duplicate_names_preserve_both_files() {
    let d = tempfile::tempdir().unwrap();
    let downloads = d.path().join("XDG Downloads");
    let a = start(&d.path().join("a"), Duration::from_secs(10)).await;
    let b = receiving(&d.path().join("b"), &downloads, &[53; 32]).await;
    let (ac, bc) = pair(&a, &b).await;
    let source = d.path().join("photo.png");
    let bytes: Vec<u8> = (0..1200 * 1024).map(|i| (i % 251) as u8).collect();
    std::fs::write(&source, &bytes).unwrap();
    a.send_resumable_file(&ac.id, &source).await.unwrap();
    let offer = file_status(&b, &bc.id, "awaiting_approval").await;
    assert!(!downloads.exists());
    b.respond_file(&bc.id, &offer.id, false).unwrap();
    file_status(&a, &ac.id, "rejected").await;
    assert!(!downloads.exists());
    let mut paths = Vec::new();
    for _ in 0..2 {
        a.send_resumable_file(&ac.id, &source).await.unwrap();
        let offer = file_status(&b, &bc.id, "awaiting_approval").await;
        b.respond_file(&bc.id, &offer.id, true).unwrap();
        let received = file_status(&b, &bc.id, "complete").await;
        file_status(&a, &ac.id, "complete").await;
        let public = PathBuf::from(received.download_path.unwrap());
        assert!(public.starts_with(downloads.join("InvarDrop")));
        assert_eq!(std::fs::read(&public).unwrap(), bytes);
        assert_eq!(std::fs::read(received.path.unwrap()).unwrap(), bytes);
        paths.push(public);
    }
    assert_ne!(paths[0], paths[1]);
    assert_eq!(std::fs::read(&paths[0]).unwrap(), bytes);
    a.stop().await;
    b.stop().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_download_publication_recovers_after_restart_without_retransmission() {
    let d = tempfile::tempdir().unwrap();
    let downloads = d.path().join("Downloads");
    std::fs::create_dir(&downloads).unwrap();
    std::fs::write(downloads.join("InvarDrop"), b"existing user file").unwrap();
    let receiver_root = d.path().join("b");
    let key = [54; 32];
    let a = start(&d.path().join("a"), Duration::from_secs(10)).await;
    let b = receiving(&receiver_root, &downloads, &key).await;
    let receiver_identity = b.snapshot().identity;
    let (ac, bc) = pair(&a, &b).await;
    let source = d.path().join("file");
    std::fs::write(&source, b"verified recovery data").unwrap();
    a.send_resumable_file(&ac.id, &source).await.unwrap();
    let offer = file_status(&b, &bc.id, "awaiting_approval").await;
    b.respond_file(&bc.id, &offer.id, true).unwrap();
    let failed = file_status(&b, &bc.id, "error").await;
    assert_eq!(failed.error, Some(Error::Downloads.to_string()));
    assert!(failed.download_path.is_none());
    closed(&a, &ac.id).await;
    assert_eq!(
        std::fs::read(downloads.join("InvarDrop")).unwrap(),
        b"existing user file"
    );
    let private = get_catalog(&b.shared)
        .unwrap()
        .completed(&a.snapshot().identity.unwrap(), &offer.id)
        .unwrap()
        .unwrap();
    assert_eq!(std::fs::read(&private).unwrap(), b"verified recovery data");
    b.stop().await;
    drop(b);
    // The receiver already has every verified byte. Recovery must finish even
    // when retransmission is impossible because the sender source is gone.
    std::fs::remove_file(&source).unwrap();
    // Remove only this test's deliberate destination collision.
    std::fs::remove_file(downloads.join("InvarDrop")).unwrap();
    let b = receiving(&receiver_root, &downloads, &key).await;
    assert_eq!(b.snapshot().identity, receiver_identity);
    a.connect("restored".into(), vec![addr(&b)]).unwrap();
    let ac = phase(&a, "connected").await;
    let bc = phase(&b, "connected").await;
    a.resume_transfer(&ac.id, &offer.id).await.unwrap();
    let received = file_status(&b, &bc.id, "complete").await;
    file_status(&a, &ac.id, "complete").await;
    assert_eq!(
        std::fs::read(received.download_path.unwrap()).unwrap(),
        b"verified recovery data"
    );
    assert_eq!(received.transferred, offer.size);
    a.stop().await;
    b.stop().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn legacy_single_and_bundle_downloads_keep_verified_bytes_and_empty_directories() {
    let d = tempfile::tempdir().unwrap();
    let downloads = d.path().join("Downloads");
    let a = start(&d.path().join("a"), Duration::from_secs(10)).await;
    let b = receiving(&d.path().join("b"), &downloads, &[55; 32]).await;
    let (ac, bc) = pair(&a, &b).await;
    let empty = d.path().join("zero");
    std::fs::write(&empty, b"").unwrap();
    a.send_file(&ac.id, Source::open(&empty).await.unwrap())
        .unwrap();
    let offer = file_status(&b, &bc.id, "awaiting_approval").await;
    b.respond_file(&bc.id, &offer.id, true).unwrap();
    let received = file_status(&b, &bc.id, "complete").await;
    file_status(&a, &ac.id, "complete").await;
    assert_eq!(
        std::fs::metadata(received.download_path.unwrap())
            .unwrap()
            .len(),
        0
    );
    let folder = d.path().join("Folder");
    std::fs::create_dir_all(folder.join("nested/empty")).unwrap();
    std::fs::write(folder.join("nested/file"), b"folder data").unwrap();
    a.send_bundle(&ac.id, BundleSource::folder(&folder).await.unwrap())
        .unwrap();
    let offer = file_status(&b, &bc.id, "awaiting_approval").await;
    b.respond_file(&bc.id, &offer.id, true).unwrap();
    let received = file_status(&b, &bc.id, "complete").await;
    file_status(&a, &ac.id, "complete").await;
    let public = PathBuf::from(received.download_path.unwrap());
    assert!(public.join("nested/empty").is_dir());
    assert_eq!(
        std::fs::read(public.join("nested/file")).unwrap(),
        b"folder data"
    );
    a.stop().await;
    b.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_before_commit_never_creates_downloads() {
    let d = tempfile::tempdir().unwrap();
    let downloads = d.path().join("Downloads");
    let a = start(&d.path().join("a"), Duration::from_secs(10)).await;
    let b = receiving(&d.path().join("b"), &downloads, &[56; 32]).await;
    let (ac, bc) = pair(&a, &b).await;
    let source = d.path().join("file");
    std::fs::write(&source, b"verified but cancelled before publication").unwrap();
    a.send_resumable_file(&ac.id, &source).await.unwrap();
    let offer = file_status(&b, &bc.id, "awaiting_approval").await;
    let gate = lock(&b.shared.state)
        .entries
        .iter()
        .find(|e| e.view.id == bc.id)
        .unwrap()
        .gate
        .clone();
    let (reached, wait) = tokio::sync::oneshot::channel();
    let (release, hold) = std::sync::mpsc::channel();
    gate.pause_before(move || {
        let _ = reached.send(());
        hold.recv_timeout(Duration::from_secs(10)).unwrap();
    });
    b.respond_file(&bc.id, &offer.id, true).unwrap();
    timeout(Duration::from_secs(10), wait)
        .await
        .unwrap()
        .unwrap();
    b.cancel_file(&bc.id, &offer.id).unwrap();
    release.send(()).unwrap();
    closed(&b, &bc.id).await;
    assert!(!downloads.exists());
    assert!(
        !b.snapshot()
            .connections
            .iter()
            .filter_map(|c| c.transfer.as_ref())
            .any(|t| t.status == "complete")
    );
    a.stop().await;
    b.stop().await;
}
