# Android/Linux connectivity and 16 KiB pages

## Diagnosed transport failure

The reported session used TCP port 54383. The Linux kernel journal contained
`[UFW BLOCK]` events for incoming **IPv6 link-local TCP SYN** packets on the USB
interface, destined for that port. Existing UFW permits were IPv4-only. The
failure therefore occurred before TLS or pairing; permitting an IPv4 subnet
does not permit IPv6 traffic. Working mDNS discovery does not establish TCP
reachability. Linux local IPv4/IPv6 probes succeeded; they do not test remote UFW
access. Android's generated manifest already declares `INTERNET`.

Discovery can advertise addresses on multiple interfaces. The application keeps
IPv4, IPv6 scope IDs, its eight-endpoint bound and existing connection budgets.
TLS 1.3, certificate signatures, saved SPKI pins, revocation, protocol negotiation
and explicit two-sided comparison-code approval remain required. No firewall
rule bypasses those checks or approves an incoming file.

## Reproducible firewall configuration

By default, InvarDrop chooses an ephemeral TCP port at each start and advertises
it through mDNS. A rule for a previous port stops applying after a restart.
To use a narrow, stable firewall rule, optionally pin an available port:

```sh
INVARDROP_LISTEN_PORT=54383 npm run tauri dev
```

Absent/zero keeps the original ephemeral behavior. Invalid configuration or an
occupied fixed port fails explicitly; it never silently falls back to another
port. Each independent instance needs a different port or the ephemeral default.

Inspect `ss -ltnp`, `avahi-browse -rtp _invardrop._tcp`, and `sudo ufw status
numbered`. Check the actual advertised destination, incoming interface and source
address family before changing a rule. On the reported USB network, an example
of the missing scoped permission is:

```sh
sudo ufw allow in on enp0s20f0u4 proto tcp from fe80::/64 to any port 54383 comment 'InvarDrop USB IPv6'
```

Review this command with the machine owner. Replace the interface/subnet/port
with the observed values; prefer a single peer address when stable. Keep existing
IPv4 permissions only for intended networks. Wi-Fi traffic arriving on `wlan0`
needs a separately reviewed rule if that path is intended. Do not disable UFW,
permit all ports, remove identities, or bypass Android network/VPN policy.

Useful targeted evidence: `journalctl -k -b --grep 'DPT=54383'`. Debug builds log
attempted socket addresses, I/O error categories, and completion of TCP/TLS/peer
authentication stages. They never log pairing codes or key/certificate bytes.
Public error messages distinguish missing endpoints, refused/timed-out/denied TCP,
TLS failures/timeouts, authentication timeouts and rejected/expired pairing.
Discovery addresses remain untrusted input, not pairing credentials.

## 16 KiB compatibility

The original APK passed ZIP alignment, but its ARM64 ELF LOAD segments had
`p_align = 0x1000`. NDK r27 defaults to 4 KiB ELF alignment. `build.rs` now passes
`-Wl,-z,max-page-size=16384` and `-Wl,-z,common-page-size=16384` only to the Android
cdylib. Desktop linker flags, NDK and dependency versions are unchanged.

Build the final ARM64 debug APK, then verify both the library and packaged bytes:

```sh
npx tauri android build --debug --target aarch64
python3 scripts/check-android-alignment.py \
  src-tauri/target/aarch64-linux-android/debug/libinvardrop_lib.so \
  src-tauri/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk
"$ANDROID_HOME/build-tools/36.0.0/zipalign" -c -P 16 -v 4 \
  src-tauri/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk
```

The script checks every native library's ELF LOAD alignment and offset/address
congruence, plus 16 KiB APK offsets for uncompressed `.so` files. Passing ZIP
alignment alone is insufficient. See [Android's guidance](https://developer.android.com/guide/practices/page-sizes).

## Physical verification boundaries

With owner approval and a connected, authorized device, use `adb install -r`
with the same signing key. Never uninstall or clear data to resolve a signature
mismatch. Compare the public device fingerprint before and after. Connect using
the intended LAN path, inspect stage diagnostics, compare every pairing-code
group, and approve on both devices only when they match. Report native-device
results separately from loopback tests and build/alignment verification.

Do not infer that a firewall change was applied from a command example or that
pairing succeeded from successful compilation. Hardware/runtime status and any
remaining administrative steps are recorded in the delivery report.
