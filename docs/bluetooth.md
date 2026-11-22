# Experimental Bluetooth implementation

Linux BlueZ and Android have native BLE/GATT discovery and Bluetooth Classic
RFCOMM backends in the uncommitted working tree. The user confirmed Bluetooth
startup and discovery between the Galaxy S25 and Linux. The Android-initiated
RFCOMM connection then failed. The Linux listener fix below has been checked
locally on BlueZ 5.88. An end-to-end connection, TLS pairing and file transfer
over Bluetooth are pending physical verification. Windows currently supports
LAN sharing.

## Linux RFCOMM listener fix

BlueZ accepted `RegisterProfile` for InvarDrop's custom server UUID with no
`Channel` option. Its generated SDP record had no RFCOMM channel, and BlueZ
opened no RFCOMM listener. The advertised UUID alone could appear in the
controller's UUID list. A local comparison on the powered BlueZ 5.88 adapter
showed no new RFCOMM listener with the option omitted and a listener on channel
1 with `Channel` set to D-Bus `uint16` zero. Both temporary profiles were
unregistered after the probe.

InvarDrop now requests automatic RFCOMM channel allocation in `RegisterProfile`.
BlueZ inserts the channel it bound into its generated SDP record for the same
per-run UUID published in the GATT bootstrap. Link authentication and BlueZ
authorization remain required. Android dials by SDP UUID using the secure
socket API. The Linux startup check verifies that the custom service UUID was
published before advertising. BlueZ may log listener errors in the journal
even when `RegisterProfile` returns success.

Android logs secure socket creation, OS bond state, socket connection, routing
preface and native descriptor handoff under `InvarDropBluetooth` and
`RustStdoutStderr`. Linux logs its direct and reverse paths and BlueZ listener
callbacks under `InvarDrop RFCOMM` on stderr. The phone was absent from ADB
during investigation. The Linux paired-device list did not show a Galaxy bond.
The new logs and OS prompts can distinguish a later bonding or routing failure
from SDP and listener setup.

## Galaxy S25 startup failure and fix

The original Galaxy S25 startup failure occurred with all three permissions
granted:

| Android permission | Confirmed device state |
| --- | --- |
| `android.permission.BLUETOOTH_SCAN` | Granted |
| `android.permission.BLUETOOTH_ADVERTISE` | Granted |
| `android.permission.BLUETOOTH_CONNECT` | Granted |

ADB confirmed all three grants, an enabled Bluetooth radio, Android API 37,
and an installed APK matching the previous debug APK hash below. The original
startup error did not retain an exception trace. Source inspection found a
deterministic failure before RFCOMM setup: Rust serializes a version-1 bootstrap
with five fields, including `address: null` on Android. Kotlin required six
fields and rejected every local bootstrap at that check.

Android now validates the named version-1 schema, requires the four core fields,
accepts the optional routing address, and rejects unknown fields. The local
Android bootstrap still cannot publish a Classic address. The UI preserves the
native stage and Logcat records exception types, callback status codes and
timeouts under `InvarDropBluetooth`. The app reaches `scanning` or `ready` after
GATT registration and a successful advertising callback.

The user subsequently confirmed startup and discovery on the Galaxy. The earlier
Galaxy transfer and Downloads checks used LAN.

## Architecture and implementation map

The current connection path is:

1. mDNS/DNS-SD and BLE/GATT produce unauthenticated nearby candidates.
2. The native connection actor attempts LAN first and may fall back to Bluetooth
   for eligible connection failures. Names and advertised addresses establish no
   identity or trust.
3. TCP or RFCOMM provides an owned asynchronous byte stream to the shared rustls
   TLS 1.3 layer. On reverse RFCOMM connections, socket direction and TLS roles
   are coordinated independently.
4. The existing identity, trust and pairing actor authenticates the peer. The
   transfer actor then applies receiver approval, bounded framing, BLAKE3,
   private staging and explicit resume rules.

