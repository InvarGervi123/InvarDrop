//! Native BluetoothSocket owns the radio connection; Rust owns one end of a
//! kernel socketpair carrying ciphertext. Dropping it terminates native pumps.
use super::*;
use crate::secure::stream::FdStream;
use serde::Deserialize;
use std::os::fd::{FromRawFd, OwnedFd};
use tauri::{
    Manager,
    ipc::Channel,
    plugin::{Builder, PluginHandle, TauriPlugin, mobile::PluginInvokeError},
};

struct Handle {
    plugin: PluginHandle<tauri::Wry>,
    signal: Channel<serde_json::Value>,
    updates: watch::Receiver<()>,
}
pub(crate) fn init() -> TauriPlugin<tauri::Wry> {
    Builder::new("android-bluetooth")
        .setup(|app, api| {
            let plugin =
                api.register_android_plugin("io.github.invargervi.invardrop", "BluetoothPlugin")?;
            let (changed, updates) = watch::channel(());
            let signal = Channel::new(move |_| {
                changed.send_replace(());
                Ok(())
            });
            app.manage(Handle {
                plugin,
                signal,
                updates,
            });
            Ok(())
        })
        .build()
}
pub(super) async fn permission(
    app: &tauri::AppHandle,
    request: bool,
) -> std::result::Result<bool, String> {
    #[derive(Deserialize)]
    struct Permission {
        granted: bool,
    }
    let handle = app.state::<Handle>().plugin.clone();
    let permission = handle
        .run_mobile_plugin_async::<Permission>(
            "permission",
            serde_json::json!({ "request": request }),
        )
        .await
        .map_err(|error| bridge_failure("permission", error))?;
    if request && !permission.granted {
        return Err("Bluetooth permission was not granted; LAN sharing remains available".into());
    }
    Ok(permission.granted)
}

