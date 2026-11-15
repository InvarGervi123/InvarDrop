# Milestone 2.1: Local Network Discovery

## Protocol and trust boundaries

mDNS is multicast DNS over UDP 5353; DNS-SD describes services using PTR/SRV/TXT
records and A/AAAA addresses. Applications query the local network for
`_invardrop._tcp.local.` and publish their own records. No external DNS, accounts,
or server are required.

The implementation uses [mdns-sd 0.21.7](https://github.com/keepsimple1/mdns-sd),
licensed under Apache-2.0/MIT. The library supports Linux and Windows without
Avahi/Bonjour, tracks network interfaces, provides a daemon monitor, and supports
graceful shutdown. Its unused async/logging features are disabled. The application
uses one worker and one library thread; no additional async runtime was added.

The advertisement contains only:

| Field | Value |
|---|---|
| Service instance | Random UUID v4 for the current run |
| Hostname | `invardrop-<uuid>.local.`, rather than the system hostname |
| SRV port | `0`: this milestone has no transfer endpoint |
| TXT `id` | Run UUID |
| TXT `name` | `InvarDrop <first 8 characters of UUID>` |
| TXT `v` | Discovery protocol version `1` |
| TXT `mode` | `discovery` |
| TXT `beat` | Update counter for the current run |

Local IP addresses are published as a necessary part of mDNS. The OS name,
username, MAC address, directories, files, and persistent machine ID are not
advertised. The ID and name change after a restart. This is a deliberate privacy
tradeoff: a persistent trusted identity will only be introduced with authenticated
pairing.

**mDNS is unauthenticated.** Any LAN participant can spoof a name or UUID. The UI
list displays `Not paired`; discovery must never authorize a transfer. SRV port 0
must not be interpreted as a ready transfer endpoint. This milestone does not read
or transfer files or directories. A future transport requires a separate endpoint,
protocol validation, confirmation, and TLS/pairing.

## Lifecycle

1. The worker creates a daemon, monitor, advertisement, and browse subscription.
   A successful outgoing advertisement confirms discovery availability. If no first
   advertisement appears within 10 seconds, or 30 seconds elapse after the latest
   advertisement, the session restarts. This also detects silent failures to create
   multicast sockets in the library.
2. Every 20 seconds, it changes the TXT heartbeat. Re-registration updates the record.
3. The parser validates the service type, version, mode, UUID, heartbeat, and name
   (at most 80 UTF-8 bytes, without control or bidi-control characters).
4. The tracker stores DNS sources separately and combines them into one device per
   UUID. It excludes its own UUID but permits another process on the same IP.
5. Goodbye/`ServiceRemoved` removes a source. Without a new heartbeat, a source
   expires after 75 seconds on a monotonic clock, regardless of a long DNS TTL.
   Repeated cached responses with the same heartbeat neither extend the deadline
   nor revive an expired/goodbye record. The latest marker for that source remains
   in the same bounded cache; inactive records are evicted first when space is
   needed for new devices. This does not protect against mDNS spoofing.
6. A daemon error or channel closure clears the list and marks the UI unavailable.
   After 5 seconds, the worker attempts to create a new session with the same
   session ID. The library checks IP changes every 2 seconds.
7. On normal exit, the worker stops browsing, unregister sends goodbye, and shutdown
   waits for daemon acknowledgement. The application joins the worker. Daemon
   waits are bounded; a crash cannot send goodbye, so peers use expiration.

Heartbeat updates add a small amount of multicast traffic. The 75-second interval
allows several missed updates while providing predictable removal of a crashed
peer. It does not check the availability of a future TLS server.

The tracker is limited to 512 DNS sources; each worker iteration processes at most
128 events per channel. This bounds application state but does not protect the
entire network stack against a hostile LAN or packet flood.

## Rust to React

`discovery_snapshot` returns the current state, and `discovery-updated` delivers
changes only to the local `main` window. A snapshot contains a revision, the name/ID
of this run, service status, and peer names/IDs. IP addresses and heartbeats are not
sent to the UI. The revision changes only when visible state changes.

React registers its listener before reading the initial snapshot. It accepts only
newer revisions, so a late command response cannot overwrite a newer event. The
listener is removed on unmount, including the async subscription race in StrictMode.
The UI does not poll the network or require refresh, filesystem, shell, or arbitrary
Tauri event permissions. Names are rendered as ordinary React text.

## Validation

```sh
npm ci
npm run check
npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo clippy --locked --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --locked --manifest-path src-tauri/Cargo.toml
npm run tauri build -- --no-bundle -- --locked
```

Unit tests do not require multicast. They cover tracking, deduplication, alias
removal, self-filtering, expiration/heartbeats, record limits, input validation, and
snapshot revisions. CI runs on Linux and Windows, including `feature/**` branches.

For an actual network test, start two application instances or diagnostic processes
(these publish real mDNS advertisements, rather than simulate discovery):

```sh
cargo run --locked --manifest-path src-tauri/Cargo.toml --example discovery_probe -- 90
```

The number is the duration in seconds. Two processes on one computer verify real
multicast and distinct identities, but **do not verify communication between two
physical devices**. That requires two devices on the same LAN.

Manual scenario: start A and check that its list is empty; start B and check that
both appear without refresh; leave both running for more than 75 seconds; close B
normally and check its removal; restart B, terminate it abruptly, and check removal
within approximately 75 seconds of its last heartbeat.

## Network environment

Wi-Fi client isolation, VLANs, multicast filtering, VPNs, firewalls, and multiple
network interfaces can affect results. mDNS UDP 5353 and IPv4/IPv6 multicast groups
must be allowed on the trusted LAN. The application does not change firewall rules,
install network services, or scan addresses. Windows runtime/multicast and firewall
behavior need verification separately from CI builds.

## Validation performed on CachyOS

- 13 unit tests; Rustfmt, strict Clippy, and a frontend production build.
- Two independent native processes: mutual discovery and self-filtering,
  retention of a live peer beyond 75 seconds, and removal after normal exit.
- Abrupt termination of the second process without goodbye: the record was absent
  at the 94-second checkpoint. A unit test with a monotonic clock separately
  verifies the 75-second expiration boundary.
- Native UI checked at 360 × 420, including the actual discovered peer name.
- After adding the watchdog, the production version was rebuilt and mutual
  discovery was verified again with an independent diagnostic process.
- `socket()` failure was injected with seccomp only into a child diagnostic
  process: unavailable at 10 seconds, automatic retry at 15 seconds, unavailable
  again at 25 seconds, and clean stop/exit 0 at 27 seconds. OS firewall/network
  settings were unchanged. Only the external test used libseccomp; it is not an
  application dependency.

Separate physical devices, Windows runtime/multicast, Wi-Fi/VPN transitions,
firewall scenarios, and prolonged packet loss have not yet been tested.
CI verifies builds/tests but does not establish network compatibility between devices.
