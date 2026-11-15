# Milestone 2.6 — Large Files and Folders

Current Milestone 2.7 supplement: [resumable transfers](resumable-transfers.md).

## Transfer and consent

An operation transfers one regular file, several selected files, or an entire
folder, including nested and empty directories and zero-byte files. Native dialogs
run in Rust: the frontend supplies only a connection ID, never a source or
destination path. A single selected file uses the existing single-file protocol
flow; multiple files use the root name `InvarDrop files`.

The receiver sees the folder name, the sender's self-reported device name, the file
count (excluding directories), and the total size. Acceptance requires checking the
confirmation for this specific operation and clicking Accept. Decline and the
90-second timeout create no staging area and save no data. A new offer does not
inherit prior consent. Pairing, pinning, trust revocation, TLS 1.3, and the OS keyring
are preserved.

## Protocol and resource limits

The new ALPN is `invardrop/5`, with hello version 5. TLS negotiates the preferred
version 5 or version 4 for Milestone 2.5 compatibility. Version 4 supports only
single files up to 4 GiB; folders and larger files are rejected before an offer is
sent. Earlier versions are unsupported. Identity/trust storage is unchanged.

The manifest contains a safe root name, a sorted list of relative paths with
`file`/`directory` types and `u64` sizes, and a `u64` total size. Declared sizes are
summed with overflow checks; directory sizes must be zero, and the sum must match
the declared total. The receiver calculates the file count.

Limits per operation:

| Resource | Limit |
| --- | --- |
| Single file / total file size | 1 TiB (2^40 bytes) |
| Entries, including directories | 4096 |
| Relative path depth | 16 components |
| Name component | 200 UTF-8 bytes |
| Relative path | 1024 UTF-8 bytes |
| Manifest JSON | 240 KiB |
| BundleOffer JSON including its wrapper | 240 KiB + 512 bytes |
| Other JSON frames | Existing 2048 bytes |
| Chunk payload | Existing 64 KiB |

The entry array is bounded directly in the serde visitor; frame length is checked
before allocation. Extra fields and unknown types are rejected. Existing limits
remain: 8 sessions, a 4-frame reader queue, 30 offers/minute, and 128 unique IDs per
session. The manifest bounds memory use, traversal depth, and the number of created
objects. Disk space is not reserved in advance; insufficient space fails the
transfer and cleans up staging. These limits are not a disk quota across sessions.

`BundleOffer` uses the existing `FileAccept` response. Only after Accept does the
sender send binary `FileChunk` frames for each file, a `BundleFileEnd` with its entry
index and BLAKE3 hash, and finally `BundleEnd` for the operation. Indices must follow
the manifest's file order exactly. Skipped/repeated indices, extra/missing bytes,
and invalid hashes end the session without publishing the folder. Directories need
no separate frames. `BundleFileEnd` does not consume the keepalive quota: its count
is bounded by the manifest and strict sequencing. `FileComplete` acknowledges the
entire operation.

The source reads one file at a time with one file handle and an incremental BLAKE3
hasher; the receiver also holds only the current file. Files are never assembled in
memory. Total progress counts actual payload bytes across all files, never resets
between files, and excludes JSON, TLS overhead, and fsync time. The existing timer
limits UI updates to 4 per second. An empty operation remains at 0% until completion,
then reaches 100%; ETA is hidden when no reliable estimate is available.

## Paths, staging, and publication

Every component follows the shared portable filename rules. Absolute paths,
`.`/`..`, empty components, backslashes, Windows ADS/device aliases, control and
misleading bidi characters, and unsafe endings are rejected. The sorted manifest
must include every parent directory with exactly matching spelling. Duplicate names
and collisions after Unicode lowercase conversion are rejected. Additional
filesystem-specific collisions fail safely through `create_new`.

Source traversal rejects symlinks and special objects; Windows reparse points,
including junctions, are also forbidden. Each file's ancestors and size are checked
again before opening it. A single-file source uses O_NOFOLLOW on Linux and
FILE_FLAG_OPEN_REPARSE_POINT on Windows. Sources are opened read-only. This is not
an atomic tree snapshot: additions/deletions after traversal, same-length content
changes, and races with a process under the same local account have no snapshot
guarantees. The trusted local account remains a threat-model boundary. Ordinary
size changes or replacement with a link cause failure.

