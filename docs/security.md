# Milestone 2.2 — TLS and Pairing

Current Milestone 2.7 supplement: [resumable transfers](resumable-transfers.md).

The Milestone 2.6 protocol, folders, limits, and compatibility with 2.5 are described
in [large-files-folders.md](large-files-folders.md). The historical context of this
milestone is retained below.

This document records the protocol and results of Milestone 2.2, merged into `main`
through PR #6. Milestone 2.3 preserves identity/trust and pairing rules but uses
ALPN `invardrop/2`, hello version 2, and new file messages. Current transfer
extensions and boundaries are documented in [transfers.md](transfers.md).

## Scope and threat model

The goal is a verified encrypted connection between InvarDrop instances without an
account, cloud server, or public CA. This milestone does not transfer files.

A LAN attacker is assumed to be able to spoof mDNS, addresses, and names; operate
TLS endpoints; intercept/redirect connections; send malformed messages; replay
requests; and disconnect the network. A name resembling a known device must never
be sufficient to establish trust.

The OS, application/WebView integrity, cryptographic libraries, and the user
comparing **all** code groups on the two intended devices over an independent
channel are trusted. Compromise of the current account, an administrator, Secret
Service/Credential Manager, or the GUI is outside the threat model. Any process
under the same user may have keyring access; the keyring is not an HSM. The project
has not undergone an independent audit or formal protocol verification.

## Cryptographic identity

- An ECDSA P-256 key pair (`rcgen`/`ring`) is generated per profile.
- A self-signed X.509 certificate contains the public key; it is not a CA identity.
- Trusted identity = SHA-256 of DER SubjectPublicKeyInfo (SPKI), 256 bits.
- The private key and certificate persist. The temporary mDNS UUID is not a
  cryptographic identity. Persistent fingerprints are not advertised in mDNS.
- Loading checks that the private key matches the certificate. A corrupted or
  missing part of an existing identity is never replaced automatically.

TLS uses stable rustls 0.23 with the ring provider and tokio-rustls. Only TLS 1.3 is
allowed; ALPN `invardrop/1` is required. TLS resumption, tickets, and 0-RTT are
disabled; key logging is not enabled. Both sides must present a certificate and
prove key possession through standard rustls TLS CertificateVerify signature
verification.

The verifier deliberately does not derive trust from Web PKI, DNS hostnames,
certificate CN/SAN, or validity dates. An unknown key's certificate is admitted
**only into pairing quarantine** after ASN.1/SPKI validation; TLS signatures are
never skipped. For an explicitly specified known-peer pin, another SPKI is rejected
during the TLS handshake without falling back to new pairing. The local key is
rejected. Changing the certificate while keeping its SPKI does not change identity;
rotating the key requires new explicit pairing. Trust is based on keys, rather than
certificate expiry.

## Comparison code and consent

After TLS, both sides obtain 16 bytes from the TLS exporter with label
`EXPORTER-InvarDrop-Pairing-v1` and a fixed-format context:
`client:<client-SPKI-hash>;server:<server-SPKI-hash>`.

The code is all 128 bits of exporter output, displayed as 8 groups of 4 hexadecimal
characters. Names, mDNS UUIDs, and advertisements do not participate in its
calculation. The code is never sent to the peer as an application message or
written to logs. It gives the user a way to bind both screens to one TLS channel
and both proven key pairs. Completing TLS with an unknown key **does not
authenticate the intended device**.

First connection:

1. TCP → mutual TLS 1.3 → ALPN, key, self-identity, and revocation checks.
2. The peers exchange `hello(version=1, initiator, trusted)` inside TLS.
3. If both sides do not trust each other's keys, the UI displays the full code. On
   **each** device, the user compares the entire code, checks the checkbox, and
   clicks `Codes match — pair`. There is no autofocus/default Enter action, TOFU,
   or automatic confirmation.
4. Only `decision(approve=true/false)` is sent over TLS. The backend accepts local
   approval only for the current connection ID, matching code, correct phase, and
   unexpired deadline. Duplicate/stale approvals are rejected.
5. Only after both approvals may the session enter `committing`, hide the code,
   and persist trust. Both sides then send `ready`; the UI displays
   `Secure connection` only after the remote ready arrives.

