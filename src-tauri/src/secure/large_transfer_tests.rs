//! Opt-in bounded-resource throughput check; ordinary CI keeps small fixtures.
use super::*;
use std::io::{Read, Seek, SeekFrom, Write};

fn hash_file(path: &Path) -> blake3::Hash {
    let mut file = std::fs::File::open(path).unwrap();
    let mut hash = blake3::Hasher::new();
    let mut chunk = [0u8; files::CHUNK_SIZE];
    loop {
        let n = file.read(&mut chunk).unwrap();
        if n == 0 {
            break;
        }
        hash.update(&chunk[..n]);
    }
    hash.finalize()
}
async fn large_status(
    service: &Service,
    connection: &str,
    id: &str,
    size: u64,
    downloads: &Path,
) -> TransferView {
    let mut previous = 0;
    timeout(Duration::from_secs(240), async {
        loop {
            let snapshot = service.snapshot();
            let c = snapshot
                .connections
                .iter()
                .find(|c| c.id == connection)
                .unwrap();
            let transfer = c.transfer.as_ref().unwrap();
            assert_eq!(transfer.id, id);
            assert!(transfer.transferred >= previous && transfer.transferred <= size);
            previous = transfer.transferred;
            assert!(
                !matches!(transfer.status.as_str(), "error" | "cancelled" | "expired"),
                "Large transfer failed: {:?}",
                transfer.error
            );
            let public = downloads
                .join("InvarDrop")
                .join(format!("received-{id}"))
                .join(&transfer.name);
            if public.exists() {
                assert!(matches!(
                    transfer.status.as_str(),
                    "committing" | "complete"
                ));
                assert_eq!(std::fs::metadata(&public).unwrap().len(), size);
            }
            if transfer.status == "complete" {
                return transfer.clone();
            }
            assert_eq!(c.phase, "connected");
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("Large transfer did not finish within the explicit test budget")
}
async fn wait_ack(service: &Service, id: &str) {
    timeout(Duration::from_secs(10), async {
        while service.snapshot().resumable.iter().any(|r| r.id == id) {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "explicit 100 MB or 1 GB loopback test; creates only temporary test data"]
async fn large_file_round_trip_with_verified_downloads_and_monotonic_progress() {
    let size: u64 = std::env::var("INVARDROP_LARGE_TEST_BYTES")
        .unwrap_or_else(|_| "100000000".into())
        .parse()
        .unwrap();
    assert!(matches!(size, 100_000_000 | 1_000_000_000));
    // Keep large data off /tmp tmpfs. RAII removes the entire fixture after the test.
    let target = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target");
    let d = tempfile::Builder::new()
        .prefix("m15-large-")
        .tempdir_in(target)
        .unwrap();
    let original = d.path().join("m15-large.bin");
    let mut file = std::fs::File::create(&original).unwrap();
    file.set_len(size).unwrap();
    let marker: Vec<u8> = (0..files::CHUNK_SIZE).map(|i| (i % 251) as u8).collect();
    file.write_all(&marker).unwrap();
    file.seek(SeekFrom::End(-(files::CHUNK_SIZE as i64)))
        .unwrap();
    file.write_all(&marker).unwrap();
    file.sync_all().unwrap();
    drop(file);
    let expected = hash_file(&original);
    let da = d.path().join("downloads-a");
    let db = d.path().join("downloads-b");
    let a = start_with_destination(
        &d.path().join("a"),
        Duration::from_secs(30),
        &[81; 32],
        Arc::new(|_| {}),
        Some(crate::secure::downloads::Destination::Desktop(da.clone())),
    )
    .await;
    let b = start_with_destination(
        &d.path().join("b"),
        Duration::from_secs(30),
        &[82; 32],
        Arc::new(|_| {}),
        Some(crate::secure::downloads::Destination::Desktop(db.clone())),
    )
    .await;
    let (ac, bc) = pair(&a, &b).await;
    assert_ne!(a.snapshot().identity, b.snapshot().identity);
    let mut source = original;
    for (sender, receiver, send_id, receive_id, downloads) in
        [(&a, &b, &ac.id, &bc.id, &db), (&b, &a, &bc.id, &ac.id, &da)]
    {
        let started = Instant::now();
        sender.send_resumable_file(send_id, &source).await.unwrap();
        let offer = file_status(receiver, receive_id, "awaiting_approval").await;
        assert_eq!(offer.size, size);
        let public_receipt = downloads
            .join("InvarDrop")
            .join(format!("received-{}", offer.id));
        assert!(!public_receipt.exists());
        receiver.respond_file(receive_id, &offer.id, true).unwrap();
        let result = large_status(receiver, receive_id, &offer.id, size, downloads).await;
        let sent = file_status(sender, send_id, "complete").await;
        assert_eq!(sent.transferred, size);
        let private = PathBuf::from(result.path.unwrap());
        let public = PathBuf::from(result.download_path.unwrap());
        assert_eq!(hash_file(&private), expected);
        assert_eq!(hash_file(&public), expected);
        wait_ack(receiver, &offer.id).await;
        // Remove only test-owned acknowledged private data; reverse direction
        // reads the actual public receipt. Peak fixture payload is about 3 GB.
        let receipt = private.parent().unwrap();
        assert!(receipt.starts_with(d.path()));
        std::fs::remove_dir_all(receipt).unwrap();
        source = public;
        println!(
            "PASS: {size} bytes over TLS; verified private/public digests and monotonic progress; {:.2}s",
            started.elapsed().as_secs_f64()
        );
    }
    a.stop().await;
    b.stop().await;
}
