# Milestone 15: Android stabilization and verification

## Status and evidence

The Android milestone (`c565862`) and Windows follow-up (`5b3883e`) are committed
and pushed to `main`. Linux and Windows CI passed on `5b3883e`; see the
[Windows CI follow-up](#windows-ci-follow-up) for exact results and root causes.
No new release was created.

On 2026-11-17, the user confirmed that Android/Linux transfers work in both
directions, including automatic Downloads saving. These manual checks are
user-reported and must not be repeated merely for session handoff. The focused
automated evidence below was collected on 2026-11-16. Advanced Galaxy workflows
remain follow-up validation; background reliability is not certified.

During that automated verification session, ADB initially detected `SM-S931B`,
API 37, a 4096-byte runtime page size, and a running debug InvarDrop WebView.
It disconnected before transfer tests could start. No new physical transfer,
restart, background or permission-dialog success is claimed. No APK was installed, app data cleared,
identity reset, permission auto-granted or trust entry removed.

## Actual automated results

| Check | Result and scope |
| --- | --- |
| 100 MB round trip | 100,000,000 bytes each direction, two independent Linux Service instances, distinct identities/profiles and real TLS. Private and public BLAKE3 digests match. Monotonic actual-byte progress and no premature Downloads publication. About 1.84 / 1.50 seconds, including local verification. |
| 1 GB round trip | 1,000,000,000 bytes each direction through the same production TLS/transfer/Downloads paths. Reverse direction reads the actual public receipt. About 10.84 / 10.76 seconds. These are loopback timings, not Android/Wi-Fi performance measurements. |
| Independent-process reliability | 9 existing tests pass: bidirectional contents, repeated pairing/untrusted/rejected requests, two abrupt file interruptions, nested-folder interruption, sender interruption, corrupted checkpoint/changed source, cancellation on both sides plus Discard, revocation across restart, and concurrent opposite-direction channels. Some tests cover several scenarios. |
| Downloads regressions | 11 pass: approval/rejection, duplicate names, atomic desktop publication, source corruption, collisions/symlinks, empty/nested folders, zero-byte files, copy failures, staging cleanup, restart recovery without the sender source, and cancellation before commit. |
| Encrypted storage | 17 pass: 10 common store tests and 7 host Android-envelope tests. They cover persistence, corruption, locking, trust/revocation and fail-closed bootstrap. The Android tests substitute a test protected key; they do not execute physical Android Keystore. |
| Permission gate | 2 Rust tests pass: pending/denied cannot allow networking; only a platform response grants this process and a fresh process must check again. |
| Native JVM helpers | 8 SAF import tests and 9 Downloads publication tests pass, plus 4 SDK/target permission-policy assertions. They do not run MediaStore or a system dialog. |
| UI | 66 selected tests pass across FileTransfer, PairCard, ResumeTransfers and ErrorNotice. |
| Static/build checks | TypeScript, Rustfmt, strict host Clippy with all targets, Android ARM64 Rust checking, Kotlin compilation and one final debug ARM64 APK build pass. |

The large-file test is explicitly opt-in and ignored during ordinary CI. It uses
one sparse source with nonzero first/last chunks, bounded hash buffers and
sequential directions. Data is real payload over TLS, without simulated progress
or compression. Approximate peak fixture payload is 3 GB for the 1 GB run; all
fixtures are in a temporary directory under ignored `src-tauri/target` and were
removed. Nothing was written into the real user's Downloads or security profile.

```sh
cargo test --manifest-path src-tauri/Cargo.toml large_file_round_trip_with_verified_downloads -- --ignored --nocapture
INVARDROP_LARGE_TEST_BYTES=1000000000 cargo test --manifest-path src-tauri/Cargo.toml large_file_round_trip_with_verified_downloads -- --ignored --nocapture
python3 scripts/test-android-import.py
```

## Compatibility bug fixed: Android 17 local network permission

The generated project targets SDK 37, but previously declared only `INTERNET`
(and the AndroidX internal receiver permission in the merged APK). Android 17
requires apps targeting SDK 37 to declare and request `ACCESS_LOCAL_NETWORK`
for raw TCP/UDP, including incoming connections and mDNS. Earlier basic transfer
success does not establish correct fresh-install or permission-denial behavior.
This is a verified source/configuration mismatch, not a claim that it caused a
new observed Galaxy transfer failure.

The existing native Tauri bridge now requests that permission when both OS and
target SDK are at least 37. Older OS/target combinations request nothing new.
Rust holds mDNS startup and the secure TCP listener behind the platform response.
Denial or bridge failure leaves networking unavailable, explains recovery in the
UI and preserves identity/trust. Denied discovery stops instead of polling in the
background. Granting permission in settings requires restarting the app to check
it again; live permission monitoring is not added. Android still enforces later
revocation independently.

`build.rs` manages only its tagged permission block in the generated manifest,
based on the generated Gradle literal `targetSdk` value. It fails explicitly if
that format cannot be understood. No target SDK downgrade, Android 16 compat
flag, permission auto-grant, Bluetooth/location request or authentication bypass
is introduced. TLS, BLAKE3, peer binding and approval checks are unchanged.

Reference: [Android local network permission](https://developer.android.com/privacy-and-security/local-network-permission).

## Background, storage and platform limits

- No foreground service, wake lock, notification service or battery-optimization
  exemption is added. Continued transfer while backgrounded or screen-locked is
  not guaranteed. Android Doze, App Standby and Samsung battery policy can defer
  networking or kill the process. Keep the app foregrounded for reliable current
  use; recover through the existing explicitly approved resume workflow.
- Process-kill tests establish core checkpoint behavior on Linux. They do not
  prove Samsung process management, Wi-Fi/mDNS behavior or Keystore persistence
  after physical app restart.
- Android SAF selection supports files, not folder picking, with 32 files and
  4 GiB total private import per selection. A full prepared copy consumes space.
  Android can receive folders; physical MediaStore/empty-directory checks remain.
- Public saving uses MediaStore on Android 10+. A folder becomes visible file by
  verified file, not as one atomic MediaStore operation. Pending files stay hidden;
  copy/readback failure never commits a corrupted or incomplete file. Private
  verified copies remain for recovery and ZIP processing.
- Private/public copies need additional space. Existing transfer/count/depth,
  checkpoint-age and finalization time limits remain. See [Downloads saving](downloads.md)
  for cross-volume staging, hard-crash leftovers and slow finalization limits.
- Native Linux and Windows CI now pass, including isolated Downloads and TLS
  regressions. No physical Windows GUI, installer installation/removal, firewall,
  normal Credential Manager usage or physical Linux-to-Windows transfer is
  verified by those tests. See [cross-device testing](cross-device-testing.md)
  for the future physical Windows checklist.

Reference: [Android Doze and App Standby](https://developer.android.com/training/monitoring-device-state/doze-standby).

## Final APK

One debug build was necessary for the Android 17 permission fix. The application
identifier, version, NDK and debug signing certificate are unchanged. ARM64 ELF
LOAD segments and uncompressed APK library offsets pass 16 KiB alignment checks.
The device observed here uses 4 KiB pages; this is not a 16 KiB-kernel runtime test.
The APK declares `INTERNET`, `ACCESS_LOCAL_NETWORK` and the existing internal
receiver permission; no storage, Bluetooth, location or background-service
permission was added. Standalone JVM test runners are not packaged.

Path: `src-tauri/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk`.

SHA-256: `2d7698ebf41e7bd25d2ea112a212601511e5433a94c19f5763f524c098cc8060`.

This ignored local artifact predates the Windows follow-up commit; it was not
rebuilt for that fix and is not a published release or an exact `5b3883e` build.

## Follow-up validation before broader compatibility claims

1. Record the public device fingerprint and trusted peer list. Update with the
   same signing key using `adb install -r`, never uninstall or clear data. If the
   signing certificate differs, stop instead of resetting the profile.
2. On API 37, test grant, denial and settings re-enable plus restart. Denial must
   prevent discovery/listening; accepting the OS permission must not approve a
   pairing or file request. Preserve the fingerprint/trust list across restart.
3. Record whether the existing manual device checks also covered 100 MB and
   1 GB files. The host large-file results above are not Android benchmarks.
   Additional device stress checks are follow-up validation.
4. Interrupt after a verified checkpoint by disconnecting a test connection or
   closing one app. Reopen/reconnect, confirm fresh resume approval and verified
   byte progress, and compare final contents. Repeat cancellation from each side;
   no incomplete Downloads file may appear. Discard should remove only its
   selected recovery state.
5. Send several files, duplicate names, zero-byte files and a Linux-selected
   nested folder with an empty directory. Android folder selection remains
   unsupported. Verify existing receipts are never overwritten.
6. Test brief Home/background and screen lock, then reopen; also test normal
   battery/Doze behavior separately. USB charging and a short successful Home
   test do not prove unattended background reliability. Do not disable OS power
   protections or request a battery exemption to manufacture a passing result.
7. Check insufficient-space/permission errors without filling the real device.
   Verified private data must remain recoverable and pending/public state must
   be truthful. Do not delete profile files or Keystore material to induce errors.
8. Record new device outcomes with the tested APK hash and preserve unresolved
   limitations. Any further source changes need appropriate fresh checks;
   publishing a release requires explicit user authorization.

## Windows CI follow-up

Run [44049872032](https://github.com/InvarGervi123/InvarDrop/actions/runs/44049872032)
passed Linux but exposed three Windows Rust failures. Two Downloads assertions
compared canonical extended-length Windows paths with noncanonical roots.
The tests now compare the exact expected receipt against a canonical root;
production path handling and no-overwrite checks stay unchanged.

The occupied-port failure also exposed missing exclusive Winsock binding.
Listeners now set `SO_EXCLUSIVEADDRUSE` before bind for both dual-stack and
IPv4 fallback sockets. Regressions keep the original occupied-port assertion,
check an occupied IPv4 port against both listener paths, and reject a competing
Windows `SO_REUSEADDR` bind. None of the three failing tests is disabled, and
TLS/pairing are unchanged.

Fix commit `5b3883e` passed
[run 44055447070](https://github.com/InvarGervi123/InvarDrop/actions/runs/44055447070):
Ubuntu 24.04 **177 Rust tests passed, 0 failed, 1 opt-in large-file test ignored**;
Windows Server 2022 **165 passed, 0 failed, 0 ignored**. All three original
failures and the new bind regressions passed. Both jobs also passed UI tests,
TypeScript, Rustfmt, strict Clippy, production packaging and package checks.
These desktop jobs do not execute Android Keystore, SAF or MediaStore.

References: [Rust canonicalize](https://doc.rust-lang.org/std/fs/fn.canonicalize.html),
[Winsock exclusive binding](https://learn.microsoft.com/en-us/windows/win32/winsock/using-so-reuseaddr-and-so-exclusiveaddruse).
