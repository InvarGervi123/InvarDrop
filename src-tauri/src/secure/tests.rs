use super::super::{
    error::Error,
    identity::{Identity, random},
    protocol::{self, Message},
    store::Store,
    transport,
};
use super::*;
use std::{
    net::{Ipv4Addr, SocketAddr},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU16, Ordering},
    },
    time::Duration,
};
use tokio::{
    net::TcpStream,
    time::{sleep, timeout},
};

thread_local! {
    static TRUST_READ_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        std::cell::RefCell::new(None);
}
pub(super) fn after_trust_read() {
    TRUST_READ_HOOK.with(|hook| {
        if let Some(pause) = hook.borrow_mut().take() {
            pause();
        }
    });
}

async fn start(root: &Path, duration: Duration) -> Service {
    start_with_master(root, duration, &random::<32>().unwrap()).await
}
async fn start_with_master(root: &Path, duration: Duration, master: &[u8]) -> Service {
    let store = Store::for_test(root, master).unwrap();
    let mut shared = make_shared(
        Arc::new(AtomicU16::new(0)),
        Arc::new(|_| {}),
        duration,
        root.join("received"),
    );
    Arc::get_mut(&mut shared).unwrap().file_approval_duration = duration;
    let s = shared.clone();
    let task = tokio::spawn(async move {
        listen(s, Arc::new(Mutex::new(store))).await.unwrap();
    });
    let service = Service {
        shared,
        listener: Mutex::new(Some(tauri::async_runtime::JoinHandle::Tokio(task))),
    };
    timeout(Duration::from_secs(10), async {
        while service.shared.port.load(Ordering::Acquire) == 0 {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("test listener startup");
    service
}
fn addr(s: &Service) -> SocketAddr {
    (Ipv4Addr::LOCALHOST, s.shared.port.load(Ordering::Acquire)).into()
}
async fn phase(s: &Service, desired: &str) -> Connection {
    timeout(Duration::from_secs(12), async {
        loop {
            if let Some(c) = s
                .snapshot()
                .connections
                .into_iter()
                .rev()
                .find(|c| c.phase == desired)
            {
                return c;
            }
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("Expected connection phase {desired}"))
}
async fn closed(s: &Service, id: &str) {
    timeout(Duration::from_secs(12), async {
        while !s
            .snapshot()
            .connections
            .iter()
            .any(|c| c.id == id && c.phase == "closed")
        {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("Expected the specific connection to close");
}
async fn pair(a: &Service, b: &Service) -> (Connection, Connection) {
    a.connect("test-contact".into(), vec![addr(b)]).unwrap();
    let ac = phase(a, "verifying").await;
    let bc = phase(b, "verifying").await;
    assert!(
        ac.code.is_some() && ac.code == bc.code,
        "SAS must match; values deliberately not logged"
    );
    a.respond(&ac.id, true, ac.code.unwrap()).unwrap();
    b.respond(&bc.id, true, bc.code.unwrap()).unwrap();
    (phase(a, "connected").await, phase(b, "connected").await)
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_tls_pair_reconnect_revoke_and_clean_shutdown() {
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let a = start(da.path(), Duration::from_secs(10)).await;
    let b = start(db.path(), Duration::from_secs(10)).await;
    let (ac, bc) = pair(&a, &b).await;
    assert!(
        a.snapshot()
            .trusted
            .contains(&b.snapshot().identity.unwrap())
    );
    assert!(ac.code.is_none() && bc.code.is_none());
    a.cancel(&ac.id).unwrap();
    closed(&a, &ac.id).await;
    closed(&b, &bc.id).await;
    a.reconnect(&ac.id).unwrap();
    let again = phase(&a, "connected").await;
    let b_again = phase(&b, "connected").await;
    assert!(again.code.is_none());
    let peer = b.snapshot().identity.unwrap();
    a.revoke(peer.clone()).await.unwrap();
    closed(&a, &again.id).await;
    closed(&b, &b_again.id).await;
    assert!(a.snapshot().revoked.contains(&peer));
    assert!(matches!(a.reconnect(&again.id), Err(Error::Revoked)));
    let port = addr(&a);
    a.stop().await;
    b.stop().await;
    assert!(a.shared.slots.available_permits() == MAX_SESSIONS);
    assert!(tokio::net::TcpListener::bind(port).await.is_ok());
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejection_and_one_sided_approval_never_create_trust() {
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let a = start(da.path(), Duration::from_secs(10)).await;
    let b = start(db.path(), Duration::from_secs(10)).await;
    a.connect("b".into(), vec![addr(&b)]).unwrap();
    let ac = phase(&a, "verifying").await;
    let bc = phase(&b, "verifying").await;
    assert!(a.respond(&ac.id, true, "wrong-code".into()).is_err());
    a.respond(&ac.id, true, ac.code.unwrap()).unwrap();
    sleep(Duration::from_millis(50)).await;
    assert!(a.snapshot().trusted.is_empty() && b.snapshot().trusted.is_empty());
    b.respond(&bc.id, false, String::new()).unwrap();
    phase(&a, "closed").await;
    phase(&b, "closed").await;
    assert!(a.snapshot().trusted.is_empty() && b.snapshot().trusted.is_empty());
    assert!(a.respond(&ac.id, true, String::new()).is_err());
    a.stop().await;
    b.stop().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn expired_pairing_has_no_persistent_side_effects() {
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let a = start(da.path(), Duration::from_millis(500)).await;
    let b = start(db.path(), Duration::from_millis(500)).await;
    a.connect("b".into(), vec![addr(&b)]).unwrap();
    phase(&a, "closed").await;
    phase(&b, "closed").await;
    assert!(a.snapshot().trusted.is_empty() && b.snapshot().trusted.is_empty());
    a.stop().await;
    b.stop().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wrong_pinned_identity_fails_tls_without_pairing_fallback() {
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let a = start(da.path(), Duration::from_secs(10)).await;
    let b = start(db.path(), Duration::from_secs(10)).await;
    let wrong = Identity::generate().unwrap().fingerprint().unwrap();
    spawn_connection(
        &a.shared,
        Input::Outgoing(vec![addr(&b)], Some(wrong)),
        None,
    )
    .unwrap();
    let c = phase(&a, "closed").await;
    assert!(c.error == Some(Error::Identity.to_string()));
    assert!(c.code.is_none() && a.snapshot().trusted.is_empty());
    a.stop().await;
    b.stop().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn authenticated_tls_rejects_out_of_order_application_messages() {
    let dir = tempfile::tempdir().unwrap();
    let server = start(dir.path(), Duration::from_secs(10)).await;
    let identity = Identity::generate().unwrap();
    let connector = transport::connector(&identity, None).unwrap();
    let tcp = TcpStream::connect(addr(&server)).await.unwrap();
    let mut tls = connector
        .connect("invardrop.local".try_into().unwrap(), tcp)
        .await
        .unwrap();
    protocol::send(
        &mut tls,
        &Message::Hello {
            version: 3,
            initiator: true,
            trusted: false,
        },
    )
    .await
    .unwrap();
    assert!(matches!(
        protocol::receive(&mut tls).await.unwrap(),
        Message::Hello { version: 3, .. }
    ));
    protocol::send(&mut tls, &Message::Ready {}).await.unwrap();
    let c = phase(&server, "closed").await;
    assert!(c.error == Some(Error::Protocol.to_string()));
    assert!(server.snapshot().trusted.is_empty());
    server.stop().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_mitm_channels_produce_different_codes_and_no_automatic_trust() {
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let dm = tempfile::tempdir().unwrap();
    let a = start(da.path(), Duration::from_secs(10)).await;
    let b = start(db.path(), Duration::from_secs(10)).await;
    let m = start(dm.path(), Duration::from_secs(10)).await;
    m.connect("a".into(), vec![addr(&a)]).unwrap();
    phase(&a, "verifying").await;
    m.connect("b".into(), vec![addr(&b)]).unwrap();
    let bc = phase(&b, "verifying").await;
    let ac = phase(&a, "verifying").await;
    assert!(
        ac.code.is_some() && ac.code != bc.code,
        "Distinct channels must not share the displayed code"
    );
    assert!(a.snapshot().trusted.is_empty() && b.snapshot().trusted.is_empty());
    a.stop().await;
    b.stop().await;
    m.stop().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stalled_handshakes_are_bounded_and_shutdown_releases_slots() {
    let dir = tempfile::tempdir().unwrap();
    let server = start(dir.path(), Duration::from_secs(10)).await;
    let mut sockets = Vec::new();
    for _ in 0..MAX_SESSIONS + 3 {
        sockets.push(TcpStream::connect(addr(&server)).await.unwrap());
    }
    sleep(Duration::from_millis(50)).await;
    assert!(server.snapshot().connections.len() <= MAX_SESSIONS);
    server.stop().await;
    assert!(server.shared.slots.available_permits() == MAX_SESSIONS);
    assert!(
        server
            .snapshot()
            .connections
            .iter()
            .all(|c| c.phase == "closed")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reconnect_after_restart_uses_persisted_keys_without_repairing() {
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let ma = random::<32>().unwrap();
    let mb = random::<32>().unwrap();
    let duration = Duration::from_secs(10);
    let a = start_with_master(da.path(), duration, &ma).await;
    let b = start_with_master(db.path(), duration, &mb).await;
    pair(&a, &b).await;
    let afp = a.snapshot().identity.unwrap();
    let bfp = b.snapshot().identity.unwrap();
    a.stop().await;
    b.stop().await;
    drop(a);
    drop(b);
    let a = start_with_master(da.path(), duration, &ma).await;
    let b = start_with_master(db.path(), duration, &mb).await;
    assert!(
        a.snapshot().identity.as_ref() == Some(&afp)
            && b.snapshot().identity.as_ref() == Some(&bfp)
    );
    a.connect("new-mdns-session".into(), vec![addr(&b)])
        .unwrap();
    let ac = phase(&a, "connected").await;
    let bc = phase(&b, "connected").await;
    assert!(ac.code.is_none() && bc.code.is_none());
    a.stop().await;
    b.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn asymmetric_trust_never_silently_downgrades_to_pairing() {
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let a = start(da.path(), Duration::from_secs(10)).await;
    let b = start(db.path(), Duration::from_secs(10)).await;
    let afp = a.snapshot().identity.unwrap();
    store_lock(&get_store(&b.shared).unwrap())
        .unwrap()
        .trust(&afp)
        .unwrap();
    a.connect("asymmetric".into(), vec![addr(&b)]).unwrap();
    let ac = phase(&a, "closed").await;
    let bc = phase(&b, "closed").await;
    assert!(ac.code.is_none() && bc.code.is_none());
    assert!(a.snapshot().trusted.is_empty());
    assert!(
        ac.error == Some(Error::Identity.to_string())
            || bc.error == Some(Error::Identity.to_string())
    );
    a.stop().await;
    b.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn partial_tcp_frame_survives_local_approval_without_reader_cancellation() {
    use tokio::io::AsyncWriteExt;
    let dir = tempfile::tempdir().unwrap();
    let server = start(dir.path(), Duration::from_secs(10)).await;
    let identity = Identity::generate().unwrap();
    let connector = transport::connector(&identity, None).unwrap();
    let tcp = TcpStream::connect(addr(&server)).await.unwrap();
    let mut tls: tokio_rustls::TlsStream<TcpStream> = connector
        .connect("invardrop.local".try_into().unwrap(), tcp)
        .await
        .unwrap()
        .into();
    protocol::send(
        &mut tls,
        &Message::Hello {
            version: 3,
            initiator: true,
            trusted: false,
        },
    )
    .await
    .unwrap();
    assert!(matches!(
        protocol::receive(&mut tls).await.unwrap(),
        Message::Hello { version: 3, .. }
    ));
    let request = phase(&server, "verifying").await;
    let (_, code) = transport::peer_and_code(&tls, &identity.fingerprint().unwrap(), true).unwrap();
    assert!(
        request.code.as_ref() == Some(&code),
        "Codes must match without logging values"
    );
    let data = serde_json::to_vec(&Message::Decision { approve: true }).unwrap();
    let length = (data.len() as u32).to_be_bytes();
    tls.write_all(&length[..2]).await.unwrap();
    tls.flush().await.unwrap();
    server.respond(&request.id, true, code).unwrap();
    assert!(matches!(
        timeout(Duration::from_secs(5), protocol::receive(&mut tls))
            .await
            .unwrap()
            .unwrap(),
        Message::Decision { approve: true }
    ));
    tls.write_all(&length[2..]).await.unwrap();
    tls.write_all(&data[..4]).await.unwrap();
    tls.flush().await.unwrap();
    sleep(Duration::from_millis(10)).await;
    tls.write_all(&data[4..]).await.unwrap();
    tls.flush().await.unwrap();
    assert!(matches!(
        timeout(Duration::from_secs(5), protocol::receive(&mut tls))
            .await
            .unwrap()
            .unwrap(),
        Message::Ready {}
    ));
    protocol::send(&mut tls, &Message::Ready {}).await.unwrap();
    phase(&server, "connected").await;
    assert!(
        server
            .snapshot()
            .trusted
            .contains(&identity.fingerprint().unwrap())
    );
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_approval_cannot_be_replayed_on_a_new_tls_channel() {
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let a = start(da.path(), Duration::from_secs(10)).await;
    let b = start(db.path(), Duration::from_secs(10)).await;
    a.connect("b".into(), vec![addr(&b)]).unwrap();
    let old = phase(&a, "verifying").await;
    let old_b = phase(&b, "verifying").await;
    let old_code = old.code.unwrap();
    a.respond(&old.id, true, old_code.clone()).unwrap();
    assert!(matches!(
        a.respond(&old.id, true, old_code.clone()),
        Err(Error::Approval)
    ));
    a.cancel(&old.id).unwrap();
    closed(&a, &old.id).await;
    closed(&b, &old_b.id).await;
    assert!(a.snapshot().trusted.is_empty() && b.snapshot().trusted.is_empty());

    a.connect("b".into(), vec![addr(&b)]).unwrap();
    let current = phase(&a, "verifying").await;
    let current_b = phase(&b, "verifying").await;
    let current_code = current.code.unwrap();
    assert!(old_code != current_code, "A new channel needs a fresh code");
    assert!(matches!(
        a.respond(&old.id, true, current_code.clone()),
        Err(Error::Approval)
    ));
    assert!(matches!(
        a.respond(&current.id, true, old_code),
        Err(Error::Approval)
    ));
    assert!(a.snapshot().trusted.is_empty() && b.snapshot().trusted.is_empty());
    a.respond(&current.id, true, current_code).unwrap();
    b.respond(&current_b.id, true, current_b.code.unwrap())
        .unwrap();
    phase(&a, "connected").await;
    phase(&b, "connected").await;
    a.stop().await;
    b.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn revocation_during_verification_blocks_until_explicit_unblock_and_new_pairing() {
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let a = start(da.path(), Duration::from_secs(10)).await;
    let b = start(db.path(), Duration::from_secs(10)).await;
    a.connect("b".into(), vec![addr(&b)]).unwrap();
    let ac = phase(&a, "verifying").await;
    let bc = phase(&b, "verifying").await;
    let peer = b.snapshot().identity.unwrap();
    a.revoke(peer.clone()).await.unwrap();
    closed(&a, &ac.id).await;
    closed(&b, &bc.id).await;
    assert!(a.snapshot().trusted.is_empty() && b.snapshot().trusted.is_empty());
    assert!(a.snapshot().revoked.contains(&peer));

    let blocked = a.connect("b".into(), vec![addr(&b)]).unwrap();
    closed(&a, &blocked).await;
    let blocked = a
        .snapshot()
        .connections
        .into_iter()
        .find(|c| c.id == blocked)
        .unwrap();
    assert!(blocked.error == Some(Error::Revoked.to_string()));
    assert!(blocked.code.is_none());
    a.unblock(peer).await.unwrap();
    assert!(a.snapshot().trusted.is_empty() && a.snapshot().revoked.is_empty());
    // Unblocking only permits a fresh exchange with two new explicit approvals.
    pair(&a, &b).await;
    a.stop().await;
    b.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connecting_to_own_identity_never_offers_pairing() {
    let dir = tempfile::tempdir().unwrap();
    let s = start(dir.path(), Duration::from_secs(10)).await;
    let id = s.connect("self".into(), vec![addr(&s)]).unwrap();
    closed(&s, &id).await;
    s.stop().await;
    let snapshot = s.snapshot();
    assert!(snapshot.trusted.is_empty());
    assert!(snapshot.connections.len() == 2);
    assert!(snapshot.connections.iter().all(|c| {
        c.phase == "closed" && c.code.is_none() && c.error == Some(Error::Identity.to_string())
    }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_inflight_commit_still_publishes_saved_trust_for_revocation() {
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let a = start(da.path(), Duration::from_secs(10)).await;
    let b = start(db.path(), Duration::from_secs(10)).await;
    a.connect("b".into(), vec![addr(&b)]).unwrap();
    let ac = phase(&a, "verifying").await;
    let bc = phase(&b, "verifying").await;
    let peer = b.snapshot().identity.unwrap();
    let store = get_store(&a.shared).unwrap();
    let (locked_tx, locked_rx) = tokio::sync::oneshot::channel();
    let (release, wait) = std::sync::mpsc::channel::<()>();
    let blocker = tokio::task::spawn_blocking(move || {
        let _guard = store_lock(&store).unwrap();
        locked_tx.send(()).unwrap();
        // Dropping the sender also releases the lock if an assertion fails.
        let _ = wait.recv_timeout(Duration::from_secs(10));
    });
    locked_rx.await.unwrap();
    a.respond(&ac.id, true, ac.code.unwrap()).unwrap();
    b.respond(&bc.id, true, bc.code.unwrap()).unwrap();
    phase(&a, "committing").await;
    a.cancel(&ac.id).unwrap();
    closed(&a, &ac.id).await;
    release.send(()).unwrap();
    blocker.await.unwrap();
    timeout(Duration::from_secs(5), async {
        while !a.snapshot().trusted.contains(&peer) {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("A completed disk commit must be visible after cancellation");
    a.revoke(peer.clone()).await.unwrap();
    assert!(!a.snapshot().trusted.contains(&peer));
    assert!(a.snapshot().revoked.contains(&peer));
    a.stop().await;
    b.stop().await;
}

#[test]
fn a_delayed_trust_refresh_cannot_overwrite_a_newer_revocation_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::for_test(dir.path(), &random::<32>().unwrap()).unwrap();
    let peer = Identity::generate().unwrap().fingerprint().unwrap();
    store.trust(&peer).unwrap();
    let store = Arc::new(Mutex::new(store));
    let shared = make_shared(
        Arc::new(AtomicU16::new(0)),
        Arc::new(|_| {}),
        Duration::from_secs(10),
        dir.path().join("received"),
    );
    *lock(&shared.store) = Some(store.clone());
    let (read_tx, read_rx) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let delayed = shared.clone();
    let worker = std::thread::spawn(move || {
        TRUST_READ_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                read_tx.send(()).unwrap();
                wait.recv_timeout(Duration::from_secs(10)).unwrap();
            }));
        });
        refresh_trust(&delayed).unwrap();
    });
    read_rx.recv_timeout(Duration::from_secs(10)).unwrap();

    // If reading and publication can be separated, force a newer revoke to be
    // published in that gap. A serialized refresh must finish before mutation.
    let revoked_early = match store.try_lock() {
        Ok(mut policy) => {
            policy.revoke(&peer).unwrap();
            true
        }
        Err(std::sync::TryLockError::WouldBlock) => false,
        Err(error) => panic!("Unexpected policy lock error: {error}"),
    };
    if revoked_early {
        refresh_trust(&shared).unwrap();
    }
    release.send(()).unwrap();
    worker.join().unwrap();
    if !revoked_early {
        store_lock(&store).unwrap().revoke(&peer).unwrap();
        refresh_trust(&shared).unwrap();
    }
    let snapshot = lock(&shared.state).snapshot.clone();
    assert!(snapshot.trusted.is_empty());
    assert!(snapshot.revoked.contains(&peer));
}

async fn file_status(service: &Service, connection: &str, status: &str) -> TransferView {
    timeout(Duration::from_secs(12), async {
        loop {
            if let Some(transfer) = service
                .snapshot()
                .connections
                .iter()
                .find(|c| c.id == connection)
                .and_then(|c| c.transfer.as_ref())
                .filter(|t| t.status == status)
            {
                return transfer.clone();
            }
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("Expected file status {status}"))
}
fn no_received_files(service: &Service) {
    let inbox = &service.shared.receive_directory;
    assert!(!inbox.exists() || std::fs::read_dir(inbox).unwrap().next().is_none());
}
async fn raw_message(tls: &mut tokio_rustls::TlsStream<TcpStream>) -> Message {
    timeout(Duration::from_secs(5), async {
        loop {
            match protocol::receive(tls).await.unwrap() {
                Message::Ping {} => protocol::send(tls, &Message::Pong {}).await.unwrap(),
                Message::Pong {} => {}
                message => return message,
            }
        }
    })
    .await
    .expect("Raw TLS peer expected a control response")
}
async fn raw_connected(
    service: &Service,
) -> (tokio_rustls::TlsStream<TcpStream>, String, Connection) {
    let identity = Identity::generate().unwrap();
    let peer = identity.fingerprint().unwrap();
    store_lock(&get_store(&service.shared).unwrap())
        .unwrap()
        .trust(&peer)
        .unwrap();
    refresh_trust(&service.shared).unwrap();
    let connector = transport::connector(&identity, service.snapshot().identity).unwrap();
    let tcp = TcpStream::connect(addr(service)).await.unwrap();
    let mut tls: tokio_rustls::TlsStream<TcpStream> = connector
        .connect("invardrop.local".try_into().unwrap(), tcp)
        .await
        .unwrap()
        .into();
    protocol::send(
        &mut tls,
        &Message::Hello {
            version: 3,
            initiator: true,
            trusted: true,
        },
    )
    .await
    .unwrap();
    assert!(matches!(
        raw_message(&mut tls).await,
        Message::Hello {
            version: 3,
            trusted: true,
            ..
        }
    ));
    assert!(matches!(raw_message(&mut tls).await, Message::Ready {}));
    protocol::send(&mut tls, &Message::Ready {}).await.unwrap();
    let connection = phase(service, "connected").await;
    (tls, peer, connection)
}
async fn raw_offer(
    service: &Service,
    connection: &str,
    tls: &mut tokio_rustls::TlsStream<TcpStream>,
    size: u64,
) -> String {
    let id = uuid::Uuid::new_v4().to_string();
    protocol::send(
        tls,
        &Message::FileOffer {
            sender_name: "Test sender".into(),
            id: id.clone(),
            name: "incoming.bin".into(),
            size,
        },
    )
    .await
    .unwrap();
    let offer = file_status(service, connection, "awaiting_approval").await;
    assert!(offer.id == id);
    no_received_files(service);
    service.respond_file(connection, &id, true).unwrap();
    assert!(
        matches!(raw_message(tls).await, Message::FileAccept{id:accepted,accept:true} if accepted==id)
    );
    id
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_file_rejection_then_zero_and_many_chunks_preserve_original_and_connection() {
    let da = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let a = start(da.path(), Duration::from_secs(10)).await;
    let b = start(db.path(), Duration::from_secs(10)).await;
    let (ac, bc) = pair(&a, &b).await;
    let path = da.path().join("source.bin");
    std::fs::write(&path, b"decline this").unwrap();
    a.send_file(&ac.id, Source::open(&path).await.unwrap())
        .unwrap();
    let offer = file_status(&b, &bc.id, "awaiting_approval").await;
    assert!(a.can_send_file(&ac.id).is_err());
    assert!(b.respond_file(&bc.id, "stale-id", true).is_err());
    no_received_files(&b);
    b.respond_file(&bc.id, &offer.id, false).unwrap();
    file_status(&a, &ac.id, "rejected").await;
    no_received_files(&b);
    let mut receipts = Vec::new();
    // More than 64 binary chunks must not trip the keepalive/control limit.
    for bytes in [Vec::new(), vec![0xa5; files::CHUNK_SIZE * 70 + 19]] {
        std::fs::write(&path, &bytes).unwrap();
        a.can_send_file(&ac.id).unwrap();
        a.send_file(&ac.id, Source::open(&path).await.unwrap())
            .unwrap();
        let offer = file_status(&b, &bc.id, "awaiting_approval").await;
        assert!(offer.size == bytes.len() as u64);
        b.respond_file(&bc.id, &offer.id, true).unwrap();
        assert!(b.respond_file(&bc.id, &offer.id, true).is_err());
        file_status(&a, &ac.id, "complete").await;
        let receipt = file_status(&b, &bc.id, "complete").await;
        let received = PathBuf::from(receipt.path.unwrap());
        assert!(received.starts_with(&b.shared.receive_directory));
        assert!(std::fs::read(&received).unwrap() == bytes);
        assert!(std::fs::read(&path).unwrap() == bytes);
        receipts.push(received);
    }
    assert!(receipts[0] != receipts[1]);
    assert!(std::fs::read(&receipts[0]).unwrap().is_empty());
    assert!(
        a.snapshot()
            .connections
            .iter()
            .any(|c| c.id == ac.id && c.phase == "connected")
    );
    a.stop().await;
    b.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn file_bytes_before_local_acceptance_close_tls_without_creating_a_file() {
    let dir = tempfile::tempdir().unwrap();
    let s = start(dir.path(), Duration::from_secs(10)).await;
    let (mut tls, _, c) = raw_connected(&s).await;
    let id = uuid::Uuid::new_v4().to_string();
    protocol::send(
        &mut tls,
        &Message::FileOffer {
            sender_name: "Test sender".into(),
            id: id.clone(),
            name: "unapproved.bin".into(),
            size: 3,
        },
    )
    .await
    .unwrap();
    file_status(&s, &c.id, "awaiting_approval").await;
    protocol::send(
        &mut tls,
        &Message::FileChunk {
            id,
            data: b"abc".to_vec(),
        },
    )
    .await
    .unwrap();
    closed(&s, &c.id).await;
    assert!(
        s.snapshot()
            .connections
            .iter()
            .any(|entry| entry.id == c.id && entry.error == Some(Error::Protocol.to_string()))
    );
    no_received_files(&s);
    s.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wrong_hash_wrong_id_and_overrun_discard_partial_receipts() {
    for failure in ["hash", "id", "overrun", "short", "disconnect"] {
        let dir = tempfile::tempdir().unwrap();
        let s = start(dir.path(), Duration::from_secs(10)).await;
        let (mut tls, _, c) = raw_connected(&s).await;
        let id = raw_offer(&s, &c.id, &mut tls, 3).await;
        let sent_id = if failure == "id" {
            uuid::Uuid::new_v4().to_string()
        } else {
            id.clone()
        };
        let data = if failure == "overrun" {
            b"abcd".to_vec()
        } else if matches!(failure, "short" | "disconnect") {
            b"a".to_vec()
        } else {
            b"abc".to_vec()
        };
        protocol::send(&mut tls, &Message::FileChunk { id: sent_id, data })
            .await
            .unwrap();
        if matches!(failure, "hash" | "short") {
            protocol::send(
                &mut tls,
                &Message::FileFinish {
                    id,
                    hash: blake3::hash(if failure == "short" { b"a" } else { b"wrong" })
                        .to_hex()
                        .to_string(),
                },
            )
            .await
            .unwrap();
        }
        if failure == "disconnect" {
            // No close_notify or FileFinish: transport loss must discard staging.
            drop(tls);
        }
        closed(&s, &c.id).await;
        let transfer = file_status(&s, &c.id, "error").await;
        assert!(transfer.path.is_none());
        no_received_files(&s);
        s.stop().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_and_revoke_mid_receipt_remove_staging_and_never_publish_completion() {
    for revoke in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let s = start(dir.path(), Duration::from_secs(10)).await;
        let (mut tls, peer, c) = raw_connected(&s).await;
        let id = raw_offer(&s, &c.id, &mut tls, files::CHUNK_SIZE as u64 * 2).await;
        protocol::send(
            &mut tls,
            &Message::FileChunk {
                id,
                data: vec![3; files::CHUNK_SIZE],
            },
        )
        .await
        .unwrap();
        // Pong is processed after the chunk, so staging has received data.
        protocol::send(&mut tls, &Message::Ping {}).await.unwrap();
        timeout(Duration::from_secs(5), async {
            loop {
                if matches!(protocol::receive(&mut tls).await.unwrap(), Message::Pong {}) {
                    break;
                }
            }
        })
        .await
        .unwrap();
        if revoke {
            s.revoke(peer).await.unwrap();
        } else {
            s.cancel(&c.id).unwrap();
        }
        closed(&s, &c.id).await;
        let transfer = file_status(&s, &c.id, "error").await;
        assert!(transfer.path.is_none());
        no_received_files(&s);
        s.stop().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn busy_file_offer_is_rejected_without_replacing_the_pending_consent() {
    let dir = tempfile::tempdir().unwrap();
    let s = start(dir.path(), Duration::from_secs(10)).await;
    let (mut tls, _, c) = raw_connected(&s).await;
    let id = uuid::Uuid::new_v4().to_string();
    protocol::send(
        &mut tls,
        &Message::FileOffer {
            sender_name: "Test sender".into(),
            id: id.clone(),
            name: "first.bin".into(),
            size: 0,
        },
    )
    .await
    .unwrap();
    file_status(&s, &c.id, "awaiting_approval").await;
    let other = uuid::Uuid::new_v4().to_string();
    protocol::send(
        &mut tls,
        &Message::FileOffer {
            sender_name: "Test sender".into(),
            id: other.clone(),
            name: "second.bin".into(),
            size: 0,
        },
    )
    .await
    .unwrap();
    assert!(
        matches!(raw_message(&mut tls).await,Message::FileAccept{id:rejected,accept:false} if rejected==other)
    );
    assert!(file_status(&s, &c.id, "awaiting_approval").await.id == id);
    s.respond_file(&c.id, &id, false).unwrap();
    assert!(matches!(
        raw_message(&mut tls).await,
        Message::FileAccept { accept: false, .. }
    ));
    no_received_files(&s);
    s.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejected_offer_id_cannot_be_reused_for_different_metadata_or_stale_consent() {
    let dir = tempfile::tempdir().unwrap();
    let s = start(dir.path(), Duration::from_secs(10)).await;
    let (mut tls, _, c) = raw_connected(&s).await;
    let id = uuid::Uuid::new_v4().to_string();
    protocol::send(
        &mut tls,
        &Message::FileOffer {
            sender_name: "Test sender".into(),
            id: id.clone(),
            name: "first.bin".into(),
            size: 0,
        },
    )
    .await
    .unwrap();
    file_status(&s, &c.id, "awaiting_approval").await;
    s.respond_file(&c.id, &id, false).unwrap();
    assert!(matches!(
        raw_message(&mut tls).await,
        Message::FileAccept { accept: false, .. }
    ));
    protocol::send(
        &mut tls,
        &Message::FileOffer {
            sender_name: "Test sender".into(),
            id: id.clone(),
            name: "replacement.bin".into(),
            size: 42,
        },
    )
    .await
    .unwrap();
    closed(&s, &c.id).await;
    assert!(s.respond_file(&c.id, &id, true).is_err());
    let connection = s
        .snapshot()
        .connections
        .into_iter()
        .find(|entry| entry.id == c.id)
        .unwrap();
    assert!(connection.error == Some(Error::Protocol.to_string()));
    assert!(connection.transfer.unwrap().name == "first.bin");
    no_received_files(&s);
    s.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn incoming_request_expires_without_approval_or_receipt() {
    let dir = tempfile::tempdir().unwrap();
    let s = start(dir.path(), Duration::from_millis(500)).await;
    let (mut tls, _, c) = raw_connected(&s).await;
    let id = uuid::Uuid::new_v4().to_string();
    protocol::send(
        &mut tls,
        &Message::FileOffer {
            id: id.clone(),
            sender_name: "Kitchen laptop".into(),
            name: "pending.txt".into(),
            size: 3,
        },
    )
    .await
    .unwrap();
    let offer = file_status(&s, &c.id, "awaiting_approval").await;
    assert_eq!(offer.sender_name.as_deref(), Some("Kitchen laptop"));
    no_received_files(&s);
    closed(&s, &c.id).await;
    file_status(&s, &c.id, "expired").await;
    assert!(s.respond_file(&c.id, &id, true).is_err());
    no_received_files(&s);
    s.stop().await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unsafe_sender_name_is_not_published_as_a_request() {
    let dir = tempfile::tempdir().unwrap();
    let s = start(dir.path(), Duration::from_secs(10)).await;
    let (mut tls, _, c) = raw_connected(&s).await;
    protocol::send(
        &mut tls,
        &Message::FileOffer {
            id: uuid::Uuid::new_v4().to_string(),
            sender_name: "fake\u{202e}name".into(),
            name: "pending.txt".into(),
            size: 3,
        },
    )
    .await
    .unwrap();
    closed(&s, &c.id).await;
    assert!(
        s.snapshot()
            .connections
            .iter()
            .find(|e| e.id == c.id)
            .unwrap()
            .transfer
            .is_none()
    );
    no_received_files(&s);
    s.stop().await;
}