fn bridge_failure(command: &str, error: PluginInvokeError) -> String {
    let reason = match error {
        PluginInvokeError::InvokeRejected(response)
            if matches!(
                response.code.as_deref(),
                Some("bluetooth-startup" | "bluetooth-rfcomm")
            ) =>
        {
            if let Some(message) = response.message {
                return message;
            }
            "native failure without a diagnostic"
        }
        PluginInvokeError::InvokeRejected(_) => "native command rejected",
        PluginInvokeError::UnreachableWebview => "webview unreachable",
        PluginInvokeError::Jni(_) => "JNI call failed",
        PluginInvokeError::CannotSerializePayload(_) => "invalid bridge request",
        PluginInvokeError::CannotDeserializeResponse(_) => "invalid bridge response",
    };
    let message = format!("Android Bluetooth bridge {command} failed ({reason})");
    eprintln!("{message}");
    message
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeviceResult {
    native_id: String,
    info: String,
    sequence: u64,
}
#[derive(Deserialize)]
struct IncomingResult {
    handle: String,
    request: String,
}
#[derive(Deserialize)]
struct Failure {
    request: String,
    error: String,
}
#[derive(Deserialize)]
struct NativeSnapshot {
    status: String,
    error: Option<String>,
    devices: Vec<DeviceResult>,
    incoming: Vec<IncomingResult>,
    failures: Vec<Failure>,
}

pub(super) async fn run(
    shared: Arc<Controller>,
    mut commands: mpsc::Receiver<Command>,
    mut stop: watch::Receiver<bool>,
) -> std::result::Result<(), String> {
    let plugin = shared.app.state::<Handle>();
    let handle = plugin.plugin.clone();
    let mut updates = plugin.updates.clone();
    let bootstrap = String::from_utf8(
        shared
            .local
            .encode()
            .map_err(|_| "Invalid Bluetooth bootstrap")?,
    )
    .map_err(|_| "Invalid Bluetooth bootstrap")?;
    let startup = handle
        .run_mobile_plugin_async::<serde_json::Value>(
            "start",
            serde_json::json!({ "bootstrap": bootstrap, "signal": plugin.signal }),
        )
        .await
        .map_err(|error| bridge_failure("start", error));
    let mut pending = BTreeMap::<String, oneshot::Sender<Result<Stream>>>::new();
    let mut sequences = BTreeMap::new();
    let mut check = tokio::time::interval(Duration::from_secs(15));
    check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut cancel_check = tokio::time::interval(Duration::from_millis(500));
    cancel_check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let result = async {
        startup?;
        loop {
            tokio::select! {
                _ = stop.changed() => break,
                command = commands.recv() => {
                    match command {
                    None => break,
                    Some(Command::Scan) => { let _ = handle.run_mobile_plugin_async::<serde_json::Value>("scan", ()).await; },
                    Some(Command::Connect { route, reply }) => {
                        if pending.len() >= 1 { let _ = reply.send(Err(Error::Limit)); continue; }
                        let request = uuid::Uuid::new_v4().to_string();
                        let result = handle.run_mobile_plugin_async::<serde_json::Value>("connect", serde_json::json!({"peer": route.native_id, "request": request})).await;
                        match result {
                            Ok(_) => { pending.insert(request, reply); }
                            Err(error) => { let _ = reply.send(Err(Error::BluetoothConnect(bridge_failure("connect", error)))); }
                        }
                    }
                    }
                    continue;
                },
                _ = cancel_check.tick(), if !pending.is_empty() => {
                    let expired = pending.iter().filter(|(_, reply)| reply.is_closed()).map(|(id, _)| id.clone()).collect::<Vec<_>>();
                    for request in expired { pending.remove(&request); let _ = handle.run_mobile_plugin_async::<serde_json::Value>("cancelConnect", serde_json::json!({"request": request})).await; }
                    continue;
                },
                _ = updates.changed() => {},
                _ = check.tick() => {},
            }
            let native = handle.run_mobile_plugin_async::<NativeSnapshot>("poll", ()).await.map_err(|error| bridge_failure("poll", error))?;
            if native.status == "unavailable" { return Err(native.error.unwrap_or_else(|| "Android Bluetooth became unavailable".into())); }
            if shared.snapshot().status != native.status || shared.snapshot().error != native.error { shared.status(&native.status, native.error); }
            sequences.retain(|id, _| native.devices.iter().any(|device| &device.native_id == id));
            for device in native.devices.into_iter().take(MAX_DEVICES) {
                if sequences.get(&device.native_id) == Some(&device.sequence) { continue; }
                sequences.insert(device.native_id.clone(), device.sequence);
                if let Ok(info) = Bootstrap::decode(device.info.as_bytes()) { shared.observe(info, device.native_id); }
            }
            for incoming in native.incoming {
                let direction = if incoming.request.is_empty() { "incoming or reverse" } else { "outgoing" };
                let stream = take_descriptor(&incoming.handle).and_then(FdStream::new)
                    .map(|fd| Box::new(fd) as Stream)
                    .map_err(|error| Error::BluetoothConnect(format!("Android {direction} descriptor handoff: {error}")));
                match &stream {
                    Ok(_) => eprintln!("InvarDrop RFCOMM: Android {direction} descriptor acquired, handing stream to TLS"),
                    Err(error) => eprintln!("InvarDrop RFCOMM: {error}"),
                }
                if incoming.request.is_empty() { if let Ok(stream) = stream { let _ = shared.incoming.try_send(Incoming { stream }); } }
                else if let Some(reply) = pending.remove(&incoming.request) { let _ = reply.send(stream); }
            }
            for failure in native.failures {
                if let Some(reply) = pending.remove(&failure.request) { let _ = reply.send(Err(Error::BluetoothConnect(failure.error))); }
            }
            shared.expire();
        }
        Ok(())
    }.await;
    pending.clear();
    let _ = handle
        .run_mobile_plugin_async::<serde_json::Value>("stop", ())
        .await;
    result
}

// A synchronous JNI call transfers fd ownership without an async cancellation
// window. Native retains and expires every descriptor that Rust never takes.
fn take_descriptor(handle: &str) -> std::io::Result<OwnedFd> {
    use jni::{
        JavaVM,
        objects::{JClass, JObject, JValue},
    };
    if tauri::tao::platform::android::prelude::main_android_context().is_none() {
        return Err(std::io::Error::other("Android context is unavailable"));
    }
    let context = ndk_context::android_context();
    // SAFETY: Tauri owns these process-lifetime VM and context references.
    let vm = unsafe { JavaVM::from_raw(context.vm().cast()) }.map_err(std::io::Error::other)?;
    let mut env = vm.attach_current_thread().map_err(std::io::Error::other)?;
    let result = env.with_local_frame(16, |env| -> jni::errors::Result<i32> {
        // SAFETY: this is a borrowed global reference, not deleted by JObject.
        let app = unsafe { JObject::from_raw(context.context().cast()) };
        let loader = env
            .call_method(app, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])?
            .l()?;
        let name = env.new_string("io.github.invargervi.invardrop.BluetoothPlugin")?;
        let class = env
            .call_method(
                loader,
                "loadClass",
                "(Ljava/lang/String;)Ljava/lang/Class;",
                &[JValue::Object(&name)],
            )?
            .l()?;
        let handle = env.new_string(handle)?;
        env.call_static_method(
            JClass::from(class),
            "takeDescriptor",
            "(Ljava/lang/String;)I",
            &[JValue::Object(&handle)],
        )?
        .i()
    });
    if env.exception_check().unwrap_or(true) {
        let _ = env.exception_clear();
        return Err(std::io::Error::other("Bluetooth descriptor handoff failed"));
    }
    let fd = result.map_err(std::io::Error::other)?;
    if fd < 0 {
        return Err(std::io::Error::other("Bluetooth descriptor expired"));
    }
    // SAFETY: native detached this descriptor exactly once in the synchronous
    // call above. Rust immediately owns it, before any await or cancellation.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}
