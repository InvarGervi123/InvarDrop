# Android file transfer

## Receive failure and duplicate messages

The resumable receiver wrote a checkpoint and immediately read it back through
`regular_file`, which rejected symlinks in every ancestor. Android may expose its
OS-owned application directory through `/data/user/0` pointing to `/data/data`.
That system alias was treated like a hostile payload symlink, producing `FileIo`
before `ResumeAccept` and before any data bytes arrived. The focused regression
reproduces that exact code path using an isolated profile alias.

The Android catalog now resolves the trusted profile prefix once. Descendants
(checkpoints, payloads and source files) still reject symlinks; Android opens
also use `O_NOFOLLOW`. This does not move the profile, change the Keystore alias,
regenerate identity or remove existing trust/checkpoints.

A second Android incompatibility affected final publication: the no-replace
rename implementation was gated to Linux and Windows. Android now uses the
Linux `renameat2` syscall with `RENAME_NOREPLACE`. The syscall avoids requiring
the Bionic API-30 wrapper on the application's API-24 minimum. Unsupported
kernels fail closed; there is no overwrite or non-atomic publication fallback.

Connection shutdown propagated the same failure to both `Connection.error` and
`TransferView.error`, and both cards rendered it. The connection card now omits
an error already rendered by the transfer card, while retaining distinct errors.

ADB had no connected device during the initial implementation. The system-alias
regression and code review establish the incompatibility; the original failing
syscall/path was not captured on the Galaxy. The user subsequently confirmed
successful transfers in both directions and Downloads saving; see the
[current verification record](android-stabilization.md).

## Sender selection through SAF

Android uses `ACTION_OPEN_DOCUMENT`, `CATEGORY_OPENABLE`, a read-only URI grant
and `ContentResolver.openInputStream`. It never treats `content://` as a disk
path, requests broad storage permissions, or modifies original documents.
`EXTRA_LOCAL_ONLY` asks the system picker for local documents. Provider behavior
is controlled by Android; InvarDrop itself contacts no cloud service.

The native Tauri plugin copies selected documents on a worker thread, in 64 KiB
chunks, into a new private `security-v1/prepared/documents-*` directory. Creation
uses `O_EXCL | O_NOFOLLOW` and mode 0600. Provider names cannot select paths; Rust
also applies the existing protocol naming policy and validates actual sizes and
regular files before transport. The native destination is restricted to this
application's canonical private prepared directory, never an arbitrary path.

Limits: 32 files, 4 GiB actual bytes per selection, eight prepared batches shared
with existing preparation, and a 64 MiB free-space reserve while copying. Duplicate
names, unsafe names, unsupported URIs, provider read failures and size overruns
reject the entire import. Picker cancellation sends nothing. Incomplete imports
are owned by a Rust TempDir and removed on failure; after a process crash,
unreferenced batches are cleaned on the next file selection.

A complete, synced private snapshot is handed to the existing TLS/BLAKE3 transfer
implementation. Receiver approval remains mandatory. When resume is supported,
its encrypted catalog references that snapshot across restarts. Cleanup retains
referenced copies and removes them after completion or recovery-record discard.
Transfer cancellation retains its existing semantics. URI permissions do not
need to survive restart because resume reads the private snapshot, not a possibly
changed provider document. Files are sent in their original format on Android;
this adapter does not introduce Android audio conversion.

## Verification and limitations

Focused checks cover the profile-alias failure and successful publication,
payload symlink rejection, no-replace collisions, restart/verified-prefix resume,
SAF name/count/type/actual-size validation, and a real-TLS 1.2 MiB prepared-file
transfer interrupted before approval, restarted, explicitly approved, verified
byte-for-byte and cleaned after delivery. UI regression checks a single alert
for a propagated transfer error and separate alerts for distinct failures.

Run pure JVM copy regressions without an emulator or additional dependencies:

```sh
python3 scripts/test-android-import.py
```

They use the Kotlin compiler already supplied by the initialized Gradle wrapper:
binary integrity and bounded chunks, empty documents, unknown-size overrun,
partial reads, provider errors, low storage, zero-read stalls and unsafe names.
`FilePickerPlugin.kt` and `DocumentImport.kt` are tracked here under
`src-tauri/android`; `build.rs` copies them into the ignored generated Android
project during Android compilation. The standalone test source is not packaged.

Linux and Windows retain their desktop picker and transport implementations.
Android folder selection, custom profile-directory overrides for SAF, and
conversion are not provided by this adapter. Completed verified transfers are
now also saved through MediaStore; see [Downloads saving](downloads.md) for API
requirements, recovery and runtime limitations. Imports need a full local
copy and have no separate import-progress/cancel control; cancel the picker to
stop before importing. A stalled provider read may require closing the app;
unverified imports are never offered as completed transfers.

The [Milestone 15 follow-up checklist](android-stabilization.md#follow-up-validation-before-broader-compatibility-claims)
tracks remaining physical restart, interruption, permission and background
checks. Preserve app data and the signing key when updating; never uninstall,
clear data or reset trust. Confirmed basic transfers need not be repeated for
session handoff. Host/JVM checks do not prove untested Android provider behavior.