| Source | Responsibility |
| --- | --- |
| [`bluetooth/mod.rs`](../src-tauri/src/bluetooth/mod.rs) | Controller, bounded route tracking, status snapshots and connection requests. |
| [`bluetooth/protocol.rs`](../src-tauri/src/bluetooth/protocol.rs) | Bootstrap validation, reverse requests and the RFCOMM routing preface. |
| [`bluetooth/linux.rs`](../src-tauri/src/bluetooth/linux.rs) | BlueZ D-Bus GATT discovery/advertising, RFCOMM profile registration and descriptor ownership. |
| [`bluetooth/android.rs`](../src-tauri/src/bluetooth/android.rs) | Tauri native bridge, incoming stream delivery and synchronous JNI descriptor handoff. |
| [`BluetoothPlugin.kt`](../src-tauri/android/BluetoothPlugin.kt), [`BluetoothPolicy.kt`](../src-tauri/android/BluetoothPolicy.kt) | Android permissions, BLE/GATT callbacks, secure RFCOMM sockets, bounded native pumps and policy. |
| [`secure/stream.rs`](../src-tauri/src/secure/stream.rs), [`secure/transport.rs`](../src-tauri/src/secure/transport.rs) | Generic asynchronous I/O and shared TLS identity/exporter verification. |
| [`secure/service.rs`](../src-tauri/src/secure/service.rs), [`secure/connect.rs`](../src-tauri/src/secure/connect.rs) | Connection lifecycle, independent security initialization and LAN-first fallback. |
| [`secure/protocol.rs`](../src-tauri/src/secure/protocol.rs), [`secure/transfer.rs`](../src-tauri/src/secure/transfer.rs), [`secure/resume_transfer.rs`](../src-tauri/src/secure/resume_transfer.rs) | Existing transfer messages and actors, with bounded Bluetooth timing budgets. |
| [`lib.rs`](../src-tauri/src/lib.rs), [`App.tsx`](../src/App.tsx), [`useBluetooth.ts`](../src/useBluetooth.ts), [`PairCard.tsx`](../src/PairCard.tsx) | Native command wiring, nearby-device presentation, explicit Bluetooth controls and selected transport. |
| [`build.rs`](../src-tauri/build.rs), [`bluetooth-permission.xml`](../src-tauri/android/bluetooth-permission.xml), [`Cargo.toml`](../src-tauri/Cargo.toml) | Managed Android sources/permissions and the Linux-specific D-Bus dependency. |

This table describes implemented code paths. It does not establish successful
execution of those paths on the Galaxy or a physical Linux Bluetooth controller.

## Operation and boundaries

Bluetooth sharing starts disabled. Enable it explicitly in both applications.
Android requests Nearby devices permission on API 31+; older supported Android
versions request the scan/location permission required by their platform. Radio
power is controlled through system settings, not changed silently by InvarDrop.
Bluetooth hardware is optional and permission denial does not disable LAN.

Sharing starts a filtered scan lasting up to 24 seconds. Android requests a new
Bluetooth permission only after an explicit action. With no reachable LAN peer,
an idle app can refresh its scan every 90 seconds while visible. A failed LAN
connection that qualifies for fallback can request a bounded search for the same
discovery ID. Fresh bootstrap results expire after 120 seconds. Settings offers
a manual scan.

On Android, native callbacks wake the Rust bridge when Bluetooth state, devices
or socket handoffs change. A slower health check detects radio or permission
loss. The activity pauses scanning and advertising when it moves to the
background, while existing transfer streams retain their own lifecycle.
Android pauses discovery/advertising while the activity is backgrounded and
restores advertising on return. Keep both applications foregrounded for the
initial hardware tests. There is no foreground service or battery exemption.
Scanning stops before initiating an RFCOMM connection. Advertising and bootstrap
connections still consume power; battery behavior has not been measured.

