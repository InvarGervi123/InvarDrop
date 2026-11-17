# Android secure storage

## Diagnosed failure

The original startup path was `Service::start` -> `Store::open` ->
`keyring::v1::Entry::new`. In keyring 4.2.0, the v1 platform initializer explicitly
rejects Android and returns an invalid-platform result; `Entry::new` consequently
returns `NoDefaultStore`. InvarDrop maps that to its public fail-closed storage
error. This is not an Android permission request or an SDK/NDK build problem.
On the connected Galaxy S25, the pre-fix app had `security-v1/profile.lock` and
no `identity.enc`, consistent with failure before identity creation.

## Platform implementation

Linux Secret Service and Windows Credential Manager keep the original keyring v1
backend, service/account identifiers, profile locking and encryption formats.
Android uses the platform `AndroidKeyStore` provider through JNI, initialized by
Tauri/Tao's application context before secure-service startup. No generated Kotlin
project edits, JavaScript key API or new Tauri filesystem permission are required.

For each canonical profile path, the existing SHA-256 account identifier also
binds a Keystore alias. The non-exportable wrapping key is AES-256, restricted to
GCM encryption/decryption with randomized encryption. There is no alternate
provider or software key-file fallback. A random 32-byte master key protects the
unchanged ChaCha20-Poly1305 identity/trust record and derived resume key.

Only the master key's AES-GCM envelope is stored as `master.android.enc` in the
private profile. Its fixed 68-byte format is an 8-byte version header, 12-byte
provider-generated IV, and 32-byte ciphertext plus 128-bit tag. AAD binds the
service, profile account and envelope version. Plaintext master keys and wrapping
keys are never written to disk. In-memory master buffers are zeroized; JNI master
byte arrays are overwritten after use where available.

Publication uses an owner-private temporary file, data sync, no-overwrite atomic
publication and Unix directory sync. Existing files, directories and symlinks
are never replaced. Reads are bounded and Android opens use `O_NOFOLLOW`.
Errors log fixed operation names only, never keys, ciphertext, pairing codes or
unfiltered Java exceptions. No identity is advertised as ready until the existing
encrypted record has loaded or its initial write has succeeded.

## Failure and recovery policy

Only a completely fresh state (no identity, envelope or Keystore alias) may create
new key material. An existing identity requires both its envelope and Keystore
key. Missing, invalidated, malformed, swapped or corrupted material fails closed;
the application never deletes a key, retries with another provider, regenerates
an identity, or silently clears trust/recovery state.

An interruption during first-ever bootstrap may leave an incomplete state that
requires explicit recovery instead of automatic identity replacement. Preserve
the profile and diagnose the failure; do not uninstall, clear application data,
delete a Keystore alias or remove encrypted files as a troubleshooting shortcut.
There is no implemented identity export/recovery or cross-device profile migration.

Android Keystore keys are scoped to the application/user and are not included in
ordinary file backups. Restoring encrypted profile files without their Keystore
key therefore fails closed. Existing Android OS backup settings are unchanged;
restorable device identity is not claimed. Hardware backing/StrongBox is not
required or attested by this implementation; availability follows the device's
Android Keystore provider. No biometric prompt is required for each transfer.
The OS, app sandbox and process integrity remain part of the threat model.

## Focused verification

Host tests exercise all bootstrap-state combinations, authenticated-envelope
round trips, preserved identity/trust/resume keys, corrupt/oversized payloads,
profile-AAD mismatch, missing keys/files, orphaned bootstrap, symlinks/directories,
backend unavailability and publication collisions. These tests use a test AES-GCM
key, not an Android device or a simulated claim of Keystore hardware support.
Existing encrypted-store tests also cover corruption, duplicate profile locks,
trust revocation and resume-epoch behavior.

Build a physical-device verification APK without clearing Cargo artifacts:

```sh
npx tauri android build --debug --target aarch64
```

Output: `src-tauri/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk`.
After owner approval, update with `adb install -r` using the same signing key;
never uninstall to work around a signature mismatch. Confirm the storage error
disappears, record the public device fingerprint, restart the app and verify it
remains unchanged. Check only safe log markers and encrypted file presence/size;
never dump master keys or pairing codes. Physical runtime results are reported
separately from host tests and APK build success.

Android file selection now uses SAF and verified transfers can be saved through
MediaStore; see [Android file transfer](android-file-transfer.md) and
[Downloads saving](downloads.md). Android folder selection and external FFmpeg
selection remain unavailable. See [Android stabilization](android-stabilization.md)
for current permission requirements, verification evidence and remaining limits.

References: [Android Keystore](https://developer.android.com/privacy-and-security/keystore),
[AES-GCM key specification](https://developer.android.com/reference/android/security/keystore/KeyGenParameterSpec),
[keyring v1 platform selection](https://docs.rs/keyring/4.2.0/src/keyring/v1.rs.html).
