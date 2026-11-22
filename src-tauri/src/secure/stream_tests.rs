//! Real mutual TLS and production actors over bounded memory streams. These
//! tests open no network listener and never use a radio or the user's keyring.
use super::*;
use crate::secure::{
    bundle::{Entry as TreeEntry, Kind},
    resume::{Manifest as ResumeManifest, Position},
};
#[cfg(not(target_os = "android"))]
use tokio::sync::oneshot;
type MemoryTls = tokio_rustls::TlsStream<tokio::io::DuplexStream>;

#[cfg(not(target_os = "android"))]
#[test]
fn bluetooth_attachment_uses_the_tauri_runtime_from_synchronous_app_setup() {
    let root = tempfile::tempdir().unwrap();
    let shared = make_shared(
        Arc::new(AtomicU16::new(0)),
        Arc::new(|_| {}),
        Duration::from_secs(10),
        root.path().join("received"),
    );
    let service = Service {
        shared,
        listener: Mutex::new(None),
    };
    let bluetooth = crate::bluetooth::Controller::new(
        uuid::Uuid::new_v4().to_string(),
        "InvarDrop setup".into(),
        |_| {},
    );
    service.attach_bluetooth(bluetooth.clone());
    assert_eq!(bluetooth.snapshot().status, "disabled");
    tauri::async_runtime::block_on(service.stop());
}