LAN candidates are attempted first, with the existing address restrictions and
five-second LAN connection budget. Bluetooth fallback is permitted for network
availability/I/O failures, including absence of a LAN endpoint. Certificate,
signature, pin, protocol, asymmetric-trust, revocation, rejection, and cancellation
failures do not switch to another identity or pairing flow. Established sessions
never migrate automatically, and interrupted transfers never resume silently.

The connection card displays the selected transport. OS Bluetooth bonding is
separate from InvarDrop identity verification. Complete any expected OS pairing
prompt, then compare the entire InvarDrop code on both intended devices and
approve on both when pairing is required. Previously trusted keys retain their
existing TLS verification behavior. Every incoming transfer and resume still
requires approval. Existing BLAKE3, private staging, publication, cancellation,
revocation, checkpoint rehashing, and explicit resume rules remain in force.

Stopping Bluetooth closes its sessions as interruptions, preserving eligible
checkpoints. Use the transfer's Cancel action to cancel and reset incomplete
data, or Discard to remove an inactive recovery record. The unchanged checkpoint
interval is 16 MiB; reaching one can take substantial time over Bluetooth.

## Bootstrap and identity

The advertised GATT service UUID is
`af8e6b90-6f6e-4e34-8a9e-4356fe9c3601`. Its read characteristic ends in `3602`;
Android's reverse-request write characteristic ends in `3603`. Bootstrap JSON is
limited to 512 bytes and contains a version, anonymous run UUID/name, random
per-run RFCOMM service UUID, and an optional Classic routing address. At most 32
discovered routes are retained. Persistent public-key fingerprints are not
advertised. Discovery names, IDs, addresses and capability hints are unverified.

Linux registers an authenticated RFCOMM server profile through BlueZ and serves
its Classic address through GATT. Android cannot depend on privileged access to
its own Classic address, so it advertises no Classic locator. When Linux initiates
a connection to Android, it writes its own locator and a one-use random token to
Android's GATT request characteristic. Android dials the Linux RFCOMM listener;
the token correlates that socket with the pending Linux attempt. Tokens expire
with their attempt and never establish trust. Requests are bounded and rate
limited; enabling sharing allows nearby devices to request connections and may
produce an OS bonding prompt. Android-to-Android routing without a desktop locator
is not implemented.

A 25-byte `IVDBT001` routing preface precedes TLS on RFCOMM only. It carries a role
marker and optional reverse-request UUID, with a three-second incoming deadline.
It contains no file metadata or credentials. The subsequent stream uses the same
mutual TLS 1.3, ALPN `invardrop/6`, `/5`, `/4`, certificate signatures, SPKI pins,
exporter pairing code and application messages as TCP. Expected pins survive
fallback. The nonce and OS bond cannot replace those checks.

Rust owns asynchronous byte streams. On Linux, owned BlueZ RFCOMM descriptors
use nonblocking Tokio readiness. On Android, bounded native workers bridge
BluetoothSocket streams through a kernel socketpair to Rust. Only encrypted TLS
bytes pass through that socketpair. File data is never encoded into Tauri IPC.
Opaque handoff handles cross the native bridge; JNI transfers descriptor
ownership synchronously so asynchronous response cancellation cannot lose a raw
descriptor. Dropping the Rust endpoint is designed to terminate the native
connection. This Android handoff still requires physical runtime verification.
Bluetooth frame-send
and receive-idle budgets are 120 seconds; LAN retains 20 seconds. Consent deadlines
and protocol/storage limits remain unchanged. No unlimited slow-peer deadline is
introduced.

## Completed automated tests and builds

For the RFCOMM fix, the complete Rust suite passed with 188 tests, zero failures
and one existing opt-in test ignored on a two-worker rerun. Two existing
transfer-recovery tests failed in the first run while builds were active. The
rerun passed both. Rustfmt, strict host Clippy and the Android JVM checks passed.
The Linux debug executable and Android ARM64 debug APK were rebuilt. The APK
passed signature and 16 KiB ZIP alignment verification.

