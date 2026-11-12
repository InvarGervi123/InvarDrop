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

async fn start(root: &Path, duration: Duration) -> Service {
    start_with_master(root, duration, &random::<32>().unwrap()).await
}
async fn start_with_master(root: &Path, duration: Duration, master: &[u8]) -> Service {
    let store = Store::for_test(root, master).unwrap();
    let shared = make_shared(Arc::new(AtomicU16::new(0)), Arc::new(|_| {}), duration);
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
            version: 1,
            initiator: true,
            trusted: false,
        },
    )
    .await
    .unwrap();
    assert!(matches!(
        protocol::receive(&mut tls).await.unwrap(),
        Message::Hello { version: 1, .. }
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
