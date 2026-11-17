# Milestone 2.3 — Single-File Transfer

Current Milestone 2.7 supplement: [resumable transfers](resumable-transfers.md).

The Milestone 2.6 protocol, folders, limits, and compatibility with 2.5 are described
in [large-files-folders.md](large-files-folders.md). The historical context of this
milestone is retained below.

This document records 2.3. The 2.4 UI and protocol changes are documented in
[receiver-acceptance.md](receiver-acceptance.md).

## Workflow and boundaries

Two instances establish a verified TLS 1.3 connection using the
[pairing](security.md) rules. Only after `connected` does the sender click
`Choose a file to send` and select one regular file in a native dialog.
The path stays in Rust: IPC accepts only a connection ID, never a source path.
The receiver sees the name and size and separately clicks `Accept file` or
`Decline file`. Trusting a device does not automatically authorize incoming files.

There is one active transfer per connection. After completion/rejection, another
file may be selected, including in the opposite direction. The file limit is
4 GiB. Empty files are allowed. Directories, symlinks, FIFOs, and devices are
rejected as sources. Files are opened read-only; the application never modifies
source data. This milestone has no folders, file queue, resume, FFmpeg, conversion,
or advanced progress display.

## Protocol and resources

The **same** TLS stream and persistent reader used for pairing are reused. There
is no second data listener, plaintext socket, or new unauthenticated connection.
One actor owns the writer, preventing file chunks and keepalives from interleaving.
ALPN is `invardrop/2`, with hello version `2`. Both devices must update; connections
using the 2.2 transport close without fallback. Persisted SPKI, private keys,
encrypted trust state, and the pairing-code algorithm require no migration.

Control frames use a 4-byte big-endian length with the high bit clear, followed by
up to 2048 bytes of JSON. In addition to hello/decision/ready/ping/pong, these are
allowed:

| Message | Data |
| --- | --- |
| `file_offer` | Random UUID `id`, validated basename `name`, size `size` |
| `file_accept` | `id`, boolean `accept` |
| `file_finish` | `id`, BLAKE3 hexadecimal stream digest |
| `file_complete` | `id`, receiver acknowledgement of verified storage |

Binary chunks use a 4-byte big-endian length with the high bit set; the remaining
31 bits specify payload length. The payload is a 16-byte UUID and 1 to 65536 bytes
of data. Bounds are checked **before** buffer allocation. JSON imitations of binary
chunks are forbidden. TCP/TLS preserves order; size and digest verify the entire
stream. There is no resume/offset API.

The reader queue holds 4 frames and UI actions 2; memory usage is independent of
file size. BLAKE3 is computed incrementally on both sides. There is no full-file
pre-read, base64, or file-content array in React. Approval waits are limited to
90 s, transfer activity/acknowledgement to 30 s, and each TLS write to 20 s.
The 2.2 session, handshake, and keepalive limits remain. Binary chunks do not
consume the keepalive/control budget. Incoming offers are limited to 30/min and
128 unique IDs per connection without evicting old IDs: a repeated offer ID closes
the connection and cannot apply old UI approval to another file. Reconnect is
required after the limit.

## Storage and finalization

The application chooses the inbox: `<security profile>/received`, normally inside
app-data. With `INVARDROP_PROFILE`, it is within the selected local test profile.
The UI shows its path. Neither the peer nor IPC can choose a destination path.
After Accept, a separate random `received-*` directory and a tempfile inside it
are created. Transfers with identical basenames use separate directories.