| RFCOMM fix artifact | SHA-256 |
| --- | --- |
| Linux debug executable | `9a3b6b687f5ece2265396b9dca549aebeaa1681c7afd69cad62217f15e565eb4` |
| Android ARM64 debug APK | `62af404fa86ab6cb5d843de784ccf8c65e6954b5666acd6856162fd56de76821` |

The preceding implementation run recorded 187 Rust tests passed, zero failed,
and the existing opt-in large-file test was ignored. This includes actual TLS
over bounded memory streams and owned Unix socketpairs, approval on both sides,
wrong-pin rejection, transfer approval/integrity, cancellation, explicit resume,
and synchronous Tauri setup. Existing TCP/TLS process tests also passed. The
first sandboxed full run could not bind loopback sockets; the complete passing
run used permitted loopback access. No automated test used a Bluetooth adapter.

All 88 frontend tests, TypeScript checks, host strict Clippy, Rustfmt, a locked
Linux release build, the existing Android JVM helper tests and 18 Bluetooth
policy assertions passed. Android ARM64
Rust checking, native Kotlin compilation and a debug APK build passed. The APK's
native library and ZIP alignment support 16 KiB pages; this does not establish a
16 KiB-kernel runtime result. Android reports compatibility API deprecations and
the existing unused desktop-processing-method warning. Windows was not built or
tested locally.

| Completed check | Evidence boundary |
| --- | --- |
| Rust suite: 187 passed, 1 existing opt-in large-file test ignored | Includes existing TCP/TLS process regressions and new in-memory/owned-socket tests; no Bluetooth radio interoperability. |
| Four in-memory TLS actor tests | Pairing, wrong-pin rejection, bidirectional BLAKE3 transfer, approval enforcement, cancellation and explicit resume over bounded memory streams. |
| Owned-socket and synchronous-startup regressions | Rust descriptor ownership/read cancellation and attachment from synchronous Tauri setup; no Android JNI or BluetoothSocket runtime proof. |
| Bootstrap/preface and fallback-policy tests | Input bounds, malformed routing metadata and exclusion of security/consent failures from fallback; no physical discovery or failover proof. |
| Frontend: 88 passed | UI actions and state presentation, including Bluetooth-only candidates; no native radio execution. |
| JVM helpers and 18 Bluetooth policy assertions | Existing SAF/Downloads helpers and pure permission/validation policy; no Android permission dialog, GATT or RFCOMM operation. |
| TypeScript, Rustfmt and strict host Clippy | Static checks; no cross-device execution. |
| Linux release and Android ARM64 debug APK | Compilation and packaging succeeded; Galaxy startup subsequently failed as reported above. |
| Android ELF/APK 16 KiB alignment | Artifact layout only; no 16 KiB-kernel runtime test. |

These results describe the first Bluetooth build.

For the startup fix, the full Rust suite passed with 187 tests and one existing
opt-in test ignored. All 88 frontend tests, TypeScript, Rustfmt, host strict
Clippy and the Android JVM helper suite passed, including 23 Bluetooth policy
assertions. The rebuilt ARM64 debug APK passed signature and 16 KiB ZIP alignment
verification with the existing debug signing certificate. Startup-fix APK SHA-256:
`09f266e3a9286e9052fc60b662a42b78531934306a7e675945241ea45b504527`.

Previous debug APK SHA-256:
`82b602fe9019b504883c1ec17d3f62ac5413b37c4737f4587aa439a02291e84d`.
Its debug signing certificate SHA-256 is
`a93d2e3337df9894946e518cd4b07997683a2cd1981ad76e8ba89e895007829b`.
These identify the local uncommitted testing artifact, not a published release.
Linux executable SHA-256:
`217bb6df1372325e3e8f2274e2012737761835b1e6c0ab07c909f88ea1c1fe79`.

## Known issues and verification limits

- **End-to-end RFCOMM verification is pending.** Use the rebuilt Linux executable
  and the matching Android diagnostic APK, then capture logs from both devices.
