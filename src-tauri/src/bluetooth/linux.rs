//! BlueZ D-Bus GATT discovery and RFCOMM profiles. No raw HCI privileges,
//! adapter power changes, shell helpers or persistent OS trust changes.
use super::protocol::{self, ReverseRequest};
use super::*;
use crate::secure::stream::FdStream;
use std::{
    collections::HashMap,
    os::fd::{AsRawFd, OwnedFd},
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use zbus::{
    Connection, Proxy,
    fdo::{ManagedObjects, ObjectManager, ObjectManagerProxy},
    zvariant::{OwnedObjectPath, OwnedValue, Value},
};

const ROOT: &str = "/io/github/invargervi/invardrop/bluetooth";
const SERVICE: &str = "/io/github/invargervi/invardrop/bluetooth/service";
const CHARACTERISTIC: &str = "/io/github/invargervi/invardrop/bluetooth/service/bootstrap";
const PROFILE: &str = "/io/github/invargervi/invardrop/profile";
const ADVERTISEMENT: &str = "/io/github/invargervi/invardrop/advertisement";

fn server_options() -> HashMap<&'static str, Value<'static>> {
    HashMap::from([
        ("Role", Value::from("server")),
        ("Name", Value::from("InvarDrop TLS")),
        // BlueZ treats an omitted Channel as no RFCOMM listener for a custom
        // UUID. Explicit uint16 zero allocates a channel and puts it in SDP.
        ("Channel", Value::from(0u16)),
        ("RequireAuthentication", Value::from(true)),
        ("RequireAuthorization", Value::from(true)),
        ("AutoConnect", Value::from(false)),
    ])
}

fn rfcomm_failure(stage: &str, detail: &str) -> Error {
    let error = Error::BluetoothConnect(format!("Linux {stage}: {detail}"));
    eprintln!("InvarDrop RFCOMM: {error}");
    error
}

fn bluez_failure(stage: &str, error: zbus::Error) -> Error {
    let detail = match &error {
        zbus::Error::MethodError(name, reason, _) => match reason.as_deref() {
            Some(
                reason @ ("br-connection-profile-unavailable"
                | "br-connection-page-timeout"
                | "br-connection-key-missing"
                | "br-connection-refused"
                | "br-connection-timeout"),
            ) => {
                format!("{name}, {reason}")
            }
            _ => name.to_string(),
        },
        zbus::Error::InputOutput(error) => error.kind().to_string(),
        _ => "D-Bus proxy or response error".into(),
    };
    rfcomm_failure(stage, &detail)
}
type ReverseWaiters = Arc<Mutex<BTreeMap<uuid::Uuid, oneshot::Sender<Result<Stream>>>>>;
struct ReverseGuard {
    waiters: ReverseWaiters,
    token: uuid::Uuid,
}
impl Drop for ReverseGuard {
    fn drop(&mut self) {
        lock(&self.waiters).remove(&self.token);
    }
}

struct Profile {
    incoming: mpsc::Sender<FdStream>,
    active: Mutex<Vec<(OwnedObjectPath, std::sync::Weak<OwnedFd>)>>,
    role: &'static str,
}
impl Profile {
    fn disconnect(&self, device: Option<&OwnedObjectPath>) {
        for (peer, fd) in lock(&self.active).iter() {
            if device.is_none_or(|d| d == peer)
                && let Some(fd) = fd.upgrade()
            {
                // SAFETY: upgrading the weak reference keeps the descriptor live.
                unsafe {
                    libc::shutdown(fd.as_raw_fd(), libc::SHUT_RDWR);
                }
            }
        }
    }
}
impl Drop for Profile {
    fn drop(&mut self) {
        self.disconnect(None);
    }
}
#[zbus::interface(name = "org.bluez.Profile1")]
impl Profile {
    fn release(&self) {
        eprintln!("InvarDrop RFCOMM: BlueZ released {} profile", self.role);
        self.disconnect(None);
    }
    fn new_connection(
        &self,
        device: OwnedObjectPath,
        fd: zbus::zvariant::OwnedFd,
        _properties: HashMap<String, OwnedValue>,
    ) -> zbus::fdo::Result<()> {
        let stream = FdStream::new(fd.into()).map_err(|error| {
            eprintln!(
                "InvarDrop RFCOMM: BlueZ {} NewConnection descriptor failed ({})",
                self.role,
                error.kind()
            );
            zbus::fdo::Error::Failed("RFCOMM descriptor is unavailable".into())
        })?;
        let mut active = lock(&self.active);
        active.retain(|(_, fd)| fd.strong_count() > 0);
        if active.len() >= 8 {
            eprintln!(
                "InvarDrop RFCOMM: BlueZ {} NewConnection rejected (connection limit)",
                self.role
            );
            return Err(zbus::fdo::Error::LimitsExceeded(
                "Connection limit reached".into(),
            ));
        }
        active.push((device, stream.close_handle()));
        self.incoming.try_send(stream).map_err(|error| {
            eprintln!(
                "InvarDrop RFCOMM: BlueZ {} NewConnection queue failed ({error})",
                self.role
            );
            zbus::fdo::Error::LimitsExceeded("Connection limit reached".into())
        })?;
        eprintln!(
            "InvarDrop RFCOMM: BlueZ {} NewConnection descriptor accepted",
            self.role
        );
        Ok(())
    }
    fn request_disconnection(&self, device: OwnedObjectPath) {
        eprintln!(
            "InvarDrop RFCOMM: BlueZ requested {} disconnection",
            self.role
        );
        self.disconnect(Some(&device));
    }
}
struct GattService;
#[zbus::interface(name = "org.bluez.GattService1")]
impl GattService {
    #[zbus(property, name = "UUID")]
    fn uuid(&self) -> &str {
        DISCOVERY_UUID
    }
    #[zbus(property)]
    fn primary(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn includes(&self) -> Vec<OwnedObjectPath> {
        vec![]
    }
}
struct BootstrapCharacteristic(Vec<u8>);
#[zbus::interface(name = "org.bluez.GattCharacteristic1")]
impl BootstrapCharacteristic {
    #[zbus(property, name = "UUID")]
    fn uuid(&self) -> &str {
        BOOTSTRAP_UUID
    }
    #[zbus(property)]
    fn service(&self) -> OwnedObjectPath {
        SERVICE.try_into().expect("constant object path")
    }
    #[zbus(property)]
    fn flags(&self) -> Vec<&str> {
        vec!["read"]
    }
    fn read_value(&self, options: HashMap<String, OwnedValue>) -> zbus::fdo::Result<Vec<u8>> {
        let offset = options
            .get("offset")
            .and_then(|v| u16::try_from(v).ok())
            .unwrap_or(0) as usize;
        self.0
            .get(offset..)
            .map(|v| v.to_vec())
            .ok_or_else(|| zbus::fdo::Error::InvalidArgs("Invalid offset".into()))
    }
}
struct Advertisement;
#[zbus::interface(name = "org.bluez.LEAdvertisement1")]
impl Advertisement {
    fn release(&self) {}
    #[zbus(property, name = "Type")]
    fn kind(&self) -> &str {
        "peripheral"
    }
    #[zbus(property, name = "ServiceUUIDs")]
    fn service_uuids(&self) -> Vec<&str> {
        vec![DISCOVERY_UUID]
    }
}
async fn proxy<'a>(
    connection: &'a Connection,
    path: &'a str,
    interface: &'a str,
) -> zbus::Result<Proxy<'a>> {
    Proxy::new(connection, "org.bluez", path, interface).await
}
async fn objects(connection: &Connection) -> zbus::Result<ManagedObjects> {
    Ok(ObjectManagerProxy::builder(connection)
        .destination("org.bluez")?
        .path("/")?
        .build()
        .await?
        .get_managed_objects()
        .await?)
}
fn text_property(properties: &HashMap<String, OwnedValue>, name: &str) -> Option<String> {
    properties
        .get(name)
        .and_then(|v| <&str>::try_from(v).ok())
        .map(str::to_owned)
}
async fn characteristic(connection: &Connection, device: &str, uuid: &str) -> zbus::Result<String> {
    let all = objects(connection).await?;
    all.iter()
        .find_map(|(path, interfaces)| {
            let p = interfaces.get("org.bluez.GattCharacteristic1")?;
            (path.as_str().starts_with(&format!("{device}/"))
                && text_property(p, "UUID").is_some_and(|s| s.eq_ignore_ascii_case(uuid)))
            .then(|| path.to_string())
        })
        .ok_or_else(|| zbus::Error::Failure("InvarDrop GATT service is unavailable".into()))
}
async fn resolve(connection: &Connection, device: &str) -> zbus::Result<Bootstrap> {
    let remote = proxy(connection, device, "org.bluez.Device1").await?;
    if !remote
        .get_property::<bool>("Connected")
        .await
        .unwrap_or(false)
    {
        remote.call::<_, _, ()>("Connect", &()).await?;
    }
    let path = tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            if let Ok(path) = characteristic(connection, device, BOOTSTRAP_UUID).await {
                break path;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .map_err(|_| zbus::Error::Failure("GATT resolution timed out".into()))?;
    let value: Vec<u8> = proxy(connection, &path, "org.bluez.GattCharacteristic1")
        .await?
        .call("ReadValue", &(HashMap::<&str, Value<'_>>::new(),))
        .await?;
    Bootstrap::decode(&value)
        .map_err(|_| zbus::Error::Failure("Invalid Bluetooth bootstrap".into()))
}

struct ProfileStream {
    stream: Stream,
    connection: Connection,
    path: String,
}
impl Drop for ProfileStream {
    fn drop(&mut self) {
        let connection = self.connection.clone();
        let path = self.path.clone();
        tauri::async_runtime::spawn(async move {
            let path: OwnedObjectPath = path.try_into().expect("registered object path");
            if let Ok(manager) = proxy(&connection, "/org/bluez", "org.bluez.ProfileManager1").await
            {
                let _ = manager
                    .call::<_, _, ()>("UnregisterProfile", &(path.clone(),))
                    .await;
            }
            let _ = connection.object_server().remove::<Profile, _>(path).await;
        });
    }
}
impl AsyncRead for ProfileStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}
impl AsyncWrite for ProfileStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write(cx, bytes)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}
async fn dial(connection: &Connection, adapter: &str, route: &Route) -> Result<Stream> {
    let address = route
        .info
        .address
        .as_ref()
        .ok_or_else(|| rfcomm_failure("direct routing", "peer has no Classic locator"))?;
    eprintln!(
        "InvarDrop RFCOMM: direct outgoing service UUID {}",
        route.info.service
    );
    let path = format!(
        "{adapter}/dev_{}",
        address.replace(':', "_").to_ascii_uppercase()
    );
    let device = proxy(connection, &path, "org.bluez.Device1")
        .await
        .map_err(|error| bluez_failure("Device1 lookup", error))?;
    let paired = device
        .get_property::<bool>("Paired")
        .await
        .map_err(|error| bluez_failure("Device1.Paired", error))?;
    eprintln!("InvarDrop RFCOMM: BlueZ Device1.Paired={paired}");
    if !paired {
        eprintln!("InvarDrop RFCOMM: awaiting BlueZ Device1.Pair and any OS prompt");
        device
            .call::<_, _, ()>("Pair", &())
            .await
            .map_err(|error| bluez_failure("Device1.Pair", error))?;
        eprintln!("InvarDrop RFCOMM: BlueZ Device1.Pair completed");
    }
    let object = format!("{PROFILE}/client{}", uuid::Uuid::new_v4().simple());
    let (incoming, mut accepted) = mpsc::channel(1);
    connection
        .object_server()
        .at(
            object.as_str(),
            Profile {
                incoming,
                active: Mutex::new(Vec::new()),
                role: "client",
            },
        )
        .await
        .map_err(|error| bluez_failure("client Profile1 export", error))?;
    // Own the export and registration through cancellation and stream lifetime.
    let mut guard = ProfileStream {
        stream: Box::new(tokio::io::empty()),
        connection: connection.clone(),
        path: object.clone(),
    };
    let manager = proxy(connection, "/org/bluez", "org.bluez.ProfileManager1")
        .await
        .map_err(|error| bluez_failure("ProfileManager1 lookup", error))?;
    let options = HashMap::from([
        ("Role", Value::from("client")),
        ("RequireAuthentication", Value::from(true)),
        ("AutoConnect", Value::from(false)),
    ]);
    manager
        .call::<_, _, ()>(
            "RegisterProfile",
            &(
                OwnedObjectPath::try_from(object.clone()).map_err(|_| Error::Protocol)?,
                route.info.service.as_str(),
                options,
            ),
        )
        .await
        .map_err(|error| bluez_failure("ProfileManager1.RegisterProfile client", error))?;
    eprintln!(
        "InvarDrop RFCOMM: calling BlueZ Device1.ConnectProfile for SDP and socket connection"
    );
    device
        .call::<_, _, ()>("ConnectProfile", &(route.info.service.as_str(),))
        .await
        .map_err(|error| bluez_failure("Device1.ConnectProfile", error))?;
    let fd = accepted.recv().await.ok_or_else(|| {
        rfcomm_failure("client Profile1.NewConnection", "descriptor queue closed")
    })?;
    guard.stream = Box::new(fd);
    protocol::preface(&mut guard, None)
        .await
        .map_err(|_| rfcomm_failure("outgoing routing preface write", "socket I/O failed"))?;
    eprintln!("InvarDrop RFCOMM: direct outgoing preface sent, handing stream to TLS");
    Ok(Box::new(guard))
}
async fn connect(
    connection: Connection,
    adapter: String,
    local: Bootstrap,
    route: Route,
    reverse: ReverseWaiters,
    mut reply: oneshot::Sender<Result<Stream>>,
) {
    let operation = async {
        if route.info.address.is_some() {
            return dial(&connection, &adapter, &route).await;
        }
        // Android cannot read its own Classic MAC. Ask it to dial this listener;
        // a random one-use token routes that socket, while TLS proves identity.
        let token = uuid::Uuid::new_v4();
        let (tx, rx) = oneshot::channel();
        lock(&reverse).insert(token, tx);
        let _guard = ReverseGuard {
            waiters: reverse.clone(),
            token,
        };
        async {
            eprintln!(
                "InvarDrop RFCOMM: requesting Android reverse connection to service UUID {}",
                local.service
            );
            let path = characteristic(&connection, &route.native_id, REQUEST_UUID)
                .await
                .map_err(|error| bluez_failure("reverse GATT characteristic lookup", error))?;
            let request = ReverseRequest {
                version: 1,
                address: local.address.ok_or_else(|| {
                    rfcomm_failure("reverse routing", "local adapter has no Classic address")
                })?,
                service: local.service,
                token: token.to_string(),
            }
            .encode()?;
            proxy(&connection, &path, "org.bluez.GattCharacteristic1")
                .await
                .map_err(|error| bluez_failure("reverse GATT proxy lookup", error))?
                .call::<_, _, ()>("WriteValue", &(request, HashMap::<&str, Value<'_>>::new()))
                .await
                .map_err(|error| bluez_failure("reverse GATT WriteValue", error))?;
            eprintln!(
                "InvarDrop RFCOMM: reverse GATT request accepted, waiting for Android socket"
            );
            rx.await.map_err(|_| {
                rfcomm_failure("reverse RFCOMM listener", "descriptor waiter closed")
            })?
        }
        .await
    };
    tokio::select! {
        _ = reply.closed() => {},
        result = tokio::time::timeout(Duration::from_secs(40), operation) => {
            let _ = reply.send(result.unwrap_or_else(|_| {
                eprintln!("InvarDrop RFCOMM: Linux outgoing attempt timed out; see the preceding stage");
                Err(Error::BluetoothTimeout)
            }));
        }
    }
}

pub(super) async fn run(
    shared: Arc<Controller>,
    mut commands: mpsc::Receiver<Command>,
    mut stop: watch::Receiver<bool>,
) -> std::result::Result<(), String> {
    let connection = Connection::system()
        .await
        .map_err(|_| "BlueZ system D-Bus is unavailable".to_string())?;
    let all = objects(&connection)
        .await
        .map_err(|_| "BlueZ is unavailable; check bluetooth.service".to_string())?;
    let (adapter, properties) = all
        .iter()
        .find_map(|(path, interfaces)| {
            let p = interfaces.get("org.bluez.Adapter1")?;
            (p.get("Powered").and_then(|v| bool::try_from(v).ok()) == Some(true))
                .then(|| (path.to_string(), p))
        })
        .ok_or("No powered Bluetooth adapter is available; enable it in system settings")?;
    let mut local = shared.local.clone();
    local.address = text_property(properties, "Address");
    if local
        .address
        .as_ref()
        .is_none_or(|a| !protocol::valid_address(a))
    {
        return Err("The Bluetooth adapter has no usable Classic address".into());
    }
    let (accepted, mut incoming) = mpsc::channel(8);
    connection
        .object_server()
        .at(
            PROFILE,
            Profile {
                incoming: accepted,
                active: Mutex::new(Vec::new()),
                role: "server",
            },
        )
        .await
        .map_err(|error| bluez_failure("server Profile1 export", error).to_string())?;
    let manager = proxy(&connection, "/org/bluez", "org.bluez.ProfileManager1")
        .await
        .map_err(|error| bluez_failure("ProfileManager1 lookup", error).to_string())?;
    manager
        .call::<_, _, ()>(
            "RegisterProfile",
            &(
                OwnedObjectPath::try_from(PROFILE).expect("constant path"),
                local.service.as_str(),
                server_options(),
            ),
        )
        .await
        .map_err(|error| {
            bluez_failure("ProfileManager1.RegisterProfile server", error).to_string()
        })?;
    let adapter_proxy = proxy(&connection, &adapter, "org.bluez.Adapter1")
        .await
        .map_err(|error| bluez_failure("Adapter1 lookup", error).to_string())?;
    let service_uuids: Vec<String> =
        adapter_proxy.get_property("UUIDs").await.map_err(|error| {
            bluez_failure("Adapter1.UUIDs after server registration", error).to_string()
        })?;
    if !service_uuids
        .iter()
        .any(|uuid| uuid.eq_ignore_ascii_case(&local.service))
    {
        return Err(rfcomm_failure("server SDP publication", "registered profile UUID is absent from the adapter; check the bluetoothd journal for listener or SDP errors").to_string());
    }
    eprintln!(
        "InvarDrop RFCOMM: authenticated BlueZ server registered with automatic channel, SDP UUID {}",
        local.service
    );
    connection
        .object_server()
        .at(SERVICE, GattService)
        .await
        .map_err(|_| "GATT service export failed")?;
    connection
        .object_server()
        .at(
            CHARACTERISTIC,
            BootstrapCharacteristic(
                local
                    .encode()
                    .map_err(|_| "Invalid local Bluetooth bootstrap")?,
            ),
        )
        .await
        .map_err(|_| "GATT characteristic export failed")?;
    connection
        .object_server()
        .at(ROOT, ObjectManager)
        .await
        .map_err(|_| "GATT application export failed")?;
    proxy(&connection, &adapter, "org.bluez.GattManager1")
        .await
        .map_err(|_| "BlueZ GATT is unavailable")?
        .call::<_, _, ()>(
            "RegisterApplication",
            &(
                OwnedObjectPath::try_from(ROOT).expect("constant path"),
                HashMap::<&str, Value<'_>>::new(),
            ),
        )
        .await
        .map_err(|_| "GATT registration failed")?;
    connection
        .object_server()
        .at(ADVERTISEMENT, Advertisement)
        .await
        .map_err(|_| "Advertisement export failed")?;
    proxy(&connection, &adapter, "org.bluez.LEAdvertisingManager1")
        .await
        .map_err(|_| "BLE advertising is unsupported")?
        .call::<_, _, ()>(
            "RegisterAdvertisement",
            &(
                OwnedObjectPath::try_from(ADVERTISEMENT).expect("constant path"),
                HashMap::<&str, Value<'_>>::new(),
            ),
        )
        .await
        .map_err(|_| "BLE advertising failed; check controller support")?;
    adapter_proxy
        .call::<_, _, ()>(
            "SetDiscoveryFilter",
            &(HashMap::from([
                ("Transport", Value::from("le")),
                ("UUIDs", Value::from(vec![DISCOVERY_UUID])),
            ]),),
        )
        .await
        .map_err(|_| "BLE scan filter failed")?;
    adapter_proxy
        .call::<_, _, ()>("StartDiscovery", &())
        .await
        .map_err(|_| "BLE discovery failed")?;
    shared.status("scanning", None);
    let scan_window = Duration::from_secs(24);
    let mut scanning_until = Instant::now() + scan_window;
    let mut scanning = true;
    let mut seen = BTreeMap::<String, Instant>::new();
    let reverse: ReverseWaiters = Arc::new(Mutex::new(BTreeMap::new()));
    let mut tasks = tokio::task::JoinSet::new();
    let mut next_check = Instant::now();
    let mut expire = tokio::time::interval(Duration::from_secs(10));
    loop {
        tokio::select! {
            _ = stop.changed() => break,
            command = commands.recv() => match command {
                None => break,
                Some(Command::Scan) => {
                    if !scanning && adapter_proxy.call::<_, _, ()>("StartDiscovery", &()).await.is_ok() {
                        scanning = true; scanning_until = Instant::now() + scan_window; seen.clear(); shared.status("scanning", None);
                    }
                }
                Some(Command::Connect { route, reply }) => {
                    if tasks.len() >= 8 { let _ = reply.send(Err(Error::Limit)); continue; }
                    if scanning {
                        let _ = adapter_proxy.call::<_, _, ()>("StopDiscovery", &()).await;
                        scanning = false;
                        shared.status("ready", None);
                    }
                    tasks.spawn(connect(connection.clone(), adapter.clone(), local.clone(), route, reverse.clone(), reply));
                }
            },
            fd = incoming.recv() => if let Some(fd) = fd {
                if tasks.len() >= 8 {
                    eprintln!("InvarDrop RFCOMM: incoming socket dropped (worker limit)");
                    continue;
                }
                let shared = shared.clone(); let reverse = reverse.clone();
                tasks.spawn(async move {
                    let mut stream = fd;
                    match tokio::time::timeout(Duration::from_secs(3), protocol::read_preface(&mut stream)).await {
                        Ok(Ok(Some(token))) => {
                            let reply = lock(&reverse).remove(&token);
                            if let Some(reply) = reply {
                                eprintln!("InvarDrop RFCOMM: reverse routing preface accepted, handing stream to TLS");
                                let _ = reply.send(Ok(Box::new(stream)));
                            } else {
                                eprintln!("InvarDrop RFCOMM: reverse routing preface rejected (no pending request)");
                            }
                        }
                        Ok(Ok(None)) => {
                            if shared.incoming.try_send(Incoming { stream: Box::new(stream) }).is_ok() {
                                eprintln!("InvarDrop RFCOMM: incoming routing preface accepted, handing stream to TLS");
                            } else {
                                eprintln!("InvarDrop RFCOMM: incoming TLS queue unavailable");
                            }
                        }
                        Ok(Err(Error::Protocol)) => eprintln!("InvarDrop RFCOMM: incoming routing preface rejected (invalid format)"),
                        Ok(Err(_)) => eprintln!("InvarDrop RFCOMM: incoming routing preface read failed (socket closed or I/O error)"),
                        Err(_) => eprintln!("InvarDrop RFCOMM: incoming routing preface timed out"),
                    }
                });
            },
            _ = tasks.join_next(), if !tasks.is_empty() => {},
            _ = expire.tick() => shared.expire(),
            _ = tokio::time::sleep_until(next_check.into()), if scanning => {
                next_check = Instant::now() + Duration::from_secs(2);
                if scanning && Instant::now() >= scanning_until {
                    let _ = adapter_proxy.call::<_, _, ()>("StopDiscovery", &()).await; scanning = false; shared.status("ready", None);
                }
                if !scanning || tasks.len() >= 4 { continue; }
                let all = objects(&connection).await.map_err(|_| "BlueZ disconnected")?;
                for (path, interfaces) in all {
                    let Some(p) = interfaces.get("org.bluez.Device1") else { continue; };
                    let has_service = p.get("UUIDs").and_then(|v| Vec::<String>::try_from(v.try_clone().ok()?).ok()).is_some_and(|ids| ids.iter().any(|s| s.eq_ignore_ascii_case(DISCOVERY_UUID)));
                    if !has_service || !path.as_str().starts_with(&adapter) || seen.get(path.as_str()).is_some_and(|t| t.elapsed() < Duration::from_secs(30)) { continue; }
                    if tasks.len() >= 4 || seen.len() >= MAX_DEVICES { break; }
                    seen.insert(path.to_string(), Instant::now()); let connection = connection.clone(); let shared = shared.clone();
                    tasks.spawn(async move { if let Ok(info) = resolve(&connection, path.as_str()).await { shared.observe(info, path.to_string()); } });
                }
            }
        }
    }
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    lock(&reverse).clear();
    if scanning {
        let _ = adapter_proxy.call::<_, _, ()>("StopDiscovery", &()).await;
    }
    // Dropping this private bus connection unregisters its GATT, advertising and
    // profiles without altering another application's discovery or OS bonds.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_sdp_profile_requests_an_authenticated_rfcomm_listener() {
        let options = server_options();
        assert_eq!(options["Channel"], Value::from(0u16));
        assert_eq!(options["Role"], Value::from("server"));
        assert_eq!(options["RequireAuthentication"], Value::from(true));
        assert_eq!(options["RequireAuthorization"], Value::from(true));
        assert!(!options.contains_key("PSM"));
        assert!(!options.contains_key("ServiceRecord"));
    }
}