Names are limited to 200 UTF-8 bytes and validated identically on Linux/Windows.
Traversal/absolute paths, `/`, `\`, ADS `:`, forbidden Windows characters, device
aliases (`CON`, `AUX`, `COM1`, etc.), trailing dots/spaces, control characters, and
bidi-control characters are rejected. Sources undergo the same filename checks.
A symlink inbox is rejected. Compromise of the same local account and parent-
directory replacement races by that account are outside the 2.2 threat model.

The receiver accepts exactly the declared size. Data before Accept, wrong IDs,
extra bytes, truncation, wrong hashes, and unexpected phases fail without publishing
a completed file. Success performs flush, `sync_all`, closes the file handle
(including waiting for Tokio I/O), then calls `TempPath::persist_noclobber` in the
same directory. Existing files are never replaced, even on an unexpected local
collision. Unix synchronizes the directory and its parent; directories use mode
0700 and files 0600. Windows inherits the profile's ACLs and closes the handle before
moving the file.

Publication of the final name is atomic where the filesystem and
`persist_noclobber` implementation support it; there is no shared transaction
between devices or crash-durability guarantee for every filesystem. Received files
are ordinary data: identity/trust AEAD does not encrypt the inbox.

## Errors, cancellation, and disconnection

Decline leaves TLS ready for another file. Protocol, size/hash, I/O, timeout,
Disconnect, or Revoke errors close the session; normal completion of the owning
tasks removes unfinished tempfiles. Completed files are never opened or executed
automatically.

Trust is checked again before finalization. Filesystem finalization already started
in a separate owning task may finish after Cancel/Disconnect: the task still
publishes the actual local result and path to the UI. Cancellation must not falsely
suggest that a saved file is absent. The sender considers success only after
`file_complete`; if acknowledgement is lost, its outcome is uncertain even though
the receiver may have a complete verified file. There is no automatic retry or
deletion of the saved file.

Process crashes or uninterruptible OS operations can leave staging artifacts; this
milestone has no automatic cleanup of other/older files. Network loss never starts
resume. A local staging collision preserves the existing file and removes only
our tempfile, so that directory may remain for inspection. Source size changes are
detected; no immutable source snapshot is created. With concurrent changes to the
same bytes, BLAKE3 describes the stream actually read, rather than an earlier file
version.

## Validation

```sh
npm run check
npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo clippy --locked --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --locked --manifest-path src-tauri/Cargo.toml
npm run tauri build -- --no-bundle -- --locked
```

Automated tests use real TLS sockets, streaming files, and BLAKE3. Test-profile
master keys stay in memory, as in the 2.2 regressions. Native pickers, Secret
Service/Credential Manager, and the GUI are verified separately. A useful runtime
scenario is two profiles, pairing/reconnect, empty/large file selection, rejection/
acceptance, byte-for-byte comparison, repeated basenames, Disconnect before
completion, and Revoke while receiving. Never print pairing codes or user file
contents in logs.

Libraries: [incremental BLAKE3](https://docs.rs/blake3/latest/blake3/struct.Hasher.html),
[native rfd picker](https://docs.rs/rfd/0.18.0/rfd/struct.AsyncFileDialog.html),
[no-clobber semantics](https://docs.rs/tempfile/latest/tempfile/struct.TempPath.html#method.persist_noclobber).

## Validation results — 2026-11-14

All 57 local Rust tests passed (none ignored), along with Rustfmt, strict Clippy
`--all-targets -- -D warnings`, both TypeScript checks, frontend production build,
and a locked Tauri production build without bundles.

Regressions transfer an empty file and a file exceeding 70 chunks over one real
TLS session, comparing results and checking unchanged sources. Coverage includes
Decline, repeated basenames, replayed offer IDs, data before Accept, wrong IDs/
hashes, excess size, a short stream with the correct hash of its short contents,
abrupt TLS loss, Cancel/Revoke during reception, unsafe names, write failure, and
no-clobber collisions.

Two Linux production GUI processes with real Secret Service were previously tested
for pairing, Reject/Cancel, reconnect, and opening the system file chooser. The full
GUI file-selection/transfer workflow was not completed because of GTK automation;
the reproducible TLS/filesystem tests above provide alternative coverage. This is
not a claim of a full GUI end-to-end test. Windows GUI/picker/Credential Manager,
two physical devices, and complex network scenarios need separate verification.


Current application builds additionally publish verified receipts to the system
Downloads directory. See [Downloads saving](downloads.md) for the destination,
private staging, recovery and platform-specific limitations. The private receipt
remains the authoritative recovery/processing copy.
