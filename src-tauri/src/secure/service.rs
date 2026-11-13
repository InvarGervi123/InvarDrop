use super::{
    error::{Error, Result},
    protocol::{self, Message, Pairing},
    store::Store,
    transport,
};
use serde::Serialize;
use std::{
    collections::VecDeque,
    net::{Ipv4Addr, SocketAddr},
    path::PathBuf,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicU16, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{
    io::AsyncWriteExt,
    net::{TcpListener, TcpStream},
    sync::{Semaphore, mpsc, watch},
    task::JoinHandle,
    time::timeout,
};
use tokio_rustls::TlsStream;

const HANDSHAKE: Duration = Duration::from_secs(5);
const PAIRING: Duration = Duration::from_secs(90);
const MAX_SESSIONS: usize = 8;
const MAX_HISTORY: usize = 24;
pub const EVENT: &str = "security-updated";
type Notify = Arc<dyn Fn(Snapshot) + Send + Sync>;
type SharedStore = Arc<Mutex<Store>>;
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}
fn store_lock(s: &SharedStore) -> Result<MutexGuard<'_, Store>> {
    s.lock().map_err(|_| Error::Storage)
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Connection {
    pub id: String,
    pub phase: String,
    pub peer: Option<String>,
    pub code: Option<String>,
    pub local_approved: bool,
    pub can_reconnect: bool,
    pub error: Option<String>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub revision: u64,
    pub status: String,
    pub identity: Option<String>,
    pub trusted: Vec<String>,
    pub revoked: Vec<String>,
    pub connections: Vec<Connection>,
    pub error: Option<String>,
}
struct Entry {
    view: Connection,
    contact: Option<String>,
    target: Option<SocketAddr>,
    actions: mpsc::Sender<Action>,
    cancel: watch::Sender<bool>,
    rejected: bool,
}
struct State {
    snapshot: Snapshot,
    entries: VecDeque<Entry>,
}
struct Shared {
    state: Mutex<State>,
    store: Mutex<Option<SharedStore>>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
    slots: Arc<Semaphore>,
    stop: watch::Sender<bool>,
    notify: Notify,
    port: Arc<AtomicU16>,
    attempts: Mutex<(Instant, u32)>,
    pairing_duration: Duration,
}
pub struct Service {
    shared: Arc<Shared>,
    listener: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
}
impl Service {
    pub fn start(
        root: PathBuf,
        port: Arc<AtomicU16>,
        notify: impl Fn(Snapshot) + Send + Sync + 'static,
    ) -> Self {
        let shared = make_shared(port, Arc::new(notify), PAIRING);
        let s = shared.clone();
        let listener = tauri::async_runtime::spawn(async move {
            let loaded = tokio::task::spawn_blocking(move || Store::open(&root)).await;
            match loaded {
                Ok(Ok(store)) => {
                    if *s.stop.borrow() {
                        return;
                    }
                    if let Err(e) = listen(s.clone(), Arc::new(Mutex::new(store))).await {
                        update(&s, |st| {
                            st.snapshot.status = "unavailable".into();
                            st.snapshot.error = Some(e.to_string());
                        });
                    }
                }
                Ok(Err(error)) => update(&s, |st| {
                    st.snapshot.status = "unavailable".into();
                    st.snapshot.error = Some(error.to_string());
                }),
                Err(_) => update(&s, |st| {
                    st.snapshot.status = "unavailable".into();
                    st.snapshot.error = Some(Error::Storage.to_string());
                }),
            }
            s.port.store(0, Ordering::Release);
        });
        Self {
            shared,
            listener: Mutex::new(Some(listener)),
        }
    }
    pub fn snapshot(&self) -> Snapshot {
        lock(&self.shared.state).snapshot.clone()
    }
    pub fn connect(&self, contact: String, endpoints: Vec<SocketAddr>) -> Result<String> {
        let expected = {
            let state = lock(&self.shared.state);
            state
                .entries
                .iter()
                .rev()
                .find(|e| e.contact.as_ref() == Some(&contact) && e.view.can_reconnect)
                .and_then(|e| e.view.peer.clone())
        };
        spawn_connection(
            &self.shared,
            Input::Outgoing(endpoints, expected),
            Some(contact),
        )
    }
    pub fn reconnect(&self, id: &str) -> Result<String> {
        let (target, peer, contact) = {
            let state = lock(&self.shared.state);
            let entry = state
                .entries
                .iter()
                .find(|e| e.view.id == id)
                .ok_or(Error::NotReady)?;
            if !entry.view.can_reconnect || entry.view.phase != "closed" {
                return Err(Error::NotReady);
            }
            (
                entry.target.ok_or(Error::NotReady)?,
                entry.view.peer.clone().ok_or(Error::Identity)?,
                entry.contact.clone(),
            )
        };
        let store = get_store(&self.shared)?;
        if !store_lock(&store)?.is_trusted(&peer) {
            return Err(Error::Revoked);
        }
        spawn_connection(
            &self.shared,
            Input::Outgoing(vec![target], Some(peer)),
            contact,
        )
    }
    pub fn respond(&self, id: &str, approve: bool, code: String) -> Result<()> {
        transaction(&self.shared, |state| {
            let entry = state
                .entries
                .iter_mut()
                .find(|e| e.view.id == id)
                .ok_or(Error::Approval)?;
            if entry.view.phase != "verifying"
                || entry.view.local_approved
                || *entry.cancel.borrow()
                || (approve && entry.view.code.as_ref() != Some(&code))
            {
                return Err(Error::Approval);
            }
            entry
                .actions
                .try_send(Action { approve, code })
                .map_err(|_| Error::Approval)?;
            if approve {
                entry.view.local_approved = true;
            } else {
                entry.rejected = true;
                entry.cancel.send_replace(true);
                entry.view.code = None;
            }
            Ok(())
        })
    }
    pub fn cancel(&self, id: &str) -> Result<()> {
        let state = lock(&self.shared.state);
        let e = state
            .entries
            .iter()
            .find(|e| e.view.id == id)
            .ok_or(Error::NotReady)?;
        let _ = e.cancel.send(true);
        Ok(())
    }
    pub async fn revoke(&self, id: String) -> Result<()> {
        let store = get_store(&self.shared)?;
        let target = id.clone();
        let result = tokio::task::spawn_blocking(move || store_lock(&store)?.revoke(&target))
            .await
            .map_err(|_| Error::Storage)?;
        update(&self.shared, |state| {
            for e in &state.entries {
                if e.view.peer.as_ref() == Some(&id) {
                    let _ = e.cancel.send(true);
                }
            }
        });
        refresh_trust(&self.shared)?;
        result
    }
    pub async fn unblock(&self, id: String) -> Result<()> {
        let store = get_store(&self.shared)?;
        tokio::task::spawn_blocking(move || store_lock(&store)?.unblock(&id))
            .await
            .map_err(|_| Error::Storage)??;
        refresh_trust(&self.shared)
    }
    pub async fn stop(&self) {
        self.shared.port.store(0, Ordering::Release);
        self.shared.stop.send_replace(true);
        let listener = lock(&self.listener).take();
        if let Some(mut task) = listener
            && timeout(Duration::from_secs(3), &mut task).await.is_err()
        {
            task.abort();
            let _ = task.await;
        }
        let tasks = std::mem::take(&mut *lock(&self.shared.tasks));
        for mut task in tasks {
            if timeout(Duration::from_secs(2), &mut task).await.is_err() {
                task.abort();
                let _ = task.await;
            }
        }
        *lock(&self.shared.store) = None;
        self.shared.port.store(0, Ordering::Release);
        update(&self.shared, |st| st.snapshot.status = "stopped".into());
    }
}
fn make_shared(port: Arc<AtomicU16>, notify: Notify, pairing_duration: Duration) -> Arc<Shared> {
    let (stop, _) = watch::channel(false);
    Arc::new(Shared {
        state: Mutex::new(State {
            snapshot: Snapshot {
                revision: 0,
                status: "starting".into(),
                identity: None,
                trusted: vec![],
                revoked: vec![],
                connections: vec![],
                error: None,
            },
            entries: VecDeque::new(),
        }),
        store: Mutex::new(None),
        tasks: Mutex::new(vec![]),
        slots: Arc::new(Semaphore::new(MAX_SESSIONS)),
        stop,
        notify,
        port,
        attempts: Mutex::new((Instant::now(), 0)),
        pairing_duration,
    })
}
fn transaction<T>(s: &Shared, change: impl FnOnce(&mut State) -> Result<T>) -> Result<T> {
    let (result, snapshot) = {
        let mut state = lock(&s.state);
        let result = change(&mut state)?;
        state.snapshot.revision = state.snapshot.revision.saturating_add(1);
        state.snapshot.connections = state.entries.iter().map(|e| e.view.clone()).collect();
        (result, state.snapshot.clone())
    };
    (s.notify)(snapshot);
    Ok(result)
}
fn update(s: &Shared, change: impl FnOnce(&mut State)) {
    let _ = transaction(s, |st| {
        change(st);
        Ok(())
    });
}
fn view(s: &Shared, id: &str, change: impl FnOnce(&mut Entry)) {
    update(s, |state| {
        if let Some(entry) = state.entries.iter_mut().find(|e| e.view.id == id) {
            change(entry);
        }
    });
}
fn get_store(s: &Shared) -> Result<SharedStore> {
    lock(&s.store).clone().ok_or(Error::NotReady)
}
fn refresh_trust(s: &Shared) -> Result<()> {
    let store = get_store(s)?;
    let store = store_lock(&store)?;
    transaction(s, move |state| {
        // Serialize reading durable policy with publication. Otherwise a delayed
        // refresh can publish an older trust list with a newer snapshot revision.
        let trusted = store.trusted();
        let revoked = store.revoked();
        #[cfg(test)]
        tests::after_trust_read();
        state.snapshot.trusted = trusted;
        state.snapshot.revoked = revoked;
        // Release policy before transaction invokes the external notifier.
        drop(store);
        Ok(())
    })
}
fn listener_socket() -> Result<std::net::TcpListener> {
    use socket2::{Domain, Protocol, Socket, Type};
    let v6 = (|| -> std::io::Result<_> {
        let socket = Socket::new(Domain::IPV6, Type::STREAM, Some(Protocol::TCP))?;
        socket.set_only_v6(false)?;
        socket.set_nonblocking(true)?;
        socket.bind(
            &"[::]:0"
                .parse::<SocketAddr>()
                .map_err(std::io::Error::other)?
                .into(),
        )?;
        socket.listen(64)?;
        Ok(socket.into())
    })();
    v6.or_else(|_| {
        let l = std::net::TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0))?;
        l.set_nonblocking(true)?;
        Ok(l)
    })
    .map_err(|_: std::io::Error| Error::Network)
}
async fn listen(s: Arc<Shared>, store: SharedStore) -> Result<()> {
    let mut stop = s.stop.subscribe();
    if *stop.borrow() {
        return Ok(());
    }
    let listener = TcpListener::from_std(listener_socket()?).map_err(|_| Error::Network)?;
    let port = listener.local_addr().map_err(|_| Error::Network)?.port();
    let identity = store_lock(&store)?.identity().fingerprint()?;
    *lock(&s.store) = Some(store);
    refresh_trust(&s)?;
    s.port.store(port, Ordering::Release);
    update(&s, |st| {
        st.snapshot.identity = Some(identity);
        st.snapshot.status = "ready".into();
        st.snapshot.error = None;
    });
    loop {
        tokio::select! {
            _=stop.changed()=>return Ok(()),
            accepted=listener.accept()=>{
                let (socket,_)=accepted.map_err(|_|Error::Network)?;
                let _=spawn_connection(&s,Input::Incoming(socket),None);
            }
        }
    }
}
enum Input {
    Incoming(TcpStream),
    Outgoing(Vec<SocketAddr>, Option<String>),
}
struct Action {
    approve: bool,
    code: String,
}
fn spawn_connection(s: &Arc<Shared>, input: Input, contact: Option<String>) -> Result<String> {
    if *s.stop.borrow() || s.port.load(Ordering::Acquire) == 0 {
        return Err(Error::NotReady);
    }
    {
        let mut attempts = lock(&s.attempts);
        if attempts.0.elapsed() >= Duration::from_secs(60) {
            *attempts = (Instant::now(), 0);
        }
        if attempts.1 >= 30 {
            return Err(Error::Limit);
        }
        attempts.1 += 1;
    }
    let permit = s
        .slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| Error::Limit)?;
    let id = uuid::Uuid::new_v4().to_string();
    let (tx, rx) = mpsc::channel(2);
    let (cancel, cancel_rx) = watch::channel(false);
    update(s, |st| {
        if st.entries.len() >= MAX_HISTORY
            && let Some(i) = st.entries.iter().position(|e| e.view.phase == "closed")
        {
            st.entries.remove(i);
        }
        st.entries.push_back(Entry {
            view: Connection {
                id: id.clone(),
                phase: "connecting".into(),
                peer: None,
                code: None,
                local_approved: false,
                can_reconnect: false,
                error: None,
            },
            contact,
            target: None,
            actions: tx,
            cancel,
            rejected: false,
        });
    });
    let shared = s.clone();
    let task_id = id.clone();
    let stop = s.stop.subscribe();
    let task = tokio::spawn(async move {
        let _permit = permit;
        let result = connection(shared.clone(), &task_id, input, rx, cancel_rx, stop).await;
        view(&shared, &task_id, |e| {
            e.view.phase = "closed".into();
            e.view.code = None;
            e.view.error = if e.rejected {
                Some(Error::Rejected.to_string())
            } else {
                result.err().map(|e| e.to_string())
            };
        });
    });
    let mut tasks = lock(&s.tasks);
    tasks.retain(|t| !t.is_finished());
    tasks.push(task);
    Ok(id)
}
async fn connection(
    s: Arc<Shared>,
    id: &str,
    input: Input,
    actions: mpsc::Receiver<Action>,
    mut cancel: watch::Receiver<bool>,
    mut stop: watch::Receiver<bool>,
) -> Result<()> {
    if *stop.borrow() || *cancel.borrow() {
        return Err(Error::Cancelled);
    }
    let store = get_store(&s)?;
    let initiator = matches!(&input, Input::Outgoing(..));
    let mut target = None;
    let handshake = async {
        let stream = match input {
            Input::Incoming(tcp) => {
                let acceptor = { transport::acceptor(store_lock(&store)?.identity())? };
                TlsStream::Server(acceptor.accept(tcp).await.map_err(|_| Error::Identity)?)
            }
            Input::Outgoing(addresses, expected) => {
                let connector = { transport::connector(store_lock(&store)?.identity(), expected)? };
                let mut socket = None;
                for address in addresses.into_iter().take(8) {
                    if let Ok(Ok(tcp)) =
                        timeout(Duration::from_millis(500), TcpStream::connect(address)).await
                    {
                        target = Some(address);
                        socket = Some(tcp);
                        break;
                    }
                }
                let socket = socket.ok_or(Error::Network)?;
                TlsStream::Client(
                    connector
                        .connect(
                            rustls::pki_types::ServerName::try_from("invardrop.local")
                                .map_err(|_| Error::Identity)?,
                            socket,
                        )
                        .await
                        .map_err(|_| Error::Identity)?,
                )
            }
        };
        Ok(stream)
    };
    let mut stream = tokio::select! {r=timeout(HANDSHAKE,handshake)=>r.map_err(|_|Error::Expired)??,_=cancel.changed()=>return Err(Error::Cancelled),_=stop.changed()=>return Err(Error::Cancelled)};
    let local = store_lock(&store)?.identity().fingerprint()?;
    let (peer, code) = transport::peer_and_code(&stream, &local, initiator)?;
    let trusted = {
        let store = store_lock(&store)?;
        if store.is_revoked(&peer) {
            return Err(Error::Revoked);
        }
        store.is_trusted(&peer)
    };
    let hello = async {
        protocol::send(
            &mut stream,
            &Message::Hello {
                version: 1,
                initiator,
                trusted,
            },
        )
        .await?;
        match protocol::receive(&mut stream).await? {
            Message::Hello {
                version: 1,
                initiator: other,
                trusted: remote,
            } if other != initiator && remote == trusted => Ok(()),
            _ => Err(Error::Identity),
        }
    };
    tokio::select! {r=timeout(HANDSHAKE,hello)=>r.map_err(|_|Error::Expired)??,_=cancel.changed()=>return Err(Error::Cancelled),_=stop.changed()=>return Err(Error::Cancelled)};
    view(&s, id, |e| {
        e.view.peer = Some(peer.clone());
        e.target = target;
    });
    let (mut reader, mut writer) = tokio::io::split(stream);
    let (messages_tx, mut messages) = mpsc::channel(4);
    let reader_task = ReaderTask(tokio::spawn(async move {
        loop {
            let result = protocol::receive(&mut reader).await;
            let failed = result.is_err();
            if messages_tx.send(result).await.is_err() || failed {
                break;
            }
        }
    }));
    let result = tokio::select! {
        r=application(&s,id,Proof {peer:&peer,code:&code,trusted},&mut writer,&mut messages,actions)=>r,
        _=cancel.changed()=>Err(Error::Cancelled),
        _=stop.changed()=>Err(Error::Cancelled),
    };
    let _ = timeout(Duration::from_secs(1), writer.shutdown()).await;
    drop(reader_task);
    result
}
struct ReaderTask(JoinHandle<()>);
impl Drop for ReaderTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}
async fn next(messages: &mut mpsc::Receiver<Result<Message>>) -> Result<Message> {
    messages.recv().await.ok_or(Error::Network)?
}
struct Proof<'a> {
    peer: &'a str,
    code: &'a str,
    trusted: bool,
}
async fn application<W: tokio::io::AsyncWrite + Unpin>(
    s: &Arc<Shared>,
    id: &str,
    proof: Proof<'_>,
    writer: &mut W,
    messages: &mut mpsc::Receiver<Result<Message>>,
    mut actions: mpsc::Receiver<Action>,
) -> Result<()> {
    let Proof {
        peer,
        code,
        trusted,
    } = proof;
    if !trusted {
        view(s, id, |e| {
            e.view.phase = "verifying".into();
            e.view.code = Some(code.into());
        });
        let mut pairing = Pairing::new(Instant::now(), s.pairing_duration);
        let expiry = tokio::time::sleep(s.pairing_duration);
        tokio::pin!(expiry);
        while !pairing.approved() {
            tokio::select! {
                _=&mut expiry=>return Err(Error::Expired),
                action=actions.recv()=>{
                    let action=action.ok_or(Error::Cancelled)?;
                    let result=pairing.local(action.approve,&action.code,code,Instant::now());
                    protocol::send(writer,&Message::Decision{approve:result.is_ok()}).await?;
                    result?;view(s,id,|e|e.view.local_approved=true);
                }
                message=next(messages)=>match message? {
                    Message::Decision{approve}=>pairing.remote(approve,Instant::now())?,
                    _=>return Err(Error::Protocol),
                }
            }
        }
        transaction(s, |state| {
            let entry = state
                .entries
                .iter_mut()
                .find(|e| e.view.id == id)
                .ok_or(Error::Cancelled)?;
            if *entry.cancel.borrow() {
                return Err(Error::Cancelled);
            }
            entry.view.phase = "committing".into();
            entry.view.code = None;
            Ok(())
        })?;
        let store = get_store(s)?;
        let peer = peer.to_owned();
        let shared = s.clone();
        tokio::task::spawn_blocking(move || {
            let result = store_lock(&store)?.trust(&peer);
            // A filesystem commit can outlive connection cancellation. Publish its
            // result from the same task so saved trust remains visible/revocable.
            let refreshed = refresh_trust(&shared);
            result.and(refreshed)
        })
        .await
        .map_err(|_| Error::Storage)??;
    }
    let store = get_store(s)?;
    if !store_lock(&store)?.is_trusted(peer) {
        return Err(Error::Revoked);
    }
    protocol::send(writer, &Message::Ready {}).await?;
    match timeout(HANDSHAKE, next(messages))
        .await
        .map_err(|_| Error::Expired)??
    {
        Message::Ready {} => {}
        _ => return Err(Error::Protocol),
    }
    view(s, id, |e| {
        e.view.phase = "connected".into();
        e.view.code = None;
        e.view.can_reconnect = e.target.is_some();
    });
    let mut tick = tokio::time::interval(Duration::from_secs(5));
    let mut last_received = Instant::now();
    let mut window = Instant::now();
    let mut received = 0;
    loop {
        if !store_lock(&store)?.is_trusted(peer) {
            return Err(Error::Revoked);
        }
        tokio::select! {
            _=tick.tick()=>protocol::send(writer,&Message::Ping{}).await?,
            _=tokio::time::sleep_until((last_received+Duration::from_secs(20)).into())=>return Err(Error::Expired),
            message=next(messages)=>{
                if window.elapsed()>=Duration::from_secs(60){window=Instant::now();received=0;}
                received+=1;if received>64{return Err(Error::Limit);}
                last_received=Instant::now();
                match message? {
                    Message::Ping{}=>protocol::send(writer,&Message::Pong{}).await?,
                    Message::Pong{}=>{},
                    _=>return Err(Error::Protocol),
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
