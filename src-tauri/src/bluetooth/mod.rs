//! Bluetooth discovery is unauthenticated. Only the shared TLS actor can create
//! trust or authorize payload. Platform routes and socket handles stay in Rust.
#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "linux")]
mod linux;
mod protocol;
#[cfg(target_os = "android")]
pub(crate) use android::init as android_plugin;

use crate::secure::{
    error::{Error, Result},
    stream::Stream,
};
use protocol::Bootstrap;
use serde::Serialize;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, oneshot, watch};

pub(crate) const EVENT: &str = "bluetooth-updated";
#[cfg(any(target_os = "linux", test))]
pub(crate) const DISCOVERY_UUID: &str = "af8e6b90-6f6e-4e34-8a9e-4356fe9c3601";
#[cfg(any(target_os = "linux", test))]
pub(crate) const BOOTSTRAP_UUID: &str = "af8e6b90-6f6e-4e34-8a9e-4356fe9c3602";
#[cfg(any(target_os = "linux", test))]
pub(crate) const REQUEST_UUID: &str = "af8e6b90-6f6e-4e34-8a9e-4356fe9c3603";
const MAX_DEVICES: usize = 32;
const MAX_AGE: Duration = Duration::from_secs(120);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Device {
    pub id: String,
    pub name: String,
    pub can_connect: bool,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Snapshot {
    pub supported: bool,
    pub revision: u64,
    pub status: String,
    pub error: Option<String>,
    pub devices: Vec<Device>,
}
#[derive(Clone)]
struct Route {
    info: Bootstrap,
    native_id: String,
    observed: Instant,
}
impl Route {
    fn can_connect(&self) -> bool {
        // Android can dial a desktop Classic locator. Android-to-Android
        // routing is unavailable on Bluetooth when neither peer publishes one.
        !cfg!(target_os = "android") || self.info.address.is_some()
    }

    fn is_connectable(&self) -> bool {
        self.observed.elapsed() < MAX_AGE && self.can_connect()
    }
}
pub(crate) struct Incoming {
    pub stream: Stream,
}
enum Command {
    Connect {
        route: Route,
        reply: oneshot::Sender<Result<Stream>>,
    },
    Scan,
}
struct State {
    snapshot: Snapshot,
    routes: BTreeMap<String, Route>,
}
pub(crate) struct Controller {
    local: Bootstrap,
    state: Mutex<State>,
    notify: Arc<dyn Fn(Snapshot) + Send + Sync>,
    observations: watch::Sender<()>,
    commands: Mutex<Option<mpsc::Sender<Command>>>,
    stop: Mutex<Option<watch::Sender<bool>>>,
    task: tokio::sync::Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
    incoming: mpsc::Sender<Incoming>,
    receiver: Mutex<Option<mpsc::Receiver<Incoming>>>,
    #[cfg(target_os = "android")]
    app: tauri::AppHandle,
}
impl Controller {
    pub(crate) fn new(
        id: String,
        name: String,
        #[cfg(target_os = "android")] app: tauri::AppHandle,
        notify: impl Fn(Snapshot) + Send + Sync + 'static,
    ) -> Arc<Self> {
        let (incoming, receiver) = mpsc::channel(8);
        Arc::new(Self {
            local: Bootstrap {
                version: 1,
                id,
                name,
                service: uuid::Uuid::new_v4().to_string(),
                address: None,
            },
            state: Mutex::new(State {
                snapshot: Snapshot {
                    supported: cfg!(any(target_os = "linux", target_os = "android")),
                    revision: 0,
                    status: "disabled".into(),
                    error: None,
                    devices: vec![],
                },
                routes: BTreeMap::new(),
            }),
            notify: Arc::new(notify),
            observations: watch::channel(()).0,
            commands: Mutex::new(None),
            stop: Mutex::new(None),
            task: tokio::sync::Mutex::new(None),
            incoming,
            receiver: Mutex::new(Some(receiver)),
            #[cfg(target_os = "android")]
            app,
        })
    }
    #[cfg(all(test, not(target_os = "android")))]
    pub(crate) fn test_stream_route(
        &self,
        peer_id: String,
        stream: Stream,
    ) -> oneshot::Receiver<()> {
        self.observe(
            Bootstrap {
                version: 1,
                id: peer_id,
                name: "Test nearby peer".into(),
                service: uuid::Uuid::new_v4().to_string(),
                address: None,
            },
            "test-route".into(),
        );
        let (commands, mut requests) = mpsc::channel(1);
        *lock(&self.commands) = Some(commands);
        let (requested, observed) = oneshot::channel();
        tokio::spawn(async move {
            if let Some(Command::Connect { reply, .. }) = requests.recv().await {
                let _ = requested.send(());
                let _ = reply.send(Ok(stream));
            }
        });
        observed
    }

    pub(crate) fn snapshot(&self) -> Snapshot {
        lock(&self.state).snapshot.clone()
    }
    pub(crate) fn take_incoming(&self) -> Option<mpsc::Receiver<Incoming>> {
        lock(&self.receiver).take()
    }
    pub(crate) fn has_peer(&self, id: &str) -> bool {
        lock(&self.state)
            .routes
            .get(id)
            .is_some_and(Route::is_connectable)
    }
    pub(crate) fn can_discover(&self) -> bool {
        matches!(
            lock(&self.state).snapshot.status.as_str(),
            "ready" | "scanning"
        )
    }
    pub(crate) async fn find_peer(&self, id: &str) -> bool {
        let mut observed = self.observations.subscribe();
        if self.has_peer(id) {
            return true;
        }
        if !self.can_discover() {
            return false;
        }
        self.scan();
        tokio::time::timeout(Duration::from_secs(8), async {
            while observed.changed().await.is_ok() {
                if self.has_peer(id) {
                    return true;
                }
                if !self.can_discover() {
                    return false;
                }
            }
            false
        })
        .await
        .unwrap_or(false)
    }
    fn status(&self, status: &str, error: Option<String>) {
        let snapshot = {
            let mut state = lock(&self.state);
            if state.snapshot.status == status && state.snapshot.error == error {
                return;
            }
            state.snapshot.status = status.into();
            state.snapshot.error = error;
            if matches!(status, "disabled" | "unavailable") {
                state.routes.clear();
                state.snapshot.devices.clear();
            }
            state.snapshot.revision += 1;
            state.snapshot.clone()
        };
        self.observations.send_replace(());
        (self.notify)(snapshot);
    }
    fn observe(&self, info: Bootstrap, native_id: String) {
        if info.validate().is_err() || info.id == self.local.id {
            return;
        }
        let snapshot = {
            let mut state = lock(&self.state);
            state.routes.retain(|_, r| r.observed.elapsed() < MAX_AGE);
            if state.routes.len() >= MAX_DEVICES && !state.routes.contains_key(&info.id) {
                return;
            }
            state.routes.insert(
                info.id.clone(),
                Route {
                    info,
                    native_id,
                    observed: Instant::now(),
                },
            );
            self.observations.send_replace(());
            let devices: Vec<_> = state
                .routes
                .values()
                .map(|r| Device {
                    id: r.info.id.clone(),
                    name: r.info.name.clone(),
                    can_connect: r.can_connect(),
                })
                .collect();
            if state.snapshot.devices == devices {
                return;
            }
            state.snapshot.devices = devices;
            state.snapshot.revision += 1;
            state.snapshot.clone()
        };
        (self.notify)(snapshot);
    }
    fn expire(&self) {
        let snapshot = {
            let mut state = lock(&self.state);
            let count = state.routes.len();
            state.routes.retain(|_, r| r.observed.elapsed() < MAX_AGE);
            if count == state.routes.len() {
                return;
            }
            let ids = state.routes.keys().cloned().collect::<Vec<_>>();
            state.snapshot.devices.retain(|d| ids.contains(&d.id));
            state.snapshot.revision += 1;
            state.snapshot.clone()
        };
        (self.notify)(snapshot);
    }
    pub(crate) fn scan(&self) {
        if let Some(tx) = lock(&self.commands).as_ref() {
            let _ = tx.try_send(Command::Scan);
        }
    }
    pub(crate) async fn enable(
        self: &Arc<Self>,
        request_permission: bool,
    ) -> std::result::Result<(), String> {
        if !self.snapshot().supported {
            return Ok(());
        }
        let mut task = self.task.lock().await;
        if task.as_ref().is_some_and(|t| !t.inner().is_finished()) {
            if request_permission {
                self.scan();
            }
            return Ok(());
        }
        #[cfg(target_os = "android")]
        if !android::permission(&self.app, request_permission).await? {
            return Ok(());
        }
        let (tx, rx) = mpsc::channel(4);
        let (stop, stopped) = watch::channel(false);
        *lock(&self.commands) = Some(tx);
        *lock(&self.stop) = Some(stop);
        self.status("starting", None);
        let shared = self.clone();
        *task = Some(tauri::async_runtime::spawn(async move {
            #[cfg(target_os = "linux")]
            let result = linux::run(shared.clone(), rx, stopped).await;
            #[cfg(target_os = "android")]
            let result = android::run(shared.clone(), rx, stopped).await;
            #[cfg(not(any(target_os = "linux", target_os = "android")))]
            let result = {
                drop((rx, stopped));
                Err("Bluetooth support is not yet available on this platform".to_string())
            };
            if let Err(error) = result {
                shared.status("unavailable", Some(error));
            }
        }));
        Ok(())
    }
    pub(crate) async fn disable(&self) {
        let mut slot = self.task.lock().await;
        if let Some(stop) = lock(&self.stop).take() {
            stop.send_replace(true);
        }
        *lock(&self.commands) = None;
        if let Some(mut task) = slot.take()
            && tokio::time::timeout(Duration::from_secs(5), &mut task)
                .await
                .is_err()
        {
            task.abort();
            let _ = task.await;
        }
        self.status("disabled", None);
    }
    pub(crate) async fn connect(&self, id: &str) -> Result<Stream> {
        let route = lock(&self.state)
            .routes
            .get(id)
            .filter(|r| r.is_connectable())
            .cloned()
            .ok_or(Error::BluetoothUnavailable)?;
        let tx = lock(&self.commands)
            .clone()
            .ok_or(Error::BluetoothUnavailable)?;
        let (reply, response) = oneshot::channel();
        tx.try_send(Command::Connect { route, reply })
            .map_err(|_| Error::Limit)?;
        tokio::time::timeout(Duration::from_secs(45), response)
            .await
            .map_err(|_| Error::BluetoothTimeout)?
            .map_err(|_| Error::BluetoothUnavailable)?
    }
}
