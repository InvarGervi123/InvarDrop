//! Independent OS processes exercising production Service actors and storage.
//! The only substituted component is the OS keyring: each test-owned encrypted
//! profile receives its own random master key over an anonymous stdin pipe.
use super::*;
use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    sync::{atomic::AtomicBool, mpsc as channel},
};

static PROCESS_TEST_LOCK: Mutex<()> = Mutex::new(());
static PAUSE_CHECKPOINT: AtomicBool = AtomicBool::new(false);
static AT_CHECKPOINT: AtomicBool = AtomicBool::new(false);
const CHILD: &str = "INVARDROP_TEST_DEVICE_CHILD";
const ENTRY: &str =
    "secure::service::cross_device_tests::independent_devices_and_bidirectional_contents";
const PREFIX: &str = "DEVICE_RESPONSE ";

// Called only after the receiver has synced payload and committed its checkpoint.
// Pausing here makes crash/cancel races deterministic without altering TLS frames.
pub(in crate::secure) fn checkpoint_barrier() {
    if PAUSE_CHECKPOINT.load(Ordering::Acquire) {
        AT_CHECKPOINT.store(true, Ordering::Release);
        while PAUSE_CHECKPOINT.load(Ordering::Acquire) {
            std::thread::park_timeout(Duration::from_millis(5));
        }
    }
}

fn string<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap()
}
fn child_main() {
    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    let config: Value = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
    let root = PathBuf::from(string(&config, "root"));
    let master: Vec<u8> = serde_json::from_value(config["master"].clone()).unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap();
    let _entered = runtime.enter();
    let service = runtime.block_on(super::tests::start_with_master(
        &root,
        Duration::from_secs(30),
        &master,
    ));
    let discovery = std::env::var_os("INVARDROP_TEST_MDNS").map(|_| {
        let d = crate::discovery::Discovery::start_with_port(service.shared.port.clone(), |_| {});
        *lock(&service.shared.local_name) = d.snapshot().local_device.name;
        d
    });
    let processing = crate::processing::Processing::default();
    println!("{PREFIX}{}", json!({"ok": true}));
    for line in lines {
        let request: Value = serde_json::from_str(&line.unwrap()).unwrap();
        let op = string(&request, "op");
        let result: std::result::Result<Value, String> = runtime.block_on(async {
            let id = request["connection"].as_str().unwrap_or("");
            let transfer = request["transfer"].as_str().unwrap_or("");
            let action = match op {
                "status" => {
                    return Ok(json!({
                        "security": service.snapshot(),
                        "port": service.shared.port.load(Ordering::Acquire),
                        "paused": AT_CHECKPOINT.load(Ordering::Acquire),
                        "processing": processing.views(),
                        "discovery": discovery.as_ref().map(|d| d.snapshot()),
                    }));
                }
                "connect" => service
                    .connect(
                        string(&request, "contact").into(),
                        serde_json::from_value(request["endpoints"].clone()).unwrap(),
                    )
                    .map(|_| ()),
                "connect_discovered" => {
                    let device = discovery
                        .as_ref()
                        .unwrap()
                        .snapshot()
                        .devices
                        .into_iter()
                        .find(|d| d.id == string(&request, "peer") && d.can_connect)
                        .unwrap();
                    service.connect(device.id, device.endpoints).map(|_| ())
                }
                "approve" => service.respond(
                    id,
                    request["accept"].as_bool().unwrap(),
                    string(&request, "code").into(),
                ),
                "respond" => {
                    service.respond_file(id, transfer, request["accept"].as_bool().unwrap())
                }
                "send" => {
                    let path = Path::new(string(&request, "path"));
                    let legacy = request["legacy"].as_bool().unwrap_or(false);
                    if path.is_dir() {
                        let source = BundleSource::folder(path)
                            .await
                            .map_err(|e| e.to_string())?;
                        if legacy {
                            service.send_bundle(id, source)
                        } else {
                            service.send_resumable_bundle(id, source).await
                        }
                    } else if legacy {
                        service.send_file(id, Source::open(path).await.map_err(|e| e.to_string())?)
                    } else {
                        service.send_resumable_file(id, path).await
                    }
                }
                "files" => {
                    let paths = serde_json::from_value(request["paths"].clone()).unwrap();
                    let source = BundleSource::files(paths)
                        .await
                        .map_err(|e| e.to_string())?;
                    service.send_resumable_bundle(id, source).await
                }
                "resume" => service.resume_transfer(id, transfer).await,
                "discard" => service.discard_resume(transfer).await,
                "cancel" => service.cancel_file(id, transfer),
                "disconnect" => service.cancel(id),
                "revoke" => service.revoke(string(&request, "peer").into()).await,
                "arm" => {
                    AT_CHECKPOINT.store(false, Ordering::Release);
                    PAUSE_CHECKPOINT.store(true, Ordering::Release);
                    Ok(())
                }
                "release" => {
                    PAUSE_CHECKPOINT.store(false, Ordering::Release);
                    Ok(())
                }
                "select" => {
                    return processing
                        .select(
                            &service,
                            id.into(),
                            vec![PathBuf::from(string(&request, "path"))],
                            request["folder"].as_bool().unwrap_or(false),
                        )
                        .await
                        .map(Value::Bool);
                }
                "configure" => {
                    return processing
                        .configure(PathBuf::from(string(&request, "path")))
                        .map(|_| Value::Null);
                }
                "decide" => {
                    return processing
                        .decide(
                            &service,
                            string(&request, "job").into(),
                            request["convert"].as_bool().unwrap(),
                        )
                        .await
                        .map(|_| Value::Null);
                }
                "extract" => {
                    return processing
                        .extract(&service, id, transfer)
                        .await
                        .map(|_| Value::Null);
                }
                "stop" => {
                    service.stop().await;
                    Ok(())
                }
                _ => panic!("Unknown test command"),
            };
            action.map(|_| Value::Null).map_err(|e| e.to_string())
        });
        let response = match result {
            Ok(value) => json!({"ok": true, "value": value}),
            Err(error) => json!({"ok": false, "error": error}),
        };
        println!("{PREFIX}{response}");
        if op == "stop" {
            break;
        }
    }
    processing.shutdown();
    if let Some(d) = discovery {
        d.stop();
    }
    runtime.block_on(service.stop());
}

