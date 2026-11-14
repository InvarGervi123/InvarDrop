//! Real authenticated TLS regressions for persisted resume sessions.
use super::*;
use crate::secure::{
    bundle::{Entry as TreeEntry, Kind},
    resume::{CHECKPOINT_BYTES, Manifest as ResumeManifest, Position},
};

type TestTls = tokio_rustls::TlsStream<TcpStream>;
fn hash_pattern(size: u64, value: u8) -> String {
    let chunk = vec![value; files::CHUNK_SIZE];
    let mut hash = blake3::Hasher::new();
    let mut remaining = size;
    while remaining > 0 {
        let len = remaining.min(chunk.len() as u64) as usize;
        hash.update(&chunk[..len]);
        remaining -= len as u64;
    }
    hash.finalize().to_hex().to_string()
}
fn position(entry: u32, offset: u64, value: u8) -> Position {
    Position {
        entry,
        offset,
        hash: hash_pattern(offset, value),
    }
}
fn single(size: u64, value: u8) -> ResumeManifest {
    ResumeManifest {
        tree: Manifest {
            name: "resume.bin".into(),
            total: size,
            entries: vec![TreeEntry {
                path: "resume.bin".into(),
                kind: Kind::File,
                size,
            }],
        },
        hashes: vec![hash_pattern(size, value)],
        folder: false,
    }
}
async fn same_identity(service: &Service, identity: &Identity) -> (TestTls, Connection) {
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
async fn offer(
    service: &Service,
    connection: &str,
    tls: &mut TestTls,
    id: &str,
    manifest: ResumeManifest,
) -> Position {
    protocol::send(
        tls,
        &Message::ResumeOffer {
            id: id.into(),
            sender_name: "Resume test sender".into(),
            manifest,
        },
    )
    .await
    .unwrap();
    let incoming = file_status(service, connection, "awaiting_approval").await;
    assert_eq!(incoming.id, id);
    assert!(incoming.path.is_none());
    service.respond_file(connection, id, true).unwrap();
    match raw_message(tls).await {
        Message::ResumeAccept {
            id: accepted,
            accept: true,
            position,
        } => {
            assert_eq!(accepted, id);
            position
        }
        _ => panic!("Expected resume acceptance"),
    }
}
async fn payload(tls: &mut TestTls, id: &str, size: u64, value: u8) {
    let mut remaining = size;
    let chunk = vec![value; files::CHUNK_SIZE];
    while remaining > 0 {
        let count = remaining.min(chunk.len() as u64) as usize;
        protocol::send(
            tls,
            &Message::FileChunk {
                id: id.into(),
                data: chunk[..count].to_vec(),
            },
        )
        .await
        .unwrap();
        remaining -= count as u64;
    }
}
async fn checkpoint(tls: &mut TestTls, id: &str, expected: Position) {
    protocol::send(
        tls,
        &Message::ResumeCheckpoint {
            id: id.into(),
            position: expected.clone(),
        },
    )
    .await
    .unwrap();
    assert!(
        matches!(raw_message(tls).await,Message::ResumeAck{id:ack,position} if ack==id && position==expected)
    );
}
async fn file_end(tls: &mut TestTls, id: &str, index: u32, hash: String, expected: Position) {
    protocol::send(
        tls,
        &Message::ResumeFileEnd {
            id: id.into(),
            index,
            hash,
        },
    )
    .await
    .unwrap();
    assert!(
        matches!(raw_message(tls).await,Message::ResumeAck{id:ack,position} if ack==id && position==expected)
    );
}
async fn recovery(service: &Service, id: &str, verified: u64) {
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
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("Expected dormant verified resume record");
}
async fn finish(tls: &mut TestTls, id: &str) {
    protocol::send(tls, &Message::ResumeEnd { id: id.into() })
        .await
        .unwrap();
    assert!(matches!(raw_message(tls).await,Message::ResumeComplete{id:done} if done==id));
    protocol::send(tls, &Message::ResumeCompleteAck { id: id.into() })
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nested_folder_survives_restart_and_repeated_tls_interruptions_without_reusing_tail() {
    let dir = tempfile::tempdir().unwrap();
    let master = random::<32>().unwrap();
    let identity = Identity::generate().unwrap();
    let size = CHECKPOINT_BYTES + files::CHUNK_SIZE as u64;
    let manifest = ResumeManifest {
        tree: Manifest {
            name: "ResumeFolder".into(),
            total: size,
            entries: vec![
                TreeEntry {
                    path: "empty".into(),
                    kind: Kind::Directory,
                    size: 0,
                },
                TreeEntry {
                    path: "nested".into(),
                    kind: Kind::Directory,
                    size: 0,
                },
                TreeEntry {
                    path: "nested/data.bin".into(),
                    kind: Kind::File,
                    size,
                },
                TreeEntry {
                    path: "zero.bin".into(),
                    kind: Kind::File,
                    size: 0,
                },
            ],
        },
        hashes: vec![
            String::new(),
            String::new(),
            hash_pattern(size, 7),
            hash_pattern(0, 0),
        ],
        folder: true,
    };
    let id = uuid::Uuid::new_v4().to_string();
    let service = start_with_master(dir.path(), Duration::from_secs(10), &master).await;
    let (mut tls, c) = same_identity(&service, &identity).await;
    assert_eq!(
        offer(&service, &c.id, &mut tls, &id, manifest.clone()).await,
        position(2, 0, 0)
    );
    payload(&mut tls, &id, CHECKPOINT_BYTES, 7).await;
    checkpoint(&mut tls, &id, position(2, CHECKPOINT_BYTES, 7)).await;
    // A valid tail that has not been checkpointed must not count as verified.
    payload(&mut tls, &id, files::CHUNK_SIZE as u64, 7).await;
    drop(tls);
    closed(&service, &c.id).await;
    recovery(&service, &id, CHECKPOINT_BYTES).await;
    no_received_files(&service);
    service.stop().await;
    drop(service);
    let service = start_with_master(dir.path(), Duration::from_secs(10), &master).await;
    recovery(&service, &id, CHECKPOINT_BYTES).await;
    let (mut tls, c) = same_identity(&service, &identity).await;
    let saved = offer(&service, &c.id, &mut tls, &id, manifest.clone()).await;
    assert_eq!(saved, position(2, CHECKPOINT_BYTES, 7));
    assert_eq!(
        file_status(&service, &c.id, "receiving").await.transferred,
        CHECKPOINT_BYTES
    );
    payload(&mut tls, &id, size - CHECKPOINT_BYTES, 7).await;
    checkpoint(&mut tls, &id, position(2, size, 7)).await;
    drop(tls);
    closed(&service, &c.id).await;
    recovery(&service, &id, size).await;
    let (mut tls, c) = same_identity(&service, &identity).await;
    assert_eq!(
        offer(&service, &c.id, &mut tls, &id, manifest.clone()).await,
        position(2, size, 7)
    );
    file_end(
        &mut tls,
        &id,
        2,
        manifest.hashes[2].clone(),
        position(3, 0, 0),
    )
    .await;
    file_end(&mut tls, &id, 3, hash_pattern(0, 0), position(4, 0, 0)).await;
    finish(&mut tls, &id).await;
    let complete = file_status(&service, &c.id, "complete").await;
    assert_eq!(complete.transferred, size);
    let path = PathBuf::from(complete.path.unwrap());
    assert!(path.join("empty").is_dir());
    assert_eq!(
        std::fs::metadata(path.join("nested/data.bin"))
            .unwrap()
            .len(),
        size
    );
    assert_eq!(std::fs::metadata(path.join("zero.bin")).unwrap().len(), 0);
    assert_eq!(
        blake3::hash(&std::fs::read(path.join("nested/data.bin")).unwrap())
            .to_hex()
            .as_str(),
        manifest.hashes[2]
    );
    timeout(Duration::from_secs(5), async {
        while service.snapshot().resumable.iter().any(|r| r.id == id) {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    service.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_offsets_and_unapproved_resume_payloads_never_publish_files() {
    for approved in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let service = start(dir.path(), Duration::from_secs(10)).await;
        let (mut tls, _, c) = raw_connected(&service).await;
        let id = uuid::Uuid::new_v4().to_string();
        let manifest = single(3, 8);
        if approved {
            offer(&service, &c.id, &mut tls, &id, manifest).await;
            protocol::send(
                &mut tls,
                &Message::ResumeCheckpoint {
                    id: id.clone(),
                    position: position(0, 16, 8),
                },
            )
            .await
            .unwrap();
        } else {
            protocol::send(
                &mut tls,
                &Message::ResumeOffer {
                    id: id.clone(),
                    sender_name: "Test".into(),
                    manifest,
                },
            )
            .await
            .unwrap();
            file_status(&service, &c.id, "awaiting_approval").await;
            protocol::send(
                &mut tls,
                &Message::FileChunk {
                    id: id.clone(),
                    data: vec![8; 3],
                },
            )
            .await
            .unwrap();
        }
        closed(&service, &c.id).await;
        no_received_files(&service);
        assert!(
            service
                .snapshot()
                .connections
                .iter()
                .filter_map(|c| c.transfer.as_ref())
                .all(|t| t.status != "complete" && t.path.is_none())
        );
        service.stop().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_clears_partial_payload_but_discard_removes_recovery_record() {
    let dir = tempfile::tempdir().unwrap();
    let service = start(dir.path(), Duration::from_secs(10)).await;
    let (mut tls, _, c) = raw_connected(&service).await;
    let id = uuid::Uuid::new_v4().to_string();
    offer(&service, &c.id, &mut tls, &id, single(CHECKPOINT_BYTES, 4)).await;
    payload(&mut tls, &id, files::CHUNK_SIZE as u64, 4).await;
    // The durable record is leased during an active operation.
    assert!(service.discard_resume(&id).await.is_err());
    service.cancel_file(&c.id, &id).unwrap();
    closed(&service, &c.id).await;
    file_status(&service, &c.id, "cancelled").await;
    recovery(&service, &id, 0).await;
    no_received_files(&service);
    let payload_path = dir
        .path()
        .join("resume-v1")
        .join(&id)
        .join("payload")
        .join("resume.bin");
    assert!(!payload_path.exists());
    service.discard_resume(&id).await.unwrap();
    assert!(!service.snapshot().resumable.iter().any(|r| r.id == id));
    assert!(!dir.path().join("resume-v1").join(id).exists());
    service.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn revocation_invalidates_persisted_resume_even_after_pairing_is_allowed_again() {
    let dir = tempfile::tempdir().unwrap();
    let service = start(dir.path(), Duration::from_secs(10)).await;
    let identity = Identity::generate().unwrap();
    let peer = identity.fingerprint().unwrap();
    let (mut tls, c) = same_identity(&service, &identity).await;
    let id = uuid::Uuid::new_v4().to_string();
    offer(&service, &c.id, &mut tls, &id, single(3, 1)).await;
    payload(&mut tls, &id, 1, 1).await;
    drop(tls);
    closed(&service, &c.id).await;
    recovery(&service, &id, 0).await;
    service.revoke(peer.clone()).await.unwrap();
    assert!(service.snapshot().resumable.is_empty());
    service.unblock(peer.clone()).await.unwrap();
    // Merely unblocking or even restoring trust cannot resurrect an older epoch.
    store_lock(&get_store(&service.shared).unwrap())
        .unwrap()
        .trust(&peer)
        .unwrap();
    refresh_trust(&service.shared).unwrap();
    assert!(
        get_catalog(&service.shared)
            .unwrap()
            .get(&peer, &id)
            .is_err()
    );
    service.stop().await;
}