Reject before local approval cancels the session. Cancel after sending approval
closes the connection but **does not revoke previously saved trust**. Revoke trust
is the separate revocation action. Cancellation before commit must not create local
trust; a blocking filesystem operation after both approvals may finish despite
network cancellation. These boundaries must not be hidden behind a claim of
atomicity. If persistence completes after connection cancellation, the persistence
task still updates the UI trust list: the saved key remains visible for Revoke trust.

On reconnect, both sides must know the corresponding SPKI; no new code or approval
is required, and TLS verifies key possession again. Asymmetric trust closes the
connection without silently switching to pairing. To reconnect after a partial
commit error, explicitly remove/revoke old trust on both sides, allow new pairing,
and compare the code again. There is no global transaction between devices.

The abbreviated UI fingerprint is for navigation and **does not replace the full
pairing code**. A test with two MITM TLS channels confirms different codes in that
scenario; it is not a formal proof against all MITM attacks. If the user does not
compare the entire code on the intended devices, the required authentication is
not achieved.

## Storage and revocation

- The OS keyring stores a random 32-byte master key: Linux Secret Service or
  Windows Credential Manager (`keyring` v4). There is no plaintext or temporary
  identity fallback. If the keyring is unavailable/locked, the secure listener
  does not start.
- `identity.enc` contains versioned JSON identity/trust/revocation state protected
  by ChaCha20-Poly1305 (`ring`), with a fresh random 96-bit nonce per write and the
  version header as authenticated associated data. Cryptographic primitives are
  provided by established libraries.
- Updates use a tempfile, `sync_all`, atomic replacement, and directory sync on
  Unix. The Unix profile directory uses mode 0700 and the tempfile mode 0600.
  Windows inherits the user app-data directory's ACLs; contents also have AEAD
  protection.
- An exclusive OS file lock prevents two writer processes for one profile.
- Limits are 64 trusted / 256 revoked identities and a 64 KiB file before parsing.
- Revoke removes trust, saves a revoked marker, and closes active sessions.
  A revoked key cannot be accepted automatically or trusted again without Unblock.
  `Allow new pairing` only removes the block; it does not restore trust.
- Failure to save new trust does not authorize a peer in memory. If saving a
  revocation fails, trust is denied in the current process, connections close,
  and the UI receives an error. **Such revocation is not durable**: old state may
  remain on disk after restart. Resolve the error and repeat the revocation.
- AEAD does not detect rollback to an older correctly encrypted backup.
  Filesystem rollback/local-account compromise is outside this milestone's guarantees.

The default profile is `security-v1` in Tauri's user app-data directory. Independent
native tests can set an absolute `INVARDROP_PROFILE`. Each such profile gets a
separate keyring entry bound to the hash of its canonical path. Do not move/delete
the profile and credential independently: losing either part must cause failure,
never a silent key change. This milestone has no identity export/recovery/rotation UI.

## Discovery, protocol, and resources

Discovery v1 (`mode=discovery`, port 0) is still recognized, but Connect is disabled.
A ready secure listener advertises v2, `mode=secure`, and its actual TCP port.
Routing fields are unauthenticated. IP/port never reach React: Connect accepts a
discovery ID, and Rust obtains current endpoints from the tracker. Pinned reconnect
verifies the endpoint against the expected key, rather than a name/IP.

Dialing is limited to private/link-local/loopback IPv4 and ULA/link-local/loopback
IPv6; public/global IPv6 endpoints are not used. This is a deliberate LAN routing
restriction, not cryptographic address authentication. The listener prefers IPv6
dual-stack with an IPv4 fallback. IPv6-only/global-prefix LANs, VPNs, firewalls,
and multiple physical devices need separate testing.

Application frames exist **only inside TLS**: a 4-byte big-endian length followed
by up to 2048 bytes of JSON. `hello`, `decision`, `ready`, `ping`, and `pong` are the
only types; extra fields, unexpected phases, wrong versions/roles, and oversized
frames are rejected. No file/message transfer API exists in this milestone.

