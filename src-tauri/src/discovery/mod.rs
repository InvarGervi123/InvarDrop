//! Unauthenticated LAN presence only. Never use discovery IDs as pairing credentials.
mod protocol;
mod tracker;

use mdns_sd::{DaemonEvent, Receiver, ServiceDaemon, ServiceEvent};
use serde::Serialize;
use std::{
    net::SocketAddr,
    sync::atomic::{AtomicU16, Ordering},
    sync::{Arc, Mutex, MutexGuard, mpsc},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use tracker::Tracker;

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(20);
const RETRY_INTERVAL: Duration = Duration::from_secs(5);
const POLL_INTERVAL: Duration = Duration::from_millis(250);
const ANNOUNCE_TIMEOUT: Duration = Duration::from_secs(10);
pub const EVENT_NAME: &str = "discovery-updated";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    pub id: String,
    pub name: String,
    pub can_connect: bool,
    #[serde(skip)]
    pub endpoints: Vec<SocketAddr>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Starting,
    Searching,
    Unavailable,
    Stopped,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub revision: u64,
    pub local_device: Device,
    pub status: Status,
    pub devices: Vec<Device>,
}

type Notify = Arc<dyn Fn(Snapshot) + Send + Sync>;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Owned by the app; one worker plus the library's mDNS event-loop thread.
/// Networking never runs on the GUI thread, except bounded shutdown waits at exit.
pub struct Discovery {
    state: Arc<Mutex<Snapshot>>,
    stop: mpsc::Sender<()>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl Discovery {
    pub fn start(notify: impl Fn(Snapshot) + Send + Sync + 'static) -> Self {
        Self::start_with_port(Arc::new(AtomicU16::new(0)), notify)
    }

    pub fn start_with_port(
        port: Arc<AtomicU16>,
        notify: impl Fn(Snapshot) + Send + Sync + 'static,
    ) -> Self {
        let state = Arc::new(Mutex::new(Snapshot {
            revision: 0,
            local_device: protocol::local_device(),
            status: Status::Starting,
            devices: Vec::new(),
        }));
        let (stop, receiver) = mpsc::channel();
        let worker_state = state.clone();
        let worker = thread::Builder::new()
            .name("invardrop-discovery".into())
            .spawn(move || run(worker_state, receiver, port, Arc::new(notify)));
        let worker = match worker {
            Ok(worker) => Some(worker),
            Err(error) => {
                eprintln!("Could not start discovery worker: {error}");
                let mut snapshot = lock(&state);
                snapshot.status = Status::Unavailable;
                snapshot.revision += 1;
                None
            }
        };
        Self {
            state,
            stop,
            worker: Mutex::new(worker),
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        lock(&self.state).clone()
    }

    pub fn stop(&self) {
        let _ = self.stop.send(());
        if let Some(worker) = lock(&self.worker).take()
            && worker.join().is_err()
        {
            eprintln!("Discovery worker exited unexpectedly");
        }
    }
}

impl Drop for Discovery {
    fn drop(&mut self) {
        self.stop();
    }
}

fn publish(state: &Mutex<Snapshot>, status: Status, devices: Vec<Device>, notify: &Notify) {
    let snapshot = {
        let mut current = lock(state);
        if current.status == status && current.devices == devices {
            return;
        }
        current.status = status;
        current.devices = devices;
        current.revision = current.revision.saturating_add(1);
        current.clone()
    };
    notify(snapshot);
}

struct Session {
    daemon: ServiceDaemon,
    services: Receiver<ServiceEvent>,
    monitor: Receiver<DaemonEvent>,
    fullname: String,
}

impl Session {
    fn new(local: &Device, heartbeat: u64, port: u16) -> mdns_sd::Result<Self> {
        let daemon = ServiceDaemon::new()?;
        let initialize = || {
            let monitor = daemon.monitor()?;
            daemon.set_ip_check_interval(2)?;
            let info = protocol::announcement(local, heartbeat, port)?;
            let fullname = info.get_fullname().to_owned();
            daemon.register(info)?;
            let services = daemon.browse(protocol::SERVICE_TYPE)?;
            Ok((monitor, fullname, services))
        };
        match initialize() {
            Ok((monitor, fullname, services)) => Ok(Self {
                daemon,
                services,
                monitor,
                fullname,
            }),
            Err(error) => {
                shutdown_daemon(&daemon);
                Err(error)
            }
        }
    }
}

fn shutdown_daemon(daemon: &ServiceDaemon) {
    // Full queues are transient; don't abandon a daemon thread on Error::Again.
    for _ in 0..4 {
        match daemon.shutdown() {
            Ok(done) => {
                if done.recv_timeout(Duration::from_secs(2)).is_err() {
                    eprintln!("mDNS shutdown acknowledgement timed out");
                }
                return;
            }
            Err(mdns_sd::Error::Again) => thread::sleep(Duration::from_millis(50)),
            Err(error) => {
                eprintln!("mDNS shutdown: {error}");
                return;
            }
        }
    }
    eprintln!("mDNS shutdown command queue remained full");
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.daemon.stop_browse(protocol::SERVICE_TYPE);
        if let Ok(done) = self.daemon.unregister(&self.fullname) {
            let _ = done.recv_timeout(Duration::from_secs(1));
        }
        shutdown_daemon(&self.daemon);
    }
}

fn announcement_expired(last: Instant, announced: bool, now: Instant) -> bool {
    let limit = if announced {
        HEARTBEAT_INTERVAL + ANNOUNCE_TIMEOUT
    } else {
        ANNOUNCE_TIMEOUT
    };
    now.saturating_duration_since(last) >= limit
}

fn run(
    state: Arc<Mutex<Snapshot>>,
    stop: mpsc::Receiver<()>,
    port: Arc<AtomicU16>,
    notify: Notify,
) {
    let local = lock(&state).local_device.clone();
    // Keep heartbeats monotonic across daemon restarts within this application session.
    let mut heartbeat = 0_u64;
    loop {
        match stop.try_recv() {
            Ok(()) | Err(mpsc::TryRecvError::Disconnected) => break,
            Err(mpsc::TryRecvError::Empty) => {}
        }
        let advertised_port = port.load(Ordering::Acquire);
        let result = Session::new(&local, heartbeat, advertised_port).and_then(|session| {
            let mut tracker = Tracker::new(local.id.clone());
            let mut last_heartbeat = Instant::now();
            let mut last_port = advertised_port;
            let mut last_announcement = last_heartbeat;
            let mut announced = false;
            publish(&state, Status::Starting, Vec::new(), &notify);
            loop {
                match stop.recv_timeout(POLL_INTERVAL) {
                    Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
                for event in session.monitor.try_iter().take(128) {
                    match event {
                        DaemonEvent::Error(error) => return Err(error),
                        DaemonEvent::Announce(_, _) => {
                            announced = true;
                            last_announcement = Instant::now();
                        }
                        _ => {}
                    }
                }
                if session.monitor.is_disconnected() || session.services.is_disconnected() {
                    return Err(mdns_sd::Error::DaemonShutdown);
                }
                let now = Instant::now();
                for event in session.services.try_iter().take(128) {
                    match event {
                        ServiceEvent::ServiceResolved(service) => {
                            if let Some((device, beat)) = protocol::parse(&service) {
                                tracker.observe(service.get_fullname(), device, beat, now);
                            }
                        }
                        ServiceEvent::ServiceRemoved(_, fullname) => tracker.remove(&fullname),
                        ServiceEvent::SearchStopped(_) => {
                            return Err(mdns_sd::Error::DaemonShutdown);
                        }
                        _ => {}
                    }
                }
                tracker.expire(now);
                // mdns-sd can stay Running even when all multicast sockets fail.
                // Require successful announcements, not merely a live daemon thread.
                if announcement_expired(last_announcement, announced, now) {
                    return Err(mdns_sd::Error::Msg("No usable multicast interface".into()));
                }
                let status = if announced {
                    Status::Searching
                } else {
                    Status::Starting
                };
                publish(&state, status, tracker.devices(), &notify);
                if now.duration_since(last_heartbeat) >= HEARTBEAT_INTERVAL
                    || last_port != port.load(Ordering::Acquire)
                {
                    heartbeat = heartbeat.wrapping_add(1);
                    session.daemon.register(protocol::announcement(
                        &local,
                        heartbeat,
                        port.load(Ordering::Acquire),
                    )?)?;
                    last_heartbeat = now;
                    last_port = port.load(Ordering::Acquire);
                }
            }
        });
        match result {
            Ok(()) => break,
            Err(error) => {
                eprintln!("LAN discovery temporarily unavailable: {error}");
                publish(&state, Status::Unavailable, Vec::new(), &notify);
                match stop.recv_timeout(RETRY_INTERVAL) {
                    Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
                heartbeat = heartbeat.wrapping_add(1);
            }
        }
    }
    publish(&state, Status::Stopped, Vec::new(), &notify);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn announcement_watchdog_detects_startup_and_later_network_failure() {
        let now = Instant::now();
        assert!(!announcement_expired(
            now,
            false,
            now + ANNOUNCE_TIMEOUT - Duration::from_millis(1)
        ));
        assert!(announcement_expired(now, false, now + ANNOUNCE_TIMEOUT));
        assert!(!announcement_expired(now, true, now + HEARTBEAT_INTERVAL));
        assert!(announcement_expired(
            now,
            true,
            now + HEARTBEAT_INTERVAL + ANNOUNCE_TIMEOUT
        ));
    }
    #[test]
    fn snapshots_only_advance_revision_for_visible_changes() {
        let state = Mutex::new(Snapshot {
            revision: 0,
            local_device: protocol::local_device(),
            status: Status::Starting,
            devices: Vec::new(),
        });
        let updates = Arc::new(Mutex::new(Vec::new()));
        let observed = updates.clone();
        let notify: Notify = Arc::new(move |s| lock(&observed).push(s));
        publish(&state, Status::Searching, Vec::new(), &notify);
        publish(&state, Status::Searching, Vec::new(), &notify);
        publish(&state, Status::Unavailable, Vec::new(), &notify);
        let updates = lock(&updates);
        assert_eq!(updates.len(), 2);
        assert_eq!(updates[0].revision, 1);
        assert_eq!(updates[1].revision, 2);
        assert_eq!(updates[1].status, Status::Unavailable);
    }
}
