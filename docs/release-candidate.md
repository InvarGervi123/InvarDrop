# InvarDrop 0.1.1-beta.2 release candidate

The existing v0.1.0-beta.1 release covered desktop LAN sharing. This candidate adds the redesigned sharing flow and prepares the first Android sideload APK. It uses Android version code 1001 and Windows MSI ProductVersion 0.1.1. Windows Installer compares the first three fields, so the patch release advances the upgrade version. The application ID and MSI upgrade code stay the same. [Historical prerelease](beta-release.md) · [Microsoft version rules](https://learn.microsoft.com/en-us/windows/win32/msi/productversion)

## Install verified release artifacts

Use the artifacts for the exact release commit and compare the SHA-256 for each file with its published manifest entry. On Linux, run `sha256sum <downloaded-file>`. On Windows, run `Get-FileHash -LiteralPath '<downloaded-file>' -Algorithm SHA256` in PowerShell. Validate the Android signature and certificate fingerprint against the values published with the release.

| Platform | Artifact | Installation |
| --- | --- | --- |
| Linux x86-64 | `InvarDrop_0.1.1-beta.2_amd64.deb` | On a compatible Debian or Ubuntu system, run `sudo apt install ./InvarDrop_0.1.1-beta.2_amd64.deb`. Launch from the desktop menu. Remove with `sudo apt remove invar-drop`. |
| Linux x86-64 | `InvarDrop_0.1.1-beta.2_amd64.AppImage` | Run `chmod u+x InvarDrop_0.1.1-beta.2_amd64.AppImage`, then launch the file. If FUSE is unavailable, pass `--appimage-extract-and-run`. |
| Windows x86-64 | NSIS setup `.exe` or WiX `.msi` | Choose one format. NSIS installs for the current user. MSI installs per machine and may require administrator approval. Both include the offline WebView2 installer. Remove through Installed apps. Keep SmartScreen, Defender, and organizational policy enabled. |
| Android ARM64 | `InvarDrop_0.1.1-beta.2_arm64.apk` | Use the APK signed by the release owner. Open it from a trusted local file source or use `adb install -r InvarDrop_0.1.1-beta.2_arm64.apk`. Android accepts an in-place update when the package and signing identity match. |

The Linux distribution baseline is Ubuntu 24.04 with glibc 2.39. The desktop requires a graphical session, WebKitGTK 4.1, session D-Bus, an unlocked Secret Service, and a file portal. Packages built on a newer rolling distribution are useful for local validation. Use the verified Ubuntu CI artifacts for distribution. Windows uses Credential Manager. Desktop testing installers are unsigned. [Desktop packaging](packaging.md)

Existing Android debug installations use a different certificate from the planned public APK. If Android reports a signing mismatch, keep the existing installation and its identity, trust and recovery data. Test the public signer on a separate device or disposable emulator. Public updates must keep the release signing identity. [Android signing procedures](https://developer.android.com/studio/publish/app-signing)

## Use InvarDrop

1. Open InvarDrop on both devices. Choose files and select a nearby device in either order, then press **Send**.
2. When prompted to pair, compare every code group on both screens and approve only when they match. A discovery name is self-reported.
3. The receiver checks the sender, file name and size, confirms the request, and accepts. **Completed** means the transfer passed integrity verification and reached its final state. Received files appear under **Downloads → InvarDrop**.
4. For an interrupted transfer, open **Settings → Transfer recovery**. Reconnect the trusted device, then request resume. The receiver approves again before any payload resumes.

If a device is missing, keep both apps visible and check local network access. **Find more devices** can request the Android Bluetooth grant. If local network access was denied on Android 17, grant it in system settings and reopen InvarDrop.

## Build, sign and verify

The GitHub CI workflow builds Linux and Windows installers on native runners and an unsigned ARM64 Android APK on Ubuntu. CI keeps the Android build hash and source commit. The APK stays on the runner, and the signing key stays with the release owner. Check all three jobs for the release commit, then build the unsigned APK from that source on the signing machine. The project uses manual release publication.

Use Android SDK platform 37, build-tools 36.0.0, NDK 27.2.12479018, JDK 17, Python 3.11 or newer, and the Rust target `aarch64-linux-android`. For a fresh checkout, initialize Android once. The generated Gradle project is ignored by Git.

```sh
rustup target add aarch64-linux-android
npm ci
npm run tauri -- android init --ci --skip-targets-install
npm run tauri -- android build --ci --target aarch64 --apk -- --locked
python3 scripts/test-android-import.py
```

The unsigned build is normally at `src-tauri/gen/android/app/build/outputs/apk/universal/release/app-universal-release-unsigned.apk`. Verify it before it enters the signing process. [Android versioning in Tauri](https://v2.tauri.app/distribute/google-play/)

```sh
android_tools="$ANDROID_HOME/build-tools/36.0.0"
unsigned_apk=src-tauri/gen/android/app/build/outputs/apk/universal/release/app-universal-release-unsigned.apk
python3 scripts/android-release.py --build-tools "$android_tools" check-unsigned "$unsigned_apk"
```

The release owner maintains the dedicated sideload signing key and a secure backup outside the repository. The first public APK establishes the certificate identity required by future sideload updates. If there is no release key yet, the release owner can create a password-protected PKCS#12 keystore with `keytool -genkeypair` using a file outside the repository. Export the public certificate with `keytool -exportcert` and record its SHA-256 from the DER certificate in the release records. [Android APK signing and verification](https://developer.android.com/tools/apksigner)

On the signing machine, export `INVARDROP_ANDROID_KEYSTORE` to the absolute path to the owned keystore and `INVARDROP_ANDROID_KEY_ALIAS` to its alias. Set `release_cert_sha256` to the independently recorded public certificate fingerprint. Enter passwords without terminal echo with shell tracing disabled.

```bash
set +x
read -rsp 'Keystore password: ' INVARDROP_ANDROID_STORE_PASSWORD
printf '\n'
read -rsp 'Key password: ' INVARDROP_ANDROID_KEY_PASSWORD
printf '\n'
export INVARDROP_ANDROID_STORE_PASSWORD INVARDROP_ANDROID_KEY_PASSWORD
mkdir -p src-tauri/target/android-release
signed_apk=src-tauri/target/android-release/InvarDrop_0.1.1-beta.2_arm64.apk
python3 scripts/android-release.py --build-tools "$android_tools" sign \
  "$unsigned_apk" "$signed_apk" --certificate-sha256 "$release_cert_sha256"
unset INVARDROP_ANDROID_STORE_PASSWORD INVARDROP_ANDROID_KEY_PASSWORD
python3 scripts/android-release.py --build-tools "$android_tools" verify \
  "$signed_apk" --certificate-sha256 "$release_cert_sha256"
```

The signing command aligns ZIP data before signing, checks ABI, ELF segments, and manifest policy, verifies the expected certificate, and prints the final APK SHA-256. Keep the keystore and credentials out of release files and CI logs. Record the exact Git commit, successful Actions run, artifact hashes, and Android certificate fingerprint. Regenerate the public checksum manifest from the final signed APK and exact installer files. [Android ZIP alignment](https://developer.android.com/tools/zipalign)

## Local validation and publication gates

On CachyOS, 88 frontend tests, 190 Rust tests, four installer-template tests,
and the Android JVM helper suites passed. The Rust run included a bidirectional
1 GB loopback transfer. It also exercised LAN preference, rejection of a wrong
saved peer key, and fallback with an in-memory Bluetooth route. The tests cover
TLS 1.3, pairing, receiver approval, BLAKE3, and transfer recovery. Rustfmt,
strict Clippy, TypeScript, and the frontend build passed. The npm advisory
registry returned no advisories for the checked dependency lock.

The ARM64 release APK passed package, permission, backup policy, and 16 KiB APK
and ELF alignment checks. A disposable key verified the signing flow and
wrong-certificate rejection. That key is for local verification only. The Linux
Debian and AppImage payload audits passed on the build host.

Publication still requires the release owner's Android signature, native CI
artifacts for the final commit, and the physical acceptance results below.
Windows packages and runtime tests require a Windows environment.

## Physical acceptance tests

Record the APK and installer SHA-256 values, OS and app versions, connection transport from **Settings → Connection diagnostics**, and pass or fail for each test. Use disposable files and profiles for denial, revocation, and installer lifecycle tests. Keep pairing codes and private data out of reports.

1. **Android install and permissions.** On a fresh install, deny Nearby devices and verify LAN sending and receiving still work. On Android 17, also test denying local network access, enabling it in system settings, and reopening the app. Confirm local discovery returns and that a Bluetooth grant is requested only when asked. Test the release build on a 16 KiB Android device or emulator, confirmed with `adb shell getconf PAGE_SIZE`.
2. **Bidirectional LAN, pairing, and approval.** With Linux and Android on the same LAN and Bluetooth enabled, select a file before the device on one side and reverse the order on the other. Confirm new connections report LAN. Compare the full code, reject one file and confirm no public copy, then accept one file in each direction. Compare BLAKE3 hashes before and after. Reopen the apps and send again to confirm saved trust. Revoke the peer on a disposable profile and verify a send cannot reuse that trust.
3. **Bluetooth-only and automatic fallback.** Start with Wi-Fi and USB tethering off on the phone. Verify discovery in both directions, expected OS pairing prompts, and a verified file in each direction, Android to Linux and then Linux to Android. Disconnect the existing session in Diagnostics before changing directions. For automatic fallback, start with both routes visible, disconnect the previous session, and interrupt LAN reachability while the peer remains visible. Press Send once and confirm the new connection reports Bluetooth RFCOMM. If the LAN entry disappears before Send, record the run as Bluetooth-only and retry the fallback case.
4. **Recovery and terminal state.** Send a file larger than 32 MiB. Interrupt the link after a checkpoint has been verified, reconnect, and confirm that payload stays paused until the sender selects Resume and the receiver accepts. Compare the final BLAKE3 hash. Close the connection and confirm Activity still shows Completed. Repeat with Cancel and Discard on disposable data.
5. **Android lifecycle.** Repeat discovery after moving the app to the background, locking the screen, returning to it, and turning Bluetooth off and on. Repeat while an approved transfer is active and check completion or recoverability after an interruption. Verify a canceled file picker keeps the prior selection. Measure idle power separately before claiming any savings.
6. **Desktop installation and Windows LAN.** On Ubuntu 24.04, install the exact CI Debian build, test the portal and Secret Service, then upgrade or remove it and verify profile and received-file retention. Launch the matching AppImage. On a supported Windows 10 or 11 x86-64 machine, exercise NSIS and MSI in separate clean environments. Upgrade each from beta.1, verify WebView2 setup, run a Linux–Windows LAN transfer in both directions, restart to verify Credential Manager trust, and uninstall. Confirm installer changes preserve identity and received files.

## Known limitations

- Linux and Android Bluetooth support is experimental. Android-to-Android Bluetooth routing, Windows Bluetooth, and background Android discovery or transfer guarantees are outside this candidate. Android can share files through the system picker. Desktop offers folder selection. Android keeps private staging and Keystore-bound state out of its app backup policy. [Android backup policy](https://developer.android.com/identity/data/autobackup)
- LAN discovery depends on multicast support and allowed local network access. Network isolation, VPN rules, and firewalls can block discovery or connection.
- Recovery uses authenticated checkpoints every 16 MiB and at file boundaries. A slow link may retransmit data since the last checkpoint. Retrying requires the original source and fresh consent.
- Local unit, JVM, and process tests do not establish Bluetooth radio behavior, Android runtime compatibility across OS versions, or Windows installer behavior on a user machine. No independent security audit or physical power measurement has been completed.

See [Bluetooth verification details](bluetooth.md), [transfer recovery](resumable-transfers.md), and the [security model](security.md).
