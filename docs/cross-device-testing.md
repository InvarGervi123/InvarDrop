# Milestone 3.1 — Cross-device testing and reliability (Step 13)

## Scope and isolation

The automated device harness runs two independent operating-system processes of
the Rust test executable. Each runs the production secure `Service`, TLS stack,
pairing state machine, transfer actors, encrypted identity/trust store, and resume
catalog. Each has a distinct randomly generated identity, random storage master
key, absolute temporary profile, receive directory, recovery state, and ephemeral
TCP listener. All test-owned profiles are removed with their temporary directory.

The only storage substitution is the OS keyring backend: test master keys are
passed through anonymous stdin pipes and retained in the parent process for
restart. They are never written into fixture files, environment variables, logs,
or repository content. This does **not** test Linux Secret Service or Windows
Credential Manager. It cannot open or modify the real application's profile.
The harness is compiled only with `cfg(test)`; there is no production test mode,
automatic approval path, TLS bypass, or new frontend command.

The parent controls explicit user actions over private pipes. Transfer payloads,
pairing decisions between peers, acknowledgements, and resume proofs travel over
actual TCP sockets and mutually verified TLS 1.3. Pairing compares the complete
exporter-derived codes and explicitly approves both devices before transferring.
Codes are compared inside the test and never printed in reports.

For deterministic crashes, a test-only barrier pauses a receiver **after** payload
sync and successful atomic checkpoint persistence, before the acknowledgement.
The parent kills the process using the native OS process API, waits for its exit,
and starts a new process with the same test-owned profile and master key. This
tests abrupt process termination; it does not emulate a machine losing power or
a filesystem/device lying about flush completion. The barrier also exercises
cancellation at a durable checkpoint boundary. It never changes protocol frames
or integrity checks. Process scenarios run serially within each test executable
to bound CPU, memory, and disk pressure; all waits have deadlines.

## Reproducing the checks

```sh
# Existing audio regressions require a locally installed FFmpeg, never bundled.
# On Windows, INVARDROP_TEST_FFMPEG may name its absolute executable path.
cargo test --locked --manifest-path src-tauri/Cargo.toml cross_device_tests
cargo test --locked --manifest-path src-tauri/Cargo.toml
npm test
npm run test:packaging
npm run check
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo clippy --locked --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
npm run tauri build -- --ci -- --locked
```

Ordinary CI uses IPv4 loopback and ephemeral ports; no external service, account,
network fault tool, container, privileged network configuration, or additional
test dependency is required. The native Windows runner launches actual Windows
child executables and uses Windows filesystem and socket implementations.

Multicast discovery can additionally be tested on a host that permits mDNS:

```sh
INVARDROP_TEST_MDNS=1 cargo test --locked --manifest-path src-tauri/Cargo.toml \
  cross_device_tests::independent_devices_and_bidirectional_contents
```

This opt-in scenario starts production discovery in each process, waits for the
other anonymous discovery identity, and connects using its advertised endpoints.
It shares the host network; it does not prove LAN multicast routing or firewall
behavior between physical devices. Multicast is deliberately not mandatory on
GitHub-hosted runners, whose network policy is outside the application's control.

## New process-level regression coverage

Eleven automated tests cover the following scenarios. Some tests exercise
multiple cases; scenario rows are not additional test counts.