- BLE advertisements and even readable GATT metadata do not guarantee an
  available RFCOMM service. OS bonding, controller capabilities and BlueZ
  authorization still need physical verification.
- Android-to-Android routing without a desktop Classic locator is not
  implemented. Windows Bluetooth is not implemented; its current role is LAN.
- Android background reliability, controller coexistence, long-transfer
  throughput and battery consumption have not been established. No foreground
  service, wake lock or battery exemption is provided.
- The 16 MiB checkpoint interval can mean substantial retransmission time over a
  slow Bluetooth link. Smaller intervals or protocol changes are not implemented.
- Current uncommitted Windows compilation/runtime behavior has not been checked
  locally. Earlier Windows CI results apply to their recorded commits.

## Remaining physical tests: CachyOS and Galaxy S25

Update the APK in place and keep existing profiles, application data, Keystore
entries, OS credentials and bonds. Retest the Android-initiated RFCOMM connection
first, then the Linux-initiated reverse connection.

| Gate | Required physical evidence | Current state |
| --- | --- | --- |
| Successful startup | Demonstrate Bluetooth sharing reaches an operational state with the existing identity and trust preserved. | User reported `ready` on both devices after the startup fix. |
| BLE/GATT discovery | Mutual appearance, fresh bootstrap reads, expiry and rescan. | User reported discovery, including the Linux peer appearing on Android. |
| RFCOMM and bonding | Android-initiated connection and Linux-initiated reverse connection, including expected OS prompts. | BlueZ listener allocation verified locally. End-to-end retest pending. |
| TLS identity and pairing | Same existing identity, saved-key reconnect, full-code comparison when required, rejection and revocation. | Automated stream coverage only; physical Bluetooth pending. |
| File integrity and approval | Approved small transfers in both directions without LAN, matching independent BLAKE3 hashes, no unapproved/public partial receipt. | Not verified over Bluetooth. |
| Interruption and resume | Persist a verified checkpoint, interrupt the link, reconnect without payload, then require sender action and fresh receiver approval. Test Cancel and Discard separately. | Automated stream coverage only; physical Bluetooth pending. |
| Transport selection | Prefer a working LAN on a new connection; fall back to Bluetooth on eligible connection failure; never migrate active payload or retry security rejection silently. | Policy/stream coverage only; physical mixed-transport test pending. |
| Lifecycle and performance | Permission denial/revocation, radio restart, app restart, background/screen lock, long files, speed and battery observations. | Not verified. |

Keep the existing permission grants, trust records and OS bonds while collecting
diagnostics. Use the normal TLS verification and approval flows.

### Check Linux prerequisites

```sh
systemctl status bluetooth --no-pager
rfkill list bluetooth
bluetoothctl list
bluetoothctl show
```

Use system settings to enable the adapter if necessary. BlueZ must expose
`GattManager1`, `LEAdvertisingManager1` and `ProfileManager1`. InvarDrop uses system
D-Bus without raw HCI privileges or shell Bluetooth helpers. If no desktop bonding
agent is running, keep an interactive terminal open:

```text
bluetoothctl
agent KeyboardDisplay
default-agent
```

Approve only the expected phone's OS request. Do not mark arbitrary devices
trusted, disable authentication, reset bonds, or grant raw HCI capabilities.
Leave existing firewall policy intact; Bluetooth RFCOMM does not use a TCP port.

### Build and update without resetting identity

From the repository root, these commands prepare local artifacts:

```sh
npm run tauri -- android build --debug --target aarch64 --apk
cargo build --offline --locked --manifest-path src-tauri/Cargo.toml
```

The desktop executable is `src-tauri/target/debug/invardrop`. The Android APK is
normally at
`src-tauri/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk`.
Use the exact artifact path reported by the build if it differs.

Connect the Galaxy through USB debugging and run:

```sh
adb devices
adb install -r src-tauri/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk
```

If more than one device is listed, add `-s SERIAL` to each ADB command. If the
update reports a signing-certificate mismatch, stop: do not uninstall, clear app
data, or replace the existing signing key. Reopen the app and confirm that the
public identity and trust list are unchanged.

