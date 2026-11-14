//! Sender and receiver actor integration through actual authenticated TLS.
use super::*;
use crate::secure::{
    bundle::{Entry as TreeEntry, Kind},
    resume::{CHECKPOINT_BYTES, Manifest as ResumeManifest, Position},
};
use std::io::{Seek, SeekFrom, Write};
type TestTls = tokio_rustls::TlsStream<TcpStream>;
fn digest(data: &[u8]) -> String {
    blake3::hash(data).to_hex().to_string()
}
fn initial() -> Position {
    Position {
        entry: 0,
        offset: 0,
        hash: digest(b""),
    }
}
fn manifest(data: &[u8]) -> ResumeManifest {
    ResumeManifest {
        tree: Manifest {
            name: "data.bin".into(),
            total: data.len() as u64,
            entries: vec![TreeEntry {
                path: "data.bin".into(),
                kind: Kind::File,
                size: data.len() as u64,
            }],
        },
        hashes: vec![digest(data)],
        folder: false,
    }
}
async fn connect_same(service: &Service, identity: &Identity) -> (TestTls, Connection) {
    let peer = identity.fingerprint().unwrap();
    let store = get_store(&service.shared).unwrap();
    if !store_lock(&store).unwrap().is_trusted(&peer) {
        store_lock(&store).unwrap().trust(&peer).unwrap();
    }
    refresh_trust(&service.shared).unwrap();
    let connector = transport::connector(identity, service.snapshot().identity).unwrap();
    let tcp = TcpStream::connect(addr(service)).await.unwrap();
    let mut tls: TestTls = connector
        .connect("invardrop.local".try_into().unwrap(), tcp)
        .await
        .unwrap()
        .into();
    protocol::send(
        &mut tls,
        &Message::Hello {
            version: 6,
            initiator: true,
            trusted: true,
        },
    )
    .await
    .unwrap();
    assert!(matches!(
        raw_message(&mut tls).await,
        Message::Hello {
            version: 6,
            trusted: true,
            ..
        }
    ));
    assert!(matches!(raw_message(&mut tls).await, Message::Ready {}));
    protocol::send(&mut tls, &Message::Ready {}).await.unwrap();
    (tls, phase(service, "connected").await)
}
async fn source_offer(
    service: &Service,
    c: &str,
    tls: &mut TestTls,
    path: &Path,
) -> (String, ResumeManifest) {
    service.send_resumable_file(c, path).await.unwrap();
    match raw_message(tls).await {
        Message::ResumeOffer { id, manifest, .. } => (id, manifest),
        _ => panic!("Expected source resume offer"),
    }
}
async fn accept(tls: &mut TestTls, id: &str, position: Position) {
    protocol::send(
        tls,
        &Message::ResumeAccept {
            id: id.into(),
            accept: true,
            position,
        },
    )
    .await
    .unwrap();
}
async fn ack(tls: &mut TestTls, id: &str, position: Position) {
    protocol::send(
        tls,
        &Message::ResumeAck {
            id: id.into(),
            position,
        },
    )
    .await
    .unwrap();
}
async fn dormant(service: &Service, id: &str, verified: u64) {
    timeout(Duration::from_secs(10), async {
        loop {
            if service
                .snapshot()
                .resumable
                .iter()
                .any(|r| r.id == id && r.verified == verified)
            {
                break;
            }
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("Expected durable recovery record");
}
async fn no_source_payload(tls: &mut TestTls) {
    timeout(Duration::from_secs(5), async {
        loop {
            match protocol::receive(tls).await {
                Ok(Message::Ping {}) => {
                    let _ = protocol::send(tls, &Message::Pong {}).await;
                }
                Ok(Message::Pong {}) => {}
                Ok(Message::FileChunk { .. } | Message::ResumeComplete { .. }) => {
                    panic!("Invalid resume must not send data or claim completion")
                }
                Ok(_) => panic!("Unexpected response to invalid resume"),
                Err(_) => break,
            }
        }
    })
    .await
    .unwrap();
}
async fn incoming_offer(
    service: &Service,
    c: &str,
    tls: &mut TestTls,
    id: &str,
    manifest: ResumeManifest,
    resuming: bool,
) -> Position {
    protocol::send(
        tls,
        &Message::ResumeOffer {
            id: id.into(),
            sender_name: "Actor test sender".into(),
            manifest,
        },
    )
    .await
    .unwrap();
    let view = file_status(service, c, "awaiting_approval").await;
    assert_eq!(view.id, id);
    assert_eq!(view.resuming, resuming);
    service.respond_file(c, id, true).unwrap();
    match raw_message(tls).await {
        Message::ResumeAccept {
            accept: true,
            position,
            ..
        } => position,
        _ => panic!("Expected explicit acceptance"),
    }
}
async fn upload(tls: &mut TestTls, id: &str, bytes: &[u8]) {
    for chunk in bytes.chunks(files::CHUNK_SIZE) {
        protocol::send(
            tls,
            &Message::FileChunk {
                id: id.into(),
                data: chunk.to_vec(),
            },
        )
        .await
        .unwrap();
    }
}
async fn checkpoint(tls: &mut TestTls, id: &str, position: Position) {
    protocol::send(
        tls,
        &Message::ResumeCheckpoint {
            id: id.into(),
            position: position.clone(),
        },
    )
    .await
    .unwrap();
    assert!(
        matches!(raw_message(tls).await,Message::ResumeAck{position:actual,..} if actual==position)
    );
}
async fn end_file(tls: &mut TestTls, id: &str, hash: String) -> Position {
    protocol::send(
        tls,
        &Message::ResumeFileEnd {
            id: id.into(),
            index: 0,
            hash,
        },
    )
    .await
    .unwrap();
    match raw_message(tls).await {
        Message::ResumeAck { position, .. } => position,
        _ => panic!("Expected durable file-end acknowledgement"),
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fresh_service_file_empty_and_folder_senders_use_real_receiver_approval_and_publication() {
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let a = start(da.path(), Duration::from_secs(10)).await;
    let b = start(db.path(), Duration::from_secs(10)).await;
    let (ac, bc) = pair(&a, &b).await;
    for index in 0..3 {
        if index < 2 {
            let path = da.path().join("data.bin");
            std::fs::write(
                &path,
                if index == 0 {
                    b"".as_slice()
                } else {
                    b"abc".as_slice()
                },
            )
            .unwrap();
            a.send_resumable_file(&ac.id, &path).await.unwrap();
        } else {
            let path = da.path().join("tree");
            std::fs::create_dir_all(path.join("nested/empty")).unwrap();
            std::fs::write(path.join("nested/data"), b"abc").unwrap();
            std::fs::write(path.join("zero"), b"").unwrap();
            for number in 0..80 {
                std::fs::write(path.join(format!("zero{number:03}")), b"").unwrap();
            }
            a.send_resumable_bundle(&ac.id, BundleSource::folder(&path).await.unwrap())
                .await
                .unwrap();
        }
        let incoming = file_status(&b, &bc.id, "awaiting_approval").await;
        assert!(
            !incoming.resuming,
            "A fresh operation must not claim it resumes saved data"
        );
        assert_eq!(incoming.transferred, 0);
        assert!(!file_status(&a, &ac.id, "awaiting_approval").await.resuming);
        b.respond_file(&bc.id, &incoming.id, true).unwrap();
        let received = file_status(&b, &bc.id, "complete").await;
        let sent = file_status(&a, &ac.id, "complete").await;
        assert_eq!(received.transferred, received.size);
        assert_eq!(sent.transferred, sent.size);
        let path = PathBuf::from(received.path.unwrap());
        if index < 2 {
            assert_eq!(
                std::fs::read(path).unwrap(),
                if index == 0 {
                    b"".as_slice()
                } else {
                    b"abc".as_slice()
                }
            );
        } else {
            assert!(path.join("nested/empty").is_dir());
            assert_eq!(std::fs::read(path.join("nested/data")).unwrap(), b"abc");
            assert_eq!(std::fs::metadata(path.join("zero")).unwrap().len(), 0);
        }
        timeout(Duration::from_secs(5), async {
            loop {
                if a.snapshot().resumable.is_empty() && b.snapshot().resumable.is_empty() {
                    break;
                }
                sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }
    a.stop().await;
    b.stop().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn service_sender_resumes_only_remaining_payload_after_verified_checkpoint_and_tls_disconnect()
 {
    let dir = tempfile::tempdir().unwrap();
    let service = start(dir.path(), Duration::from_secs(10)).await;
    let identity = Identity::generate().unwrap();
    let peer = identity.fingerprint().unwrap();
    let (mut tls, c) = connect_same(&service, &identity).await;
    let path = dir.path().join("large.bin");
    let data = vec![0x65; (CHECKPOINT_BYTES + 1024 * 1024) as usize];
    std::fs::write(&path, &data).unwrap();
    let (id, manifest) = source_offer(&service, &c.id, &mut tls, &path).await;
    assert!(
        !file_status(&service, &c.id, "awaiting_approval")
            .await
            .resuming
    );
    accept(&mut tls, &id, initial()).await;
    let mut received = 0u64;
    let checkpoint = loop {
        match raw_message(&mut tls).await {
            Message::FileChunk { data: chunk, .. } => {
                assert!(chunk.iter().all(|byte| *byte == 0x65));
                received += chunk.len() as u64;
            }
            Message::ResumeCheckpoint { position, .. } => {
                assert_eq!(position.offset, CHECKPOINT_BYTES);
                assert_eq!(received, CHECKPOINT_BYTES);
                break position;
            }
            _ => panic!("Expected payload then checkpoint"),
        }
    };
    ack(&mut tls, &id, checkpoint.clone()).await;
    timeout(Duration::from_secs(5), async {
        loop {
            if get_catalog(&service.shared)
                .unwrap()
                .get(&peer, &id)
                .unwrap()
                .position
                == checkpoint
            {
                break;
            }
            sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
    drop(tls);
    closed(&service, &c.id).await;
    dormant(&service, &id, CHECKPOINT_BYTES).await;
    let (mut tls, c) = connect_same(&service, &identity).await;
    service.resume_transfer(&c.id, &id).await.unwrap();
    match raw_message(&mut tls).await {
        Message::ResumeOffer {
            id: offered,
            manifest: actual,
            ..
        } => {
            assert_eq!(offered, id);
            assert_eq!(actual.digest(), manifest.digest());
        }
        _ => panic!("Expected repeat offer after explicit Resume"),
    }
    assert!(
        file_status(&service, &c.id, "awaiting_approval")
            .await
            .resuming
    );
    accept(&mut tls, &id, checkpoint).await;
    let mut tail = Vec::new();
    loop {
        match raw_message(&mut tls).await {
            Message::FileChunk { data, .. } => tail.extend_from_slice(&data),
            Message::ResumeCheckpoint { position, .. } => {
                assert_eq!(position.offset, manifest.tree.total);
                ack(&mut tls, &id, position).await;
            }
            Message::ResumeFileEnd { index, hash, .. } => {
                assert_eq!(index, 0);
                assert_eq!(hash, manifest.hashes[0]);
                ack(
                    &mut tls,
                    &id,
                    Position {
                        entry: 1,
                        offset: 0,
                        hash: digest(b""),
                    },
                )
                .await;
            }
            Message::ResumeEnd { .. } => break,
            _ => panic!("Unexpected sender state"),
        }
    }
    assert_eq!(tail.len(), 1024 * 1024);
    assert_eq!(tail, data[CHECKPOINT_BYTES as usize..]);
    protocol::send(&mut tls, &Message::ResumeComplete { id: id.clone() })
        .await
        .unwrap();
    assert!(matches!(
        raw_message(&mut tls).await,
        Message::ResumeCompleteAck { .. }
    ));
    assert_eq!(
        file_status(&service, &c.id, "complete").await.transferred,
        manifest.tree.total
    );
    assert!(
        get_catalog(&service.shared)
            .unwrap()
            .get(&peer, &id)
            .is_err()
    );
    service.stop().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sender_rejects_changed_source_invalid_offset_and_wrong_prefix_before_payload() {
    for scenario in 0..3 {
        let dir = tempfile::tempdir().unwrap();
        let service = start(dir.path(), Duration::from_secs(10)).await;
        let (mut tls, _, c) = raw_connected(&service).await;
        let path = dir.path().join("data.bin");
        std::fs::write(&path, b"abc").unwrap();
        let (id, _) = source_offer(&service, &c.id, &mut tls, &path).await;
        let position = match scenario {
            0 => {
                std::fs::write(&path, b"abd").unwrap();
                initial()
            }
            1 => Position {
                entry: 0,
                offset: 4,
                hash: digest(b"abc"),
            },
            _ => Position {
                entry: 0,
                offset: 3,
                hash: digest(b"bad"),
            },
        };
        accept(&mut tls, &id, position).await;
        no_source_payload(&mut tls).await;
        closed(&service, &c.id).await;
        let failed = file_status(&service, &c.id, "error").await;
        assert!(failed.path.is_none());
        assert_eq!(failed.transferred, 0);
        no_received_files(&service);
        service.stop().await;
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn corrupted_receiver_prefix_is_rejected_before_resume_acceptance() {
    let dir = tempfile::tempdir().unwrap();
    let service = start(dir.path(), Duration::from_secs(10)).await;
    let identity = Identity::generate().unwrap();
    let (mut tls, c) = connect_same(&service, &identity).await;
    let data = vec![0x45; CHECKPOINT_BYTES as usize + 1];
    let manifest = manifest(&data);
    let id = uuid::Uuid::new_v4().to_string();
    incoming_offer(&service, &c.id, &mut tls, &id, manifest.clone(), false).await;
    upload(&mut tls, &id, &data[..CHECKPOINT_BYTES as usize]).await;
    checkpoint(
        &mut tls,
        &id,
        Position {
            entry: 0,
            offset: CHECKPOINT_BYTES,
            hash: digest(&data[..CHECKPOINT_BYTES as usize]),
        },
    )
    .await;
    drop(tls);
    closed(&service, &c.id).await;
    dormant(&service, &id, CHECKPOINT_BYTES).await;
    let payload = dir
        .path()
        .join("resume-v1")
        .join(&id)
        .join("payload/data.bin");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open(payload)
        .unwrap();
    file.seek(SeekFrom::Start(0)).unwrap();
    file.write_all(b"corrupted").unwrap();
    file.sync_all().unwrap();
    drop(file);
    let (mut tls, c) = connect_same(&service, &identity).await;
    protocol::send(
        &mut tls,
        &Message::ResumeOffer {
            id: id.clone(),
            sender_name: "Same source".into(),
            manifest,
        },
    )
    .await
    .unwrap();
    assert!(
        file_status(&service, &c.id, "awaiting_approval")
            .await
            .resuming
    );
    service.respond_file(&c.id, &id, true).unwrap();
    timeout(Duration::from_secs(5), async {
        loop {
            match protocol::receive(&mut tls).await {
                Ok(Message::Ping {}) => {
                    let _ = protocol::send(&mut tls, &Message::Pong {}).await;
                }
                Ok(Message::Pong {}) => {}
                Err(_) => break,
                Ok(_) => panic!("Corrupt saved bytes must fail before ResumeAccept"),
            }
        }
    })
    .await
    .unwrap();
    closed(&service, &c.id).await;
    assert!(file_status(&service, &c.id, "error").await.path.is_none());
    no_received_files(&service);
    service.discard_resume(&id).await.unwrap();
    service.stop().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resume_finalization_races_preserve_publication_and_recoverable_journal() {
    for scenario in 0..3 {
        let dir = tempfile::tempdir().unwrap();
        let service = start(dir.path(), Duration::from_secs(10)).await;
        let identity = Identity::generate().unwrap();
        let peer = identity.fingerprint().unwrap();
        let (mut tls, c) = connect_same(&service, &identity).await;
        let id = uuid::Uuid::new_v4().to_string();
        incoming_offer(&service, &c.id, &mut tls, &id, manifest(b"abc"), false).await;
        upload(&mut tls, &id, b"abc").await;
        assert_eq!(
            end_file(&mut tls, &id, digest(b"abc")).await,
            Position {
                entry: 1,
                offset: 0,
                hash: digest(b"")
            }
        );
        let gate = lock(&service.shared.state)
            .entries
            .iter()
            .find(|e| e.view.id == c.id)
            .unwrap()
            .gate
            .clone();
        let (at, reached) = tokio::sync::oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let hook = move || {
            let _ = at.send(());
            let _ = wait.recv_timeout(Duration::from_secs(10));
        };
        if scenario == 1 {
            gate.pause_after(hook);
        } else {
            gate.pause_before(hook);
        }
        protocol::send(&mut tls, &Message::ResumeEnd { id: id.clone() })
            .await
            .unwrap();
        timeout(Duration::from_secs(5), reached)
            .await
            .unwrap()
            .unwrap();
        if scenario == 1 {
            assert_eq!(service.cancel_file(&c.id, &id), Err(Error::CommitStarted));
            release.send(()).unwrap();
            assert!(matches!(
                raw_message(&mut tls).await,
                Message::ResumeComplete { .. }
            ));
            let complete = file_status(&service, &c.id, "complete").await;
            assert_eq!(std::fs::read(complete.path.unwrap()).unwrap(), b"abc");
            protocol::send(&mut tls, &Message::ResumeCompleteAck { id: id.clone() })
                .await
                .unwrap();
        } else {
            if scenario == 0 {
                service.cancel_file(&c.id, &id).unwrap();
            } else {
                drop(tls);
                timeout(Duration::from_secs(3), async {
                    while !gate.is_cancelled() {
                        sleep(Duration::from_millis(5)).await;
                    }
                })
                .await
                .expect("Transport loss must cancel the gate before commit is released");
            }
            release.send(()).unwrap();
            closed(&service, &c.id).await;
            let failed = file_status(
                &service,
                &c.id,
                if scenario == 0 { "cancelled" } else { "error" },
            )
            .await;
            assert!(failed.path.is_none());
            no_received_files(&service);
            let record = get_catalog(&service.shared)
                .unwrap()
                .get(&peer, &id)
                .unwrap();
            assert_eq!(record.verified(), if scenario == 0 { 0 } else { 3 });
            assert!(!record.completed);
        }
        service.stop().await;
    }
}