| Scenario | Verified result |
| --- | --- |
| Independent identities and bidirectional transfers | Different identities and ports; explicit two-sided pairing; zero bytes, small payloads, a 70-chunk payload, and accented/CJK filenames transferred both ways. The reverse transfer reads the receiver-owned receipt. Destination filenames, streaming BLAKE3 hashes and byte lengths are checked. |
| Folder and multi-selection | Nested paths, empty directory, zero-byte file, and two selected files; preserved structure and contents. |
| Untrusted and repeated pairing attempts | Sending while quarantined fails; two rejected pairing attempts create no trust; stale approval IDs and acceptance after decline fail; inbox stays empty. |
| Two abrupt file interruptions | A 33 MiB + 4 byte file survives two receiver kills and restarts of both processes; 16/32 MiB verified checkpoints persist; peer identities/trust persist; reconnect alone does not start receiving; each resume requires explicit approval; final contents match. |
| Abrupt nested-folder interruption | The same large file inside a nested folder resumes after restarting both processes; empty entries survive; final contents match. |
| Abrupt sender interruption | Killing the active sender before the receiver's checkpoint acknowledgement still permits recovery from the receiver's verified prefix after restarting both processes; final contents match. |
| Corrupted partial / changed source | Altering either the durable received prefix or source bytes makes the resumed authenticated channel fail without publishing a completed file. |
| Sender and receiver cancellation | Each side cancels at the checkpoint boundary; incomplete payload disappears, progress is reset, no completed receipt appears, source remains; Discard then removes durable recovery records. |
| Revocation across restart | A formerly trusted peer cannot reconnect to recovery after receiver revocation and restart; revocation persists and inbox remains empty. |
| Received ZIP processing | Real transferred bytes match the source; extraction never starts implicitly; approved extraction preserves nested/empty entries; a traversal archive fails extraction, leaves no published extraction, and retains the verified original ZIP. |
| Concurrent channels | Two authenticated connections carry opposite-direction transfers concurrently; a second offer on a busy channel fails; both received files match their sources. |
| OGG choice and actual output | Conversion without a selected converter fails before sending; keeping OGG succeeds unchanged; explicitly selecting FFmpeg and converting sends the actual MP3 name/size, decodable MP3 bytes, and preserves original OGG bytes. |

The unchanged existing regression suite additionally covers handshake/approval
expiry, stalled TCP/TLS clients, wrong pins and asymmetric trust, corrupted frames,
early/unapproved payloads, invalid offsets and manifests, traversal and symlinks,
destination collisions, >4 GiB sparse streaming where supported, resource limits,
progress monotonicity/throttling, cancellation/finalization races, missing/corrupt
checkpoints, prepared converted-folder restart/resume, converter cancellation and
failure cleanup, ZIP bombs, entry/depth limits, and extraction finalization races.
UI tests cover confirmation defaults, explicit approval, disabled duplicate
actions, recovery/discard controls, processing choices, and compact themes.
These are separate scopes: malformed protocol tests may use a handcrafted peer;
the new independent-process tests run both production peer actors.

## Validation results