Limits: 8 simultaneous sessions, 30 attempts/minute, backlog 64, up to 8 addresses
per attempt, TCP dial 500 ms/address within a total TLS deadline of 5 s,
hello/ready deadline 5 s, pairing 90 s, idle timeout 20 s, keepalive 5 s, and up to
64 received keepalive frames/minute/session. The incoming-frame queue holds 4,
UI actions 2, and history 24 entries. A dedicated async TCP-reader task prevents
UI-wait cancellation from losing a partial frame. Cancellation and shutdown close
connections, perform TLS shutdown where TLS is established, and finish tasks.
The OS keyring wait is a blocking operation outside the GUI thread; it must not be
considered instantly cancellable. The listener is not opened before it completes.

These limits bound application resources but do not guarantee availability on a
hostile LAN: an attacker can occupy slots, jam multicast, or interrupt TCP.

## Validation and reproduction

```sh
npm ci
npm run check
npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo clippy --locked --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --locked --manifest-path src-tauri/Cargo.toml
npm run tauri build -- --no-bundle -- --locked
```

Rust tests use real TCP/TLS sockets, cryptography, and AEAD files, but keep test
storage master keys in memory instead of the system keyring. This is not a
production fallback. Tests cover matching keys/codes, both approvals, rejection,
expiration, malformed/oversized messages, wrong pins, two distinct MITM channels,
reconnect, restart with persisted keys, revoke, corruption, write failures,
exclusive profiles, and terminating stalled handshakes. Native keyring/UI behavior
must be tested separately.

Native scenario: open two profiles, Connect, and verify there is no automatic
approval; compare the entire code on both screens; approve on both; verify
Secure connection, Disconnect/Reconnect, profile restarts, Revoke trust, and
rejection of subsequent connections. Check Reject and Cancel separately. Do not
print pairing codes, private keys, or TLS exporter output in test logs.

## Regression results — 2026-11-13

Local environment: CachyOS x86_64, Niri/Wayland, Linux 7.3.3, Rust 1.99.1,
Node.js 26.13.1, npm 12.2.2; CI uses Node.js 24.

- Full `cargo test --locked`: 40 passed, 0 failed, no ignored tests. Real TCP/TLS
  tests require permitted sockets and ran outside the sandbox.
- Rustfmt, Clippy `--all-targets -- -D warnings`, both TypeScript checks, Vite
  production build, and a locked Tauri release build without bundles passed.
- Asymmetric-trust and partial-TCP-frame regressions during local approval were
  preserved and completed. Added coverage for duplicate/stale approvals,
  cancellation after one-sided consent, self-identity, revocation during pairing,
  fresh nonces, malformed/oversized storage, and revoke/unblock errors.
- Fixed a cancellation race during commit that could omit a saved key from the UI
  snapshot. A deterministic test blocks storage during cancellation, then checks
  publication of the completed commit and availability of revocation.
- Trust/revocation reads and snapshot publication are serialized: concurrent refresh
  can no longer overwrite a newer revocation with an old list carrying a newer
  revision. A controlled thread-scheduling regression reproduces the old failure
  and passes with the fix; the external notifier is called without holding the mutex.
- Two independent production GUI processes with separate `INVARDROP_PROFILE`
  values used real Linux Secret Service, mDNS, and TCP/TLS. AT-SPI checks verified
  all eight code groups matched without printing their values, approval disabled
  before checking the checkbox, Reject, Cancel after one approval, both approvals,
  Disconnect, and pinned Reconnect.
- After normal shutdown and restart of both processes, tests verified unchanged
  public fingerprints and trust, connection without a new code, Revoke closing the
  connection, rejected reconnect, and Unblock without restoring trust.

For Windows, the locked `x86_64-pc-windows-msvc` dependency tree was checked:
keyring selects Windows Credential Manager; Unix permissions/directory sync are
guarded by `cfg(unix)`. CI runs native builds, strict Clippy, and tests on
`windows-2022` and `ubuntu-24.04`; current results apply to a specific commit in
[GitHub Actions](https://github.com/InvarGervi123/InvarDrop/actions/workflows/ci.yml).
There is no local Windows toolchain/runtime. CI does not test the Windows GUI,
Credential Manager, or runtime multicast.

Two physical devices, Windows runtime, IPv6-only/global-prefix LANs,
Wi-Fi/VPN/firewall transitions, prolonged packet loss, an unavailable/locked system
keyring, and independent auditing remain separate checks. This milestone has no
file transfer, identity export/recovery, or cross-device trust transactions.