#[cfg(not(target_os = "android"))]
#[tokio::test]
async fn nearby_prefers_lan_and_only_falls_back_before_identity_acceptance() {
    let root = tempfile::tempdir().unwrap();
    let a = start(&root.path().join("sender"), Duration::from_secs(10)).await;
    let b = start(&root.path().join("receiver"), Duration::from_secs(10)).await;
    let other = start(&root.path().join("different-peer"), Duration::from_secs(10)).await;
    let contact = uuid::Uuid::new_v4().to_string();
    let bluetooth = crate::bluetooth::Controller::new(
        uuid::Uuid::new_v4().to_string(),
        "Test sender".into(),
        |_| {},
    );
    let (client, server) = tokio::io::duplex(4096);
    let mut requested = bluetooth.test_stream_route(contact.clone(), Box::new(client));
    a.attach_bluetooth(bluetooth);

    let lan = a.connect_nearby(contact.clone(), vec![addr(&b)]).unwrap();
    let ac = phase(&a, "verifying").await;
    let bc = phase(&b, "verifying").await;
    assert_eq!(ac.id, lan);
    assert_eq!(ac.transport, "lan");
    assert!(
        ac.code.is_some() && ac.code == bc.code,
        "Both codes must match"
    );
    a.respond(&ac.id, true, ac.code.unwrap()).unwrap();
    b.respond(&bc.id, true, bc.code.unwrap()).unwrap();
    phase(&a, "connected").await;
    phase(&b, "connected").await;
    assert!(matches!(
        requested.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    a.cancel(&lan).unwrap();
    closed(&a, &lan).await;
    closed(&b, &bc.id).await;

    let wrong_peer = a
        .connect_nearby(contact.clone(), vec![addr(&other)])
        .unwrap();
    closed(&a, &wrong_peer).await;
    let failed = a
        .snapshot()
        .connections
        .into_iter()
        .find(|c| c.id == wrong_peer)
        .unwrap();
    assert_eq!(failed.error, Some(Error::Identity.to_string()));
    assert!(matches!(
        requested.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));

    let fallback = a.connect_nearby(contact, vec![]).unwrap();
    let incoming = spawn_connection(&b.shared, Input::Stream(Box::new(server)), None).unwrap();
    timeout(Duration::from_secs(5), requested)
        .await
        .unwrap()
        .unwrap();
    let ac = phase(&a, "connected").await;
    let bc = phase(&b, "connected").await;
    assert_eq!((ac.id, bc.id), (fallback, incoming));
    assert_eq!(ac.transport, "bluetooth");
    assert_eq!(bc.transport, "bluetooth");
    assert_eq!(ac.peer, b.snapshot().identity);
    assert!(ac.code.is_none() && bc.code.is_none());
    a.stop().await;
    b.stop().await;
    other.stop().await;
}

async fn memory_service(root: &Path) -> Service {
    let store = Arc::new(Mutex::new(
        Store::for_test(root, &random::<32>().unwrap()).unwrap(),
    ));
    let identity = store_lock(&store)
        .unwrap()
        .identity()
        .fingerprint()
        .unwrap();
    let shared = make_shared(
        Arc::new(AtomicU16::new(0)),
        Arc::new(|_| {}),
        Duration::from_secs(10),
        root.join("received"),
    );
    *lock(&shared.catalog) = Some(Arc::new(
        resume::Catalog::new(root.to_owned(), store.clone()).unwrap(),
    ));
    *lock(&shared.store) = Some(store);
    refresh_trust(&shared).unwrap();
    update(&shared, |state| {
        state.snapshot.identity = Some(identity);
        state.snapshot.status = "ready".into();
    });
    Service {
        shared,
        listener: Mutex::new(None),
    }
}
async fn connect_memory(
    service: &Service,
    identity: &Identity,
    trusted: bool,
) -> (MemoryTls, String) {
    let (client, server) = tokio::io::duplex(4096);
    let id = spawn_connection(&service.shared, Input::Stream(Box::new(server)), None).unwrap();
    let mut tls: MemoryTls = transport::connector(identity, service.snapshot().identity)
        .unwrap()
        .connect("invardrop.local".try_into().unwrap(), client)
        .await
        .unwrap()
        .into();
    protocol::send(
        &mut tls,
        &Message::Hello {
            version: 6,
            initiator: true,
            trusted,
        },
    )
    .await
    .unwrap();
    assert!(
        matches!(raw_message(&mut tls).await, Message::Hello { version: 6, initiator: false, trusted: actual } if actual == trusted)
    );
    (tls, id)
}
async fn pair_memory(service: &Service, identity: &Identity) -> (MemoryTls, String) {
    let (mut tls, id) = connect_memory(service, identity, false).await;
    let verifying = phase(service, "verifying").await;
    let (peer, code) =
        transport::peer_and_code(&tls, &identity.fingerprint().unwrap(), true).unwrap();
    assert_eq!(Some(peer), service.snapshot().identity);
    assert!(
        verifying.code.as_ref() == Some(&code),
        "Full exporter codes must match; never log them"
    );
    assert_eq!(
        tls.get_ref().1.protocol_version(),
        Some(rustls::ProtocolVersion::TLSv1_3)
    );
    assert_eq!(tls.get_ref().1.alpn_protocol(), Some(transport::ALPN));
    assert!(service.can_send_file(&id).is_err());
    assert!(service.respond(&id, true, "wrong code".into()).is_err());
    service.respond(&id, true, code).unwrap();
    assert!(matches!(
        raw_message(&mut tls).await,
        Message::Decision { approve: true }
    ));
    assert!(
        service.snapshot().trusted.is_empty(),
        "One approval cannot create trust"
    );
    protocol::send(&mut tls, &Message::Decision { approve: true })
        .await
        .unwrap();
    assert!(matches!(raw_message(&mut tls).await, Message::Ready {}));
    protocol::send(&mut tls, &Message::Ready {}).await.unwrap();
    let connected = phase(service, "connected").await;
    assert_eq!(connected.transport, "bluetooth");
    assert!(connected.code.is_none());
    assert!(
        service
            .snapshot()
            .trusted
            .contains(&identity.fingerprint().unwrap())
    );
    assert_eq!(service.shared.port.load(Ordering::Acquire), 0);
    (tls, id)
}
async fn trusted_memory(service: &Service, identity: &Identity) -> (MemoryTls, String) {
    let (mut tls, id) = connect_memory(service, identity, true).await;
    assert!(matches!(raw_message(&mut tls).await, Message::Ready {}));
    protocol::send(&mut tls, &Message::Ready {}).await.unwrap();
    assert!(phase(service, "connected").await.code.is_none());
    (tls, id)
}

#[tokio::test]
async fn in_memory_tls_pairing_approval_and_bidirectional_blake3_transfer() {
    let root = tempfile::tempdir().unwrap();
    let service = memory_service(root.path()).await;
    let identity = Identity::generate().unwrap();
    let (mut tls, connection) = pair_memory(&service, &identity).await;
    let data = vec![0xa7; files::CHUNK_SIZE * 3 + 19];
    let id = uuid::Uuid::new_v4().to_string();
    protocol::send(
        &mut tls,
        &Message::FileOffer {
            sender_name: "Memory sender".into(),
            id: id.clone(),
            name: "received.bin".into(),
            size: data.len() as u64,
        },
    )
    .await
    .unwrap();
    file_status(&service, &connection, "awaiting_approval").await;
    no_received_files(&service);
    service.respond_file(&connection, &id, true).unwrap();
    assert!(matches!(
        raw_message(&mut tls).await,
        Message::FileAccept { accept: true, .. }
    ));
    for chunk in data.chunks(files::CHUNK_SIZE) {
        protocol::send(
            &mut tls,
            &Message::FileChunk {
                id: id.clone(),
                data: chunk.to_vec(),
            },
        )
        .await
        .unwrap();
    }
    protocol::send(
        &mut tls,
        &Message::FileFinish {
            id: id.clone(),
            hash: blake3::hash(&data).to_hex().to_string(),
        },
    )
    .await
    .unwrap();
    assert!(
        matches!(raw_message(&mut tls).await, Message::FileComplete { id: done } if done == id)
    );
    let receipt = file_status(&service, &connection, "complete").await;
    let received = std::fs::read(receipt.path.unwrap()).unwrap();
    assert_eq!(blake3::hash(&received), blake3::hash(&data));
    let source = root.path().join("source.bin");
    std::fs::write(&source, &data).unwrap();
    service
        .send_file(&connection, Source::open(&source).await.unwrap())
        .unwrap();
    let id = match raw_message(&mut tls).await {
        Message::FileOffer { id, .. } => id,
        _ => panic!("Expected offer"),
    };
    protocol::send(
        &mut tls,
        &Message::FileAccept {
            id: id.clone(),
            accept: true,
        },
    )
    .await
    .unwrap();
    let mut actual = Vec::new();
    loop {
        match raw_message(&mut tls).await {
            Message::FileChunk { data, .. } => actual.extend(data),
            Message::FileFinish { hash, .. } => {
                assert_eq!(hash, blake3::hash(&actual).to_hex().to_string());
                break;
            }
            _ => panic!("Unexpected transfer message"),
        }
    }
    assert_eq!(actual, data);
    protocol::send(&mut tls, &Message::FileComplete { id })
        .await
        .unwrap();
    let completed = file_status(&service, &connection, "complete").await;
    assert_eq!(std::fs::read(source).unwrap(), data);
    drop(tls);
    closed(&service, &connection).await;
    let after_close = file_status(&service, &connection, "complete").await;
    assert_eq!(completed.id, after_close.id);
    assert_eq!(after_close.error, None);
    service.stop().await;
}

#[tokio::test]
async fn in_memory_tls_rejects_wrong_pin_and_cancels_stalled_handshake() {
    let root = tempfile::tempdir().unwrap();
    let service = memory_service(root.path()).await;
    let identity = Identity::generate().unwrap();
    let wrong = Identity::generate().unwrap().fingerprint().unwrap();
    let (client, server) = tokio::io::duplex(257);
    let id = spawn_connection(&service.shared, Input::Stream(Box::new(server)), None).unwrap();
    assert!(
        transport::connector(&identity, Some(wrong))
            .unwrap()
            .connect("invardrop.local".try_into().unwrap(), client)
            .await
            .is_err()
    );
    closed(&service, &id).await;
    assert!(service.snapshot().trusted.is_empty());
    assert!(
        service
            .snapshot()
            .connections
            .iter()
            .all(|c| c.code.is_none())
    );
    let (_client, server) = tokio::io::duplex(257);
    let id = spawn_connection(&service.shared, Input::Stream(Box::new(server)), None).unwrap();
    service.cancel(&id).unwrap();
    timeout(Duration::from_secs(1), closed(&service, &id))
        .await
        .unwrap();
    service.stop().await;
    assert_eq!(service.shared.slots.available_permits(), MAX_SESSIONS);
}

#[tokio::test]
async fn in_memory_tls_unapproved_payload_and_corrupt_hash_never_publish() {
    for approve in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let service = memory_service(root.path()).await;
        let identity = Identity::generate().unwrap();
        let (mut tls, connection) = pair_memory(&service, &identity).await;
        let id = uuid::Uuid::new_v4().to_string();
        protocol::send(
            &mut tls,
            &Message::FileOffer {
                sender_name: "Memory sender".into(),
                id: id.clone(),
                name: "bad.bin".into(),
                size: 3,
            },
        )
        .await
        .unwrap();
        file_status(&service, &connection, "awaiting_approval").await;
        if approve {
            service.respond_file(&connection, &id, true).unwrap();
            assert!(matches!(
                raw_message(&mut tls).await,
                Message::FileAccept { accept: true, .. }
            ));
        }
        protocol::send(
            &mut tls,
            &Message::FileChunk {
                id: id.clone(),
                data: b"abc".to_vec(),
            },
        )
        .await
        .unwrap();
        if approve {
            let _ = protocol::send(
                &mut tls,
                &Message::FileFinish {
                    id,
                    hash: "0".repeat(64),
                },
            )
            .await;
        }
        closed(&service, &connection).await;
        no_received_files(&service);
        service.stop().await;
    }
}

async fn resume_offer(
    service: &Service,
    connection: &str,
    tls: &mut MemoryTls,
    id: &str,
    manifest: ResumeManifest,
) -> Position {
    protocol::send(
        tls,
        &Message::ResumeOffer {
            id: id.into(),
            sender_name: "Memory sender".into(),
            manifest,
        },
    )
    .await
    .unwrap();
    let offer = file_status(service, connection, "awaiting_approval").await;
    assert_eq!(offer.id, id);
    service.respond_file(connection, id, true).unwrap();
    match raw_message(tls).await {
        Message::ResumeAccept {
            accept: true,
            position,
            ..
        } => position,
        _ => panic!("Expected explicit resume acceptance"),
    }
}
async fn resume_file(tls: &mut MemoryTls, id: &str, index: u32, data: &[u8]) {
    protocol::send(
        tls,
        &Message::FileChunk {
            id: id.into(),
            data: data.to_vec(),
        },
    )
    .await
    .unwrap();
    protocol::send(
        tls,
        &Message::ResumeFileEnd {
            id: id.into(),
            index,
            hash: blake3::hash(data).to_hex().to_string(),
        },
    )
    .await
    .unwrap();
    assert!(matches!(raw_message(tls).await, Message::ResumeAck { .. }));
}
#[tokio::test]
async fn in_memory_tls_reconnect_never_resumes_without_sender_and_receiver_approval() {
    let root = tempfile::tempdir().unwrap();
    let service = memory_service(root.path()).await;
    let identity = Identity::generate().unwrap();
    let (mut tls, connection) = pair_memory(&service, &identity).await;
    let first = b"verified prefix";
    let second = b"explicitly approved tail";
    let manifest = ResumeManifest {
        tree: Manifest {
            name: "folder".into(),
            total: (first.len() + second.len()) as u64,
            entries: vec![
                TreeEntry {
                    path: "first.bin".into(),
                    kind: Kind::File,
                    size: first.len() as u64,
                },
                TreeEntry {
                    path: "second.bin".into(),
                    kind: Kind::File,
                    size: second.len() as u64,
                },
            ],
        },
        hashes: vec![
            blake3::hash(first).to_hex().to_string(),
            blake3::hash(second).to_hex().to_string(),
        ],
        folder: true,
    };
    let id = uuid::Uuid::new_v4().to_string();
    assert_eq!(
        resume_offer(&service, &connection, &mut tls, &id, manifest.clone())
            .await
            .offset,
        0
    );
    resume_file(&mut tls, &id, 0, first).await;
    drop(tls);
    closed(&service, &connection).await;
    let record = get_catalog(&service.shared)
        .unwrap()
        .get(&identity.fingerprint().unwrap(), &id)
        .unwrap();
    assert_eq!(record.verified(), first.len() as u64);
    no_received_files(&service);
    let (mut tls, connection) = trusted_memory(&service, &identity).await;
    // Ping/Pong is an actor barrier: reconnect has processed messages, yet has
    // neither offered a transfer nor started payload or approval automatically.
    protocol::send(&mut tls, &Message::Ping {}).await.unwrap();
    assert!(matches!(
        protocol::receive(&mut tls).await.unwrap(),
        Message::Pong {} | Message::Ping {}
    ));
    assert!(
        service
            .snapshot()
            .connections
            .iter()
            .find(|c| c.id == connection)
            .unwrap()
            .transfer
            .is_none()
    );
    service.resume_transfer(&connection, &id).await.unwrap();
    assert!(matches!(
        raw_message(&mut tls).await,
        Message::ResumeRequest { .. }
    ));
    let position = resume_offer(&service, &connection, &mut tls, &id, manifest).await;
    assert_eq!(position.entry, 1);
    assert_eq!(position.offset, 0);
    resume_file(&mut tls, &id, 1, second).await;
    protocol::send(&mut tls, &Message::ResumeEnd { id: id.clone() })
        .await
        .unwrap();
    assert!(matches!(
        raw_message(&mut tls).await,
        Message::ResumeComplete { .. }
    ));
    protocol::send(&mut tls, &Message::ResumeCompleteAck { id })
        .await
        .unwrap();
    let receipt = file_status(&service, &connection, "complete").await;
    let path = PathBuf::from(receipt.path.unwrap());
    assert_eq!(std::fs::read(path.join("first.bin")).unwrap(), first);
    assert_eq!(std::fs::read(path.join("second.bin")).unwrap(), second);
    service
        .revoke(identity.fingerprint().unwrap())
        .await
        .unwrap();
    closed(&service, &connection).await;
    service.stop().await;
}