Milestone 3.0 commits `c79fabe` and `691ff16` were reviewed for installer safety,
stable identity, permissions, payload allowlists and data preservation. The
literal Windows archive-path fix in `691ff16` preserves the full package audit.
[Push CI for 691ff16](https://github.com/InvarGervi123/InvarDrop/actions/runs/43703740775)
and [PR #14 CI](https://github.com/InvarGervi123/InvarDrop/actions/runs/43705953080)
passed on both Ubuntu 24.04 and Windows Server 2022 before merge `5d3b7cf`.

Local Milestone 3.1 results on CachyOS:

- Rust: **147 passed**, zero failed/ignored, including the **11** new independent
  process tests. The test binary and doctest targets contain zero additional tests.
- UI: **77 passed** across five files; installer safety guards: **4 passed**.
- Rustfmt, strict Clippy (`--all-targets -- -D warnings`), TypeScript and
  production frontend/native builds passed. AppImage and `.deb` payload audits
  passed; AppStream reported only its existing missing-release-entry pedantic hint.
- The opt-in production mDNS scenario passed on this host using the discovered
  network endpoints and actual TLS; ordinary CI intentionally uses loopback.
- One native AppImage GUI workflow passed using two dedicated profiles and actual
  discovery/pairing/TLS: decline without saving, explicit receive approval,
  separately approved ZIP extraction, malicious traversal rejection with original
  archive preservation, trust-revocation confirmation and receipt preservation,
  and 360x420 / 720x520 windows. The app exited with code zero. Only the two
  test-created keyring credentials were removed; the real profile was not used.

No production defect was found in these runs. The initial sandboxed socket test
was stopped because the execution sandbox denied opening a TCP listener; the
network-capable run passed. This is an execution constraint, not an application
TLS or trust bypass. No backend protocol, processing policy, or UI was redesigned.

Native matrix results for `bfdefc4`:
[successful Linux and Windows CI](https://github.com/InvarGervi123/InvarDrop/actions/runs/43707565251).

| Runner | Rust | UI | Installer guards | Native production packages |
| --- | --- | --- | --- | --- |
| Ubuntu 24.04 | **147 passed** | **77 passed** | **4 passed** | AppImage / `.deb` built and audited; actual dpkg install/remove preserved profile sentinels |
| Windows Server 2022 | **140 passed** | **77 passed** | **4 passed**, plus **1** PowerShell literal-path regression | NSIS / MSI built and inspected; Microsoft WebView2 signature validated |

Both jobs passed TypeScript, Rustfmt and strict Clippy, and uploaded testing
artifacts without publishing a release. All **11** independent-process scenarios
executed successfully on each native OS, including actual Windows child `.exe`
processes, loopback TLS, cancellation and restart recovery. No Rust test failed
or was ignored. Seven existing Unix-only tests account for the count difference:
three symlink/source/staging cases, private profile permissions, injected Linux
ENOSPC, and two >4 GiB sparse streaming tests (one over TLS). Windows CI does not
claim those Unix-only cases or >4 GiB streaming execution.

The follow-up commit strengthens the already passing round-trip case to send
back the receiver-owned receipt and assert Unicode destination names explicitly;
its local full suite again passed **147** tests. Consult the
[branch checks](https://github.com/InvarGervi123/InvarDrop/actions?query=branch%3Afeature%2Fcross-device-testing)
for the latest commit's native matrix status. Recorded run IDs identify the
tested revision rather than implying that future branch commits are validated.

Windows GUI, installer runtime, Credential Manager, physical Windows devices and
physical Linux-to-Windows transfer are not claimed. Installer formats and runtime
limitations remain documented in [packaging.md](packaging.md).

## Future physical Linux-to-Windows testing

1. Use two physical computers on the same trusted LAN, with explicit permission
   for local discovery/transfer traffic under each platform's normal firewall
   policy. Do not disable firewalls, Defender, SmartScreen, or the WebKit sandbox.
2. Verify artifact commit and SHA-256, then use the appropriate Linux package and
   Windows installer. Unsigned installer warnings require a future signing and
   distribution decision; never bypass platform security to finish this checklist.
3. Use dedicated test profiles (`INVARDROP_PROFILE` with distinct absolute paths)
   and test data. Do not reuse, copy, delete, or export real device identities.
   OS keyring credentials for these profiles require separate explicit cleanup;
   deleting a profile directory alone does not delete its keyring credential.
4. Confirm mutual discovery, compare **every** pairing code group out of band,
   approve on both machines, restart, and verify pinned reconnection without
   silently trusting changed identities. Reject repeated requests and revoke
   trust; verify no recovery occurs for the revoked identity.
5. Transfer both ways: small/large files, >4 GiB files, multiple selections,
   Unicode names, nested and empty folders. Approve/decline/let requests expire.
   Compare streaming BLAKE3 digests and file sizes on both machines. Verify
   collisions preserve existing destination data.
6. Cancel on each side; interrupt connectivity through a controlled network
   change; close/terminate and restart each app. Confirm verified progress,
   explicit Resume/Discard, stable identity, clean incomplete data, and exact
   final bytes. Check temporary-device unavailability and repeated reconnects.
7. Exercise OGG Keep original and explicit conversion with/without a selected
   compatible FFmpeg. Inspect actual offered MP3 metadata and output decoding.
   Transfer valid/malicious ZIP fixtures, separately approve extraction, and
   confirm failure retains the original and never executes extracted content.
8. Record OS versions, filesystems, interface/VPN configuration, firewall rules,
   package/commit hashes, exact steps, observed failures, and digests without
   recording private keys or pairing codes. Test upgrade/uninstall data retention
   separately on disposable profiles.

## Limits and remaining risks

One physical CachyOS machine is available. Independent processes on one host and
native Linux/Windows CI do not establish full cross-platform compatibility.
Physical Linux-to-Windows transfers, Windows GUI/installer execution, Windows
firewall prompts, separate-machine discovery, Wi-Fi isolation, VPN/routing,
suspend/resume, slow/lossy WAN-like links, power loss, and device/process-level
disk-full/permission fault injection still require additional testing. The
existing Linux ENOSPC regression is a storage-unit fault injection, not a full
device filesystem-exhaustion test. Directory metadata synchronization is
Unix-specific; Windows uses the existing user-profile ACLs and filesystem
publication guarantees described in [resumable-transfers.md](resumable-transfers.md).
Forced process death and TCP
closure reproduce important interruption paths without emulating packet loss,
link shaping, or all real network timing conditions. No QR pairing, mobile
browser transfer, Android support, resume protocol redesign, or dependency was
introduced. This work is a reliability increment, not an independent security
audit or a release certification.