struct Device {
    child: Child,
    input: ChildStdin,
    output: channel::Receiver<Value>,
    root: PathBuf,
    master: [u8; 32],
}
impl Device {
    fn new(root: PathBuf) -> Self {
        Self::spawn(root, crate::secure::identity::random().unwrap())
    }
    fn spawn(root: PathBuf, master: [u8; 32]) -> Self {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", ENTRY, "--nocapture", "--test-threads=1"])
            .env(CHILD, "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        writeln!(input, "{}", json!({"root": root, "master": master})).unwrap();
        let stdout = child.stdout.take().unwrap();
        let (sender, output) = channel::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout)
                .lines()
                .map_while(std::result::Result::ok)
            {
                if let Some((_, json)) = line.split_once(PREFIX)
                    && let Ok(value) = serde_json::from_str(json)
                    && sender.send(value).is_err()
                {
                    break;
                }
            }
        });
        let device = Self {
            child,
            input,
            output,
            root,
            master,
        };
        assert!(
            device.output.recv_timeout(Duration::from_secs(45)).unwrap()["ok"] == true,
            "Isolated listener startup"
        );
        device
    }
    fn request(&mut self, value: Value) -> Value {
        writeln!(self.input, "{value}").unwrap();
        self.input.flush().unwrap();
        self.output
            .recv_timeout(Duration::from_secs(45))
            .expect("Isolated device command timed out")
    }
    fn rpc(&mut self, value: Value) -> Value {
        let op = string(&value, "op").to_owned();
        let response = self.request(value);
        assert!(
            response["ok"] == true,
            "Command {op}: {}",
            response["error"]
        );
        response["value"].clone()
    }
    fn status(&mut self) -> Value {
        self.rpc(json!({"op": "status"}))
    }
    fn wait(&mut self, label: &str, predicate: impl Fn(&Value) -> bool) -> Value {
        let until = Instant::now() + Duration::from_secs(45);
        loop {
            let value = self.status();
            if predicate(&value) {
                return value;
            }
            if Instant::now() >= until {
                // Never include pairing codes or complete command/snapshot dumps.
                let states: Vec<_> = value["security"]["connections"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|c| {
                        json!({
                            "phase": c["phase"], "error": c["error"],
                            "transferStatus": c["transfer"]["status"],
                            "transferred": c["transfer"]["transferred"],
                        })
                    })
                    .collect();
                panic!("Timed out waiting for {label}: {states:?}");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn connection(&mut self, phase: &str) -> Value {
        let value = self.wait(phase, |v| {
            v["security"]["connections"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["phase"] == phase)
        });
        value["security"]["connections"]
            .as_array()
            .unwrap()
            .iter()
            .rev()
            .find(|c| c["phase"] == phase)
            .unwrap()
            .clone()
    }
    fn transfer(&mut self, connection: &str, status: &str) -> Value {
        let value = self.wait(status, |v| {
            v["security"]["connections"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["id"] == connection && c["transfer"]["status"] == status)
        });
        value["security"]["connections"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == connection)
            .unwrap()["transfer"]
            .clone()
    }
    fn crash(&mut self) {
        self.child.kill().unwrap();
        self.child.wait().unwrap();
    }
    fn restart(&mut self) {
        let _ = self.child.kill();
        self.child.wait().unwrap();
        *self = Self::spawn(self.root.clone(), self.master);
    }
}
impl Drop for Device {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn connect(a: &mut Device, b: &mut Device) {
    let target = b.status();
    if std::env::var_os("INVARDROP_TEST_MDNS").is_some() {
        let peer = target["discovery"]["localDevice"]["id"].clone();
        a.wait("real multicast discovery", |v| {
            v["discovery"]["devices"]
                .as_array()
                .unwrap()
                .iter()
                .any(|d| d["id"] == peer && d["canConnect"] == true)
        });
        a.rpc(json!({"op": "connect_discovered", "peer": peer}));
    } else {
        a.rpc(json!({"op": "connect", "contact": "independent-device", "endpoints": [format!("127.0.0.1:{}", target["port"])]}));
    }
}
fn pair(a: &mut Device, b: &mut Device) -> (String, String) {
    connect(a, b);
    let ac = a.connection("verifying");
    let bc = b.connection("verifying");
    assert!(
        ac["code"].is_string() && ac["code"] == bc["code"],
        "Independent TLS exporters must agree"
    );
    a.rpc(json!({"op": "approve", "connection": ac["id"], "accept": true, "code": ac["code"]}));
    b.rpc(json!({"op": "approve", "connection": bc["id"], "accept": true, "code": bc["code"]}));
    connected(a, b)
}
fn connected(a: &mut Device, b: &mut Device) -> (String, String) {
    let ac = a.connection("connected");
    let bc = b.connection("connected");
    assert!(ac["code"].is_null() && bc["code"].is_null());
    (string(&ac, "id").into(), string(&bc, "id").into())
}
fn offer(a: &mut Device, b: &mut Device, ac: &str, bc: &str, path: &Path) -> Value {
    a.rpc(json!({"op": "send", "connection": ac, "path": path}));
    let offer = b.transfer(bc, "awaiting_approval");
    assert_eq!(offer["transferred"], 0);
    offer
}
fn accept(b: &mut Device, bc: &str, transfer: &Value) {
    b.rpc(json!({"op": "respond", "connection": bc, "transfer": transfer["id"], "accept": true}));
}
fn finish(a: &mut Device, b: &mut Device, ac: &str, bc: &str) -> PathBuf {
    let outgoing = a.transfer(ac, "complete");
    let incoming = b.transfer(bc, "complete");
    assert_eq!(outgoing["transferred"], outgoing["size"]);
    assert_eq!(incoming["transferred"], incoming["size"]);
    let path = PathBuf::from(string(&incoming, "path"));
    assert!(path.starts_with(b.root.join("received")) && path.exists());
    path
}
fn same_file(source: &Path, received: &Path) {
    fn hash(path: &Path) -> blake3::Hash {
        let mut file = fs::File::open(path).unwrap();
        let mut hash = blake3::Hasher::new();
        let mut chunk = [0u8; 65536];
        loop {
            let n = std::io::Read::read(&mut file, &mut chunk).unwrap();
            if n == 0 {
                break;
            }
            hash.update(&chunk[..n]);
        }
        hash.finalize()
    }
    assert_eq!(
        fs::metadata(source).unwrap().len(),
        fs::metadata(received).unwrap().len()
    );
    assert_eq!(hash(source), hash(received));
}
fn empty_inbox(device: &Device) {
    let path = device.root.join("received");
    assert!(
        !path.exists() || fs::read_dir(path).unwrap().next().is_none(),
        "Incomplete data was published"
    );
}
fn fixtures(root: &Path) -> (Device, Device) {
    (
        Device::new(root.join("alpha")),
        Device::new(root.join("beta")),
    )
}

#[test]
fn independent_devices_and_bidirectional_contents() {
    if std::env::var_os(CHILD).is_some() {
        child_main();
        return;
    }
    let _serial = lock(&PROCESS_TEST_LOCK);
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let (mut a, mut b) = fixtures(root);
    assert_ne!(
        a.status()["security"]["identity"],
        b.status()["security"]["identity"]
    );
    assert_ne!(a.status()["port"], b.status()["port"]);
    let (ac, bc) = pair(&mut a, &mut b);
    let source = root.join("caf\u{e9}-\u{65e5}\u{672c}.bin");
    for data in [
        Vec::new(),
        b"independent process TLS".to_vec(),
        vec![0xa7; files::CHUNK_SIZE * 70 + 17],
    ] {
        fs::write(&source, data).unwrap();
        let request = offer(&mut a, &mut b, &ac, &bc, &source);
        assert_eq!(
            request["name"],
            source.file_name().unwrap().to_str().unwrap()
        );
        accept(&mut b, &bc, &request);
        let path = finish(&mut a, &mut b, &ac, &bc);
        same_file(&source, &path);
        let request = offer(&mut b, &mut a, &bc, &ac, &source);
        accept(&mut a, &ac, &request);
        let path = finish(&mut b, &mut a, &bc, &ac);
        same_file(&source, &path);
    }
    let tree = root.join("Nested");
    fs::create_dir_all(tree.join("inner/empty")).unwrap();
    fs::write(tree.join("inner/data.txt"), b"nested contents").unwrap();
    fs::write(tree.join("empty.txt"), []).unwrap();
    let request = offer(&mut a, &mut b, &ac, &bc, &tree);
    accept(&mut b, &bc, &request);
    let path = finish(&mut a, &mut b, &ac, &bc);
    assert!(path.join("inner/empty").is_dir());
    same_file(&tree.join("inner/data.txt"), &path.join("inner/data.txt"));
    same_file(&tree.join("empty.txt"), &path.join("empty.txt"));
    a.rpc(json!({"op": "files", "connection": ac, "paths": [source, tree.join("empty.txt")]}));
    let request = b.transfer(&bc, "awaiting_approval");
    assert_eq!(request["fileCount"], 2);
    accept(&mut b, &bc, &request);
    let path = finish(&mut a, &mut b, &ac, &bc);
    same_file(&source, &path.join(source.file_name().unwrap()));
}

#[test]
fn untrusted_requests_repeated_pairing_and_declined_transfer_never_publish() {
    let _serial = lock(&PROCESS_TEST_LOCK);
    let directory = tempfile::tempdir().unwrap();
    let (mut a, mut b) = fixtures(directory.path());
    let path = directory.path().join("private.txt");
    fs::write(&path, b"approval required").unwrap();
    for _ in 0..2 {
        connect(&mut a, &mut b);
        let ac = a.connection("verifying");
        let bc = b.connection("verifying");
        assert!(
            a.request(json!({"op": "send", "connection": ac["id"], "path": path}))["ok"] == false
        );
        a.rpc(
            json!({"op": "approve", "connection": ac["id"], "accept": false, "code": ac["code"]}),
        );
        a.connection("closed");
        b.wait("rejected channel", |v| {
            v["security"]["connections"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["id"] == bc["id"] && c["phase"] == "closed")
        });
        assert_eq!(a.status()["security"]["trusted"], json!([]));
        assert_eq!(b.status()["security"]["trusted"], json!([]));
    }
    let (ac, bc) = pair(&mut a, &mut b);
    let request = offer(&mut a, &mut b, &ac, &bc, &path);
    empty_inbox(&b);
    assert!(b.request(json!({"op": "respond", "connection": bc, "transfer": uuid::Uuid::new_v4().to_string(), "accept": true}))["ok"] == false);
    b.rpc(json!({"op": "respond", "connection": bc, "transfer": request["id"], "accept": false}));
    a.transfer(&ac, "rejected");
    empty_inbox(&b);
    assert!(
        b.request(
            json!({"op": "respond", "connection": bc, "transfer": request["id"], "accept": true})
        )["ok"]
            == false
    );
}

fn interrupted_peer(
    root: &Path,
    folder: bool,
    kill_sender: bool,
) -> (Device, Device, PathBuf, String) {
    let (mut a, mut b) = fixtures(root);
    let (ac, bc) = pair(&mut a, &mut b);
    let source = root.join(if folder { "Tree" } else { "large.bin" });
    let file = if folder {
        fs::create_dir_all(source.join("nested/empty")).unwrap();
        fs::write(source.join("zero"), []).unwrap();
        source.join("nested/large.bin")
    } else {
        source.clone()
    };
    let mut out = fs::File::create(file).unwrap();
    for index in 0..33 {
        out.write_all(&vec![index as u8; 1024 * 1024]).unwrap();
    }
    out.write_all(b"tail").unwrap();
    drop(out);
    b.rpc(json!({"op": "arm"}));
    let request = offer(&mut a, &mut b, &ac, &bc, &source);
    accept(&mut b, &bc, &request);
    b.wait("durable checkpoint before acknowledgement", |v| {
        v["paused"] == true
    });
    empty_inbox(&b);
    if kill_sender {
        a.crash();
        b.rpc(json!({"op": "release"}));
        b.connection("closed");
    } else {
        b.crash();
        a.connection("closed");
    }
    a.restart();
    b.restart();
    let id = string(&request, "id").to_owned();
    let status = b.status();
    let records = status["security"]["resumable"].as_array().unwrap();
    let record = records
        .iter()
        .find(|r| r["id"] == id)
        .expect("Durable recovery after process kill");
    assert_eq!(record["verified"], resume::CHECKPOINT_BYTES);
    assert_eq!(record["requested"], false);
    (a, b, source, id)
}
fn interrupted(root: &Path, folder: bool) -> (Device, Device, PathBuf, String) {
    interrupted_peer(root, folder, false)
}
fn reconnect(a: &mut Device, b: &mut Device) -> (String, String) {
    let old_a = a.status()["security"]["identity"].clone();
    let old_b = b.status()["security"]["identity"].clone();
    assert!(
        a.status()["security"]["trusted"]
            .as_array()
            .unwrap()
            .contains(&old_b)
    );
    assert!(
        b.status()["security"]["trusted"]
            .as_array()
            .unwrap()
            .contains(&old_a)
    );
    connect(a, b);
    connected(a, b)
}
fn resume_offer(a: &mut Device, b: &mut Device, ac: &str, bc: &str, id: &str) -> Value {
    a.rpc(json!({"op": "resume", "connection": ac, "transfer": id}));
    let request = b.transfer(bc, "awaiting_approval");
    assert_eq!(request["id"], id);
    empty_inbox(b);
    request
}
#[test]
fn killed_file_processes_resume_twice_with_explicit_approval() {
    let _serial = lock(&PROCESS_TEST_LOCK);
    let directory = tempfile::tempdir().unwrap();
    let (mut a, mut b, source, id) = interrupted(directory.path(), false);
    let original_a = a.status()["security"]["identity"].clone();
    let original_b = b.status()["security"]["identity"].clone();
    let (ac, bc) = reconnect(&mut a, &mut b);
    assert!(b.status()["security"]["connections"][0]["transfer"].is_null());
    let request = resume_offer(&mut a, &mut b, &ac, &bc, &id);
    assert_eq!(request["transferred"], resume::CHECKPOINT_BYTES);
    b.rpc(json!({"op": "arm"}));
    accept(&mut b, &bc, &request);
    b.wait("second durable checkpoint", |v| v["paused"] == true);
    b.crash();
    a.connection("closed");
    a.restart();
    b.restart();
    assert_eq!(a.status()["security"]["identity"], original_a);
    assert_eq!(b.status()["security"]["identity"], original_b);
    assert_eq!(
        b.status()["security"]["resumable"][0]["verified"],
        2 * resume::CHECKPOINT_BYTES
    );
    let (ac, bc) = reconnect(&mut a, &mut b);
    let request = resume_offer(&mut a, &mut b, &ac, &bc, &id);
    accept(&mut b, &bc, &request);
    let path = finish(&mut a, &mut b, &ac, &bc);
    same_file(&source, &path);
}
#[test]
fn killed_nested_folder_resumes_without_losing_empty_entries() {
    let _serial = lock(&PROCESS_TEST_LOCK);
    let directory = tempfile::tempdir().unwrap();
    let (mut a, mut b, source, id) = interrupted(directory.path(), true);
    let (ac, bc) = reconnect(&mut a, &mut b);
    let request = resume_offer(&mut a, &mut b, &ac, &bc, &id);
    accept(&mut b, &bc, &request);
    let path = finish(&mut a, &mut b, &ac, &bc);
    same_file(
        &source.join("nested/large.bin"),
        &path.join("nested/large.bin"),
    );
    assert!(path.join("nested/empty").is_dir());
    same_file(&source.join("zero"), &path.join("zero"));
}

#[test]
fn killed_sender_resumes_from_receivers_durable_unacknowledged_checkpoint() {
    let _serial = lock(&PROCESS_TEST_LOCK);
    let directory = tempfile::tempdir().unwrap();
    let (mut a, mut b, source, id) = interrupted_peer(directory.path(), false, true);
    let (ac, bc) = reconnect(&mut a, &mut b);
    let request = resume_offer(&mut a, &mut b, &ac, &bc, &id);
    assert_eq!(request["transferred"], resume::CHECKPOINT_BYTES);
    accept(&mut b, &bc, &request);
    let received = finish(&mut a, &mut b, &ac, &bc);
    same_file(&source, &received);
}
#[test]
fn corrupted_partial_and_changed_source_cannot_complete_after_restart() {
    let _serial = lock(&PROCESS_TEST_LOCK);
    for corrupt_receiver in [true, false] {
        let directory = tempfile::tempdir().unwrap();
        let (mut a, mut b, source, id) = interrupted(directory.path(), false);
        let changed = if corrupt_receiver {
            b.root.join("resume-v1").join(&id).join("payload/large.bin")
        } else {
            source
        };
        let mut file = fs::OpenOptions::new().write(true).open(changed).unwrap();
        file.write_all(b"changed").unwrap();
        file.sync_all().unwrap();
        drop(file);
        let (ac, bc) = reconnect(&mut a, &mut b);
        let request = resume_offer(&mut a, &mut b, &ac, &bc, &id);
        accept(&mut b, &bc, &request);
        a.connection("closed");
        b.connection("closed");
        empty_inbox(&b);
        assert!(
            b.status()["security"]["connections"]
                .as_array()
                .unwrap()
                .iter()
                .all(|c| c["transfer"]["status"] != "complete")
        );
    }
}
#[test]
fn sender_and_receiver_cancel_at_checkpoint_then_discard_recovery() {
    let _serial = lock(&PROCESS_TEST_LOCK);
    for receiver_cancel in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let (mut a, mut b, source, id) = interrupted(directory.path(), false);
        let (ac, bc) = reconnect(&mut a, &mut b);
        let request = resume_offer(&mut a, &mut b, &ac, &bc, &id);
        b.rpc(json!({"op": "arm"}));
        accept(&mut b, &bc, &request);
        b.wait("checkpoint cancellation boundary", |v| v["paused"] == true);
        let (device, connection) = if receiver_cancel {
            (&mut b, &bc)
        } else {
            (&mut a, &ac)
        };
        device.rpc(json!({"op": "cancel", "connection": connection, "transfer": id}));
        b.rpc(json!({"op": "release"}));
        a.wait("sender cancellation", |v| {
            v["security"]["connections"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["transfer"]["status"] == "cancelled" || c["phase"] == "closed")
        });
        b.wait("receiver cancellation cleanup", |v| {
            v["security"]["resumable"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["id"] == id && r["verified"] == 0)
        });
        empty_inbox(&b);
        assert!(source.exists());
        let payload = b.root.join("resume-v1").join(&id).join("payload/large.bin");
        assert!(!payload.exists() || fs::metadata(payload).unwrap().len() == 0);
        a.rpc(json!({"op": "discard", "transfer": id}));
        b.rpc(json!({"op": "discard", "transfer": id}));
        assert!(!a.root.join("resume-v1").join(&id).exists());
        assert!(!b.root.join("resume-v1").join(&id).exists());
    }
}
#[test]
fn revoked_peer_cannot_reconnect_to_durable_recovery() {
    let _serial = lock(&PROCESS_TEST_LOCK);
    let directory = tempfile::tempdir().unwrap();
    let (mut a, mut b, _, _) = interrupted(directory.path(), false);
    let peer = a.status()["security"]["identity"].clone();
    b.rpc(json!({"op": "revoke", "peer": peer}));
    b.restart();
    connect(&mut a, &mut b);
    a.connection("closed");
    b.connection("closed");
    empty_inbox(&b);
    assert!(
        b.status()["security"]["trusted"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        b.status()["security"]["revoked"]
            .as_array()
            .unwrap()
            .contains(&peer)
    );
}

#[test]
fn received_archives_require_separate_extraction_and_preserve_verified_zip() {
    let _serial = lock(&PROCESS_TEST_LOCK);
    use zip::write::SimpleFileOptions;
    let directory = tempfile::tempdir().unwrap();
    let (mut a, mut b) = fixtures(directory.path());
    let (ac, bc) = pair(&mut a, &mut b);
    for malicious in [false, true] {
        let source = directory.path().join(if malicious {
            "malicious.zip"
        } else {
            "valid.zip"
        });
        let mut archive = zip::ZipWriter::new(fs::File::create(&source).unwrap());
        archive
            .add_directory("nested/empty/", SimpleFileOptions::default())
            .unwrap();
        archive
            .start_file(
                if malicious {
                    "../escape.txt"
                } else {
                    "nested/data.txt"
                },
                SimpleFileOptions::default(),
            )
            .unwrap();
        archive.write_all(b"verified archive content").unwrap();
        archive
            .start_file("zero.txt", SimpleFileOptions::default())
            .unwrap();
        archive.finish().unwrap();
        let request = offer(&mut a, &mut b, &ac, &bc, &source);
        accept(&mut b, &bc, &request);
        let path = finish(&mut a, &mut b, &ac, &bc);
        same_file(&source, &path);
        assert_eq!(
            fs::read_dir(path.parent().unwrap()).unwrap().count(),
            1,
            "No implicit extraction"
        );
        let result =
            b.request(json!({"op": "extract", "connection": bc, "transfer": request["id"]}));
        assert_eq!(result["ok"], !malicious);
        same_file(&source, &path);
        let status = b.status();
        let job = status["processing"].as_array().unwrap().last().unwrap();
        if malicious {
            assert_eq!(job["state"], "failed");
            assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
        } else {
            let extracted = PathBuf::from(string(job, "path"));
            assert!(extracted.join("nested/empty").is_dir());
            assert_eq!(
                fs::read(extracted.join("nested/data.txt")).unwrap(),
                b"verified archive content"
            );
            assert_eq!(fs::metadata(extracted.join("zero.txt")).unwrap().len(), 0);
        }
    }
}

#[test]
fn simultaneous_opposite_transfers_use_independent_authenticated_channels() {
    let _serial = lock(&PROCESS_TEST_LOCK);
    let directory = tempfile::tempdir().unwrap();
    let (mut a, mut b) = fixtures(directory.path());
    let (ac, bc) = pair(&mut a, &mut b);
    connect(&mut a, &mut b);
    for device in [&mut a, &mut b] {
        device.wait("second authenticated channel", |v| {
            v["security"]["connections"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|c| c["phase"] == "connected")
                .count()
                == 2
        });
    }
    let (ac2, bc2) = connected(&mut a, &mut b);
    assert_ne!(ac, ac2);
    assert_ne!(bc, bc2);
    let first = directory.path().join("first.bin");
    let second = directory.path().join("second.bin");
    fs::write(&first, vec![0x37; 4 * 1024 * 1024]).unwrap();
    fs::write(&second, vec![0x69; 4 * 1024 * 1024]).unwrap();
    let request_b = offer(&mut a, &mut b, &ac, &bc, &first);
    assert!(a.request(json!({"op": "send", "connection": ac, "path": second}))["ok"] == false);
    let request_a = offer(&mut b, &mut a, &bc2, &ac2, &second);
    accept(&mut b, &bc, &request_b);
    accept(&mut a, &ac2, &request_a);
    let received_first = finish(&mut a, &mut b, &ac, &bc);
    let received_second = finish(&mut b, &mut a, &bc2, &ac2);
    same_file(&first, &received_first);
    same_file(&second, &received_second);
}

#[test]
fn audio_choice_requires_converter_and_sends_actual_verified_format() {
    let _serial = lock(&PROCESS_TEST_LOCK);
    let directory = tempfile::tempdir().unwrap();
    let (mut a, mut b) = fixtures(directory.path());
    let (ac, bc) = pair(&mut a, &mut b);
    let binary = std::env::var_os("INVARDROP_TEST_FFMPEG")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::split_paths(&std::env::var_os("PATH").unwrap())
                .map(|p| {
                    p.join(if cfg!(windows) {
                        "ffmpeg.exe"
                    } else {
                        "ffmpeg"
                    })
                })
                .find(|p| p.is_file())
        })
        .expect("FFmpeg is required for the existing audio regression suite");
    let binary = fs::canonicalize(binary).unwrap();
    let source = directory.path().join("song.ogg");
    assert!(
        Command::new(&binary)
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=0.2",
                "-c:a",
                "libvorbis"
            ])
            .arg(&source)
            .status()
            .unwrap()
            .success()
    );
    let original = fs::read(&source).unwrap();
    a.rpc(json!({"op": "select", "connection": ac, "path": source}));
    let job = a.status()["processing"][0]["id"].clone();
    assert!(a.request(json!({"op": "decide", "job": job, "convert": true}))["ok"] == false);
    assert_eq!(a.status()["processing"][0]["state"], "choice");
    empty_inbox(&b);
    a.rpc(json!({"op": "decide", "job": job, "convert": false}));
    let request = b.transfer(&bc, "awaiting_approval");
    assert_eq!(request["name"], "song.ogg");
    accept(&mut b, &bc, &request);
    let received = finish(&mut a, &mut b, &ac, &bc);
    same_file(&source, &received);
    a.rpc(json!({"op": "configure", "path": binary}));
    a.rpc(json!({"op": "select", "connection": ac, "path": source}));
    let job = a.status()["processing"][1]["id"].clone();
    a.rpc(json!({"op": "decide", "job": job, "convert": true}));
    let request = b.transfer(&bc, "awaiting_approval");
    assert_eq!(request["name"], "song.mp3");
    assert!(request["size"].as_u64().unwrap() > 0);
    accept(&mut b, &bc, &request);
    let received = finish(&mut a, &mut b, &ac, &bc);
    assert!(
        Command::new(&binary)
            .args(["-v", "error", "-i"])
            .arg(&received)
            .args(["-f", "null", "-"])
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(fs::read(source).unwrap(), original);
}