After consent, a private `.incoming-*` directory is created under the profile's
fixed `received` directory. All files and directories remain inside `payload`;
files are created exclusively with create_new and Unix mode 0600.
Inbox/staging/receipt directories use Unix mode 0700; Windows uses the user profile's
ACLs. Each file handle closes after verification; all files are fsynced before
publication in a separate finalization task. No individual file is published before
`BundleEnd`.

After all hashes/sizes pass verification, the tree is synchronized and the entire
`payload` is renamed to `received-<unique name>/<folder name>` on the same filesystem.
Linux uses renameat2(RENAME_NOREPLACE); Windows uses MoveFileExW with flags=0.
Neither files nor empty/nonempty destination directories are replaced. A unique
receipt preserves earlier operations with the same name. Unsupported filesystems
or platforms fail without an unsafe fallback; macOS is outside this milestone's
target platforms. Windows/filesystems can impose additional full-path length
limits; such failures clean up the operation.

Files and folders become available to consumers atomically only after all checks.
See [Microsoft](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-movefileexw)
for the MoveFileExW contract. The Linux no-replace requirement is documented in
[rename(2)](https://man7.org/linux/man-pages/man2/rename.2.html).

## Cancellation and errors

Cancel on either side cancels the entire operation and closes the TLS session.
The existing CommitGate atomically chooses between cancellation and publication.
Before publication starts, cancellation prevents rename and Drop removes the entire
staging area, including verified files. Once publication begins, Cancel returns
CommitStarted and the UI shows Saving. A separate tracked task owns finalization
and reports its actual result even after the connection closes. No unverified file
receives Completed status.

A disconnection before publication also cancels commit and cleans up staging. If
the sender cancels after the receiver publishes, the verified folder may remain:
there is no distributed transaction between devices. Cancellation in the middle of
an incomplete network frame may appear as Failed to the other side. Keepalives
continue during finalization, so slow fsync does not block peer responses.
Acknowledgement/finalization waits are limited to five minutes; the ordinary idle
timeout is 20 seconds, and the data-activity timeout is 30 seconds. A stalled system
I/O operation cannot be promised to stop immediately. A directory fsync failure
after successful rename leaves the verified tree but returns an error. A process
crash or power loss may leave `.incoming-*` directories; this milestone does not
implement a startup janitor or guarantee power-loss durability.

## Validation and limitations

Regression tests cover real TLS folder/multi-file operations, more than 64 files,
rejection, timeout, data before consent, invalid manifests/order/hashes, early
BundleEnd, disconnections, cancellation by either side, and deterministic
publication races. Total progress is checked at multiple file boundaries for
monotonicity and exact byte counts. A separate regression holds commit and checks
keepalive responses before publication for a single file and a folder. Tests also
cover the 4096-entry, depth, and metadata limits, large u64 sizes, duplicate names,
path traversal, symlinks, cleanup, and no-replace rename.

On Linux, a sparse file of 4 GiB + 17 bytes is read completely in bounded chunks.
A separate test transfers it over real TLS; the receiver verifies BLAKE3 and exact
u64 counters while streaming, without writing 4 GiB to disk. Smaller fixtures test
actual tree writes and publication. The large sparse test is Unix-only; Windows CI
checks u64 manifests, ordinary TLS/filesystem operations, and no-replace publication.

Outside this milestone: resume, compression, FFmpeg, ACL/mtime/Unix executable-bit
preservation, hardlink identity, filesystem snapshots, source-folder scanning
progress, destination selection, and global disk quotas. Two physical devices,
Windows GUI/native dialogs/Credential Manager, and unusual network filesystems need
separate runtime tests. CI does not replace those checks.

Local validation on 2026-11-14 (CachyOS/Niri): 87 Rust tests and 28 UI tests,
Rustfmt, strict Clippy, both TypeScript checks, Vite, and a locked Tauri release build
passed. The native production GUI at 360×420 used real Secret Service, mDNS, and TLS
with a separate Rust probe sender. Tests covered pairing, the folder name,
2 files/1 MiB, Accept disabled until the checkbox was checked, Decline without
saving, and Accept with BLAKE3/byte equality plus nested/empty directories and a
zero-byte file. A subsequent 256 MiB operation verified real partial progress,
receiver GUI Cancel, and no new receipts/staging. Processes exited normally.
The probe supplies its source directly through the Rust API; the GTK chooser was
not automated. Manual native file/folder selection and Windows GUI need separate
verification. Hosted CI results apply to the specific commit on
`feature/large-files-folders`.