Close the older desktop instance, then run the rebuilt debug executable from
the repository root. Start Logcat before selecting Connect.

```sh
src-tauri/target/debug/invardrop 2>&1 | tee /tmp/invardrop-rfcomm-linux.log
adb logcat -T 1 -s InvarDropBluetooth:I RustStdoutStderr:I BluetoothSocket:D
bluetoothctl devices Paired
journalctl -u bluetooth -f
```

Run those commands in separate terminals. Compare the Linux server SDP UUID
with the Android target UUID. For direct connections, look for Linux
`NewConnection` followed by `incoming routing preface accepted`. For reverse
connections, look for `reverse GATT request accepted` followed by `reverse
routing preface accepted`. On Android, check the bond state, `BluetoothSocket.connect`
and descriptor acquisition before interpreting a TLS error.

### Verify Bluetooth independently of LAN

1. On the phone, turn Wi-Fi off and disable USB tethering. Leave Bluetooth on.
   USB debugging can remain connected. This prevents the phone from using the
   computer's LAN path for the test.
2. Open InvarDrop on both devices. A device with an existing permission grant
   can start discovery automatically. Use Find more devices on a fresh Android
   install to request nearby device permission. Keep both apps in the foreground.
   Settings offers Scan now if the devices were opened at different times.
3. Choose a file and select the nearby device, then press Send. Start on Android.
   After the transfer, use Disconnect in Settings under Connection diagnostics,
   then send from Linux to exercise reverse routing. Check any expected system
   bonding prompt.
4. Check Connection diagnostics on both devices for Bluetooth RFCOMM. Compare
   all pairing code groups on both intended devices and approve when new pairing
   is required.
5. Generate a small source and its reference digest on Linux:

   ```sh
   mkdir -p /tmp/invardrop-bt-check
   dd if=/dev/urandom of=/tmp/invardrop-bt-check/probe.bin bs=1M count=1 status=none
   b3sum /tmp/invardrop-bt-check/probe.bin
   ```

6. Select that file through InvarDrop and approve it on Android. Confirm there is
   no public receipt before approval/completion. Find the completed public copy:

   ```sh
   adb shell find /sdcard/Download/InvarDrop -name probe.bin
   ```

   Copy the exact reported receipt path with `adb pull`, for example:

   ```sh
   adb pull '/sdcard/Download/InvarDrop/received-TRANSFER-ID/probe.bin' /tmp/invardrop-bt-check/android-received.bin
   b3sum /tmp/invardrop-bt-check/probe.bin /tmp/invardrop-bt-check/android-received.bin
   ```

   Replace `TRANSFER-ID` with the reported directory. The digests must match.
   Select the Android receipt through its native picker, send it back, approve
   on Linux, and compare the returned receipt's digest as well.
7. Generate a 20 MiB source with `count=20`, transfer beyond the displayed 16 MiB
   verified checkpoint, then switch off one radio or stop Bluetooth sharing.
   Re-enable, rescan and reconnect. Confirm there is no automatic payload.
   Resume must require sender action and fresh receiver approval. Verify the
   final digest, then separately test Cancel and Discard.
8. Restore the phone's original connectivity settings. Later, with both devices
   on a working LAN, confirm that a new connection selects LAN. Interrupting an
   active transfer must still require explicit recovery rather than migration.

Record the APK hash, OS versions, controller information, initiation direction,
selected transport, pairing/approval outcome, final hashes and any exact error.
Do not record pairing codes, keys or file contents. Useful local diagnostics:

```sh
journalctl -u bluetooth -b --no-pager -n 100
adb logcat -T 1 -s InvarDropBluetooth RustStdoutStderr BluetoothLeScanner
```

Throughput, long transfers, permission revocation, radio restart and background
behavior remain hardware verification items. Windows Bluetooth implementation
and physical Windows testing remain subsequent work.
