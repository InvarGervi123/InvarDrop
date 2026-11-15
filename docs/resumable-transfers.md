# Milestone 2.7 — Resumable Transfers

## What persists and how to resume

A new transfer between resume-capable instances receives a random UUIDv4. It can
contain one file, several selected files, or a nested folder. After a network
interruption, application shutdown, or temporary device unavailability, the original
manifest, trusted peer-key binding, and verified portion of the data persist.
A successful reconnection alone **does not start a transfer**.

`Transfer recovery` lists individual operations: name, direction, peer key, total
size, verified and remaining bytes, and percentage. Resume is disabled while the
peer is offline; Discard remains available. Connection matching checks the actual
trusted TLS key and negotiated resume support, rather than the device's mDNS name.

To resume:

1. Connect the trusted devices. Resume becomes available for the matching peer
   when its connection is not occupied by another operation.
2. The sender checks confirmation for the specific record and clicks Resume.
   If the receiver clicks Resume first, the sender receives a request: this is
   only a notification, and the sender must still explicitly approve Resume locally.
3. The receiver approves the offer again through the checkbox and Accept. Until
   then, saved data stays isolated and payload transmission does not continue.

Decline and offer expiration preserve the existing recovery record. A new offer
with the same persistent UUID requires a new TLS connection: reusing an ID within
one session is forbidden so that old consent cannot approve a new attempt. After
reconnect, the UI requires a fresh checkbox confirmation. There is no automatic
resume or silent restoration of trust.

**Cancel and Discard have different effects.** Cancel stops the active operation
and closes the connection. Before publication begins, incomplete received data is
deleted and the position resets to zero; the recovery record allows a fresh start
after new consent. Discard, with separate confirmation, deletes an inactive recovery
record and its staging. The sender's original files are never deleted. Discard is
forbidden for an active operation; Cancel is required first. Devices do not share
a deletion transaction: the other device's saved record may need a separate Discard
there or remain until cleanup expiration.

If FileCancel cannot safely be appended after a partially sent TLS frame, the
connection closes. The other peer may interpret this as a network interruption and
retain its verified checkpoint. It never resumes automatically; removing its
recovery data requires local Discard. The device where Cancel was clicked resets
its own partial data.

## Verifiable checkpoints

Before the first offer, Rust builds a complete bounded manifest and computes each
source file's BLAKE3 hash in a streaming pass. Unlike ordinary 2.6 transfers, this
requires reading sources before sending starts. Network data still uses chunks of
at most 64 KiB, without loading a whole file into memory and with one current file
handle.

A checkpoint identifies a continuously verified operation prefix: the next entry
index, a `u64` file offset, and the prefix's BLAKE3 hash. Earlier files finish
completely before the current file's prefix is saved. Directories carry no data.
Intermediate checkpoints occur every **16 MiB**; a final checkpoint may occur at
the exact end of a file. Each file's complete BLAKE3 is verified before advancing
to the next, including zero-byte files.

The receiver independently compares the declared position with actual written bytes
and its own BLAKE3. Positions beyond EOF, invalid indices/hashes, offsets outside
allowed boundaries, and unexpected phases are rejected. Checkpoint persistence
first calls `sync_all` on data, then atomically writes and synchronizes metadata,
and only then sends ResumeAck. The sender also saves the acknowledged position.
Sending bytes alone is never treated as proof that the other side persisted them.

After restart or renewed consent, the receiver rereads saved complete files and the
current prefix, checking their sizes and BLAKE3. Unacknowledged tails are truncated;
later unverified files are removed. Before every resume, the sender revalidates
**all** source files against their saved sizes and full hashes, then checks the
requested prefix. Matching names or sizes are therefore insufficient: same-length
content changes also cause rejection. An offset alone never justifies skipping data.

A lost final Ack can cause retransmission of the last unacknowledged segment.
For an ordinary interruption, this is at most one checkpoint interval of the current
file. A deleted, corrupted, or unauthentic checkpoint is never recovered by guessing
a position: the recovery record is cleaned up, and a new transfer must be created.
Completed user receipts are not deleted.

Active resume progress starts at verified bytes and increases only for actual
transferred payload bytes. Speed and ETA use that starting position, so saved data
cannot create a false MiB/s spike. Source validation, fsync, and staging verification
are excluded from network speed. An empty operation stays at 0% until completion,
then reaches 100%.

## Trust binding and metadata protection

TLS 1.3, SPKI verification, pairing, and the OS keyring are preserved. The new ALPN
is `invardrop/6`, with hello version 6. A 2.6 peer negotiates `invardrop/5` for ordinary
file/folder transfer; a 2.5 peer negotiates `invardrop/4` for single files up to 4 GiB.
Such legacy transfers do not become resumable retroactively.

A recovery record binds the UUID, direction, full original peer key, manifest with
all hashes, position, and trust epoch. Destinations come only from the profile's
fixed `received` directory; source paths come only from native selection in Rust.
Resume/Discard IPC accepts identifiers, never filesystem paths. The manifest follows
2.6's portable rules: no absolute paths, traversal, unsafe components,
symlinks/reparse points, or implicit file replacement.

Metadata in `resume-v1/<UUID>/checkpoint.enc` is protected by ChaCha20-Poly1305 with
a separate key derived from the keyring master key using domain-separated BLAKE3
derivation. Each write uses a new random nonce. The envelope version/purpose and
epoch are included in AAD. Identity/trust ciphertext is not a valid checkpoint, or
vice versa. Records contain the sender's local paths, but these are never sent to
the frontend or peer. Staging file contents are **not encrypted on disk**; the
private profile directory and OS permissions protect them, as with ordinary
received files.

Every Revoke increments the persisted global resume epoch. For simplicity and
security, this invalidates **all** older recovery records in the profile, including
other peers' records. Unblock or new pairing never restores old checkpoints.
If durable revocation persistence fails, the old epoch is forbidden in the current
process; as with trust itself, guarantees after restart require a successful disk
write. Keyring/storage errors never enable a plaintext fallback.

When opening an older profile without resume_epoch, version 2.7 atomically saves
that field before recovery starts. An older 2.6 binary using deny_unknown_fields
will then refuse to open the profile: profile downgrade is unsupported. This does
not prevent ALPN 4/5 network compatibility between different profiles/devices, and
prevents an older version from silently revoking trust without changing the
checkpoint generation.

These measures protect against network attackers and accidental disk corruption.
They do not replace protection from a process running as the same user or detect
rollback of an entire older valid profile together with its trust state. Sources
are not atomic filesystem snapshots: the application rechecks files but does not
lock out external processes for the entire operation.

## Publication and crash recovery

An incomplete folder stays in private `resume-v1/<UUID>/payload`, separate from
completed files. Verified intermediate files are never published individually.
Finalization rechecks all files, synchronizes data/directories, then uses CommitGate
to choose between cancellation and the start of publication.

Before rename, a journal with a new random receipt name is persisted. The completed
file or entire tree is published through one no-replace operation on the same
filesystem (Linux renameat2 / Windows MoveFileExW). Existing destinations are never
replaced. After rename, a `completed` marker is saved; the receiver sends
ResumeComplete, and the sender replies ResumeCompleteAck. Only after that
acknowledgement does the receiver delete the recovery journal.

A crash before rename leaves the saved payload available for validation and resume.
If rename has occurred but `completed` has not yet been saved, recovery verifies
the actual published tree against the full manifest and recognizes completion.
If network acknowledgement was lost, a new offer with the same UUID acknowledges
the already completed receipt instead of creating a copy. Both scenarios require
an intact authentic journal and valid data.

After publication starts, Cancel returns CommitStarted. The finalization owner
must report its actual result even after the connection closes. A disconnection
at this boundary may leave the folder on the receiver while the sender still sees
an error; there is no distributed atomic commit between devices. Restart and the
completion exchange resolve ambiguity while journals remain intact. If a journal
is corrupted/lost, the application neither guesses its association with a receipt
nor deletes published user data.

## Limits, cleanup, and practical limitations

| Resource | Limit |
| --- | --- |
| Operation / single file | 1 TiB |
| Tree entries, including directories | 4096 |
| Depth / component / relative path | 16 components / 200 bytes / 1024 bytes |
| Tree manifest / manifest including hashes | 240 KiB / 512 KiB |
| ResumeOffer frame | 768 KiB |
| Encrypted recovery record | Up to 8 MiB |
| Recovery records per profile | 8 |
| Sum of recovery records' declared sizes | 2 TiB |
| Inactive recovery record age | 7 days |
| Verification/acknowledgement/finalization wait | Up to 5 minutes |

Age is measured from the last saved checkpoint/record change, not from merely
showing it in the UI. Loading and rescanning the directory removes expired, revoked,
and corrupted UUID records; actively leased operations are left untouched. Unknown
names outside this format are never used as deletion paths. Journals of completed
but unacknowledged operations also consume the limit until ResumeCompleteAck,
manual Discard, or age-based cleanup. They appear separately as
`Completed · awaiting acknowledgement`, without Resume. Discard removes only their
recovery metadata; published files remain. If the other side retains an old record
after manual journal deletion and offers the same UUID again, it becomes a new
offer requiring receiver consent. Another separate receipt may be created, but
existing data is never replaced.

The 2 TiB limit bounds declared totals; it neither preallocates storage nor
guarantees free disk space. Each active operation retains at most one unacknowledged
interval; disk exhaustion safely stops writing. If metadata is inaccessible or
corrupted, useful bytes may become impossible to resume. Recovery preserves
verifiable data rather than promising no repeated I/O.

Large sources and staging may require lengthy full rereads. Exceeding the
five-minute wait safely fails the operation; an uninterruptible OS operation may
finish later but cannot bypass CommitGate. A slow network disk or very large tree
may require another attempt. Resume requires the sender's sources and profile;
independent completion by the receiver alone is outside this milestone.

Unix uses private permissions; Windows uses the user profile's ACLs. `sync_all`,
atomic metadata replacement, and no-replace rename depend on OS/filesystem
contracts. Guarantees under physical power loss, controller corruption, stalled
I/O, or unusual network filesystems are not equivalent to a tested normal process
restart. A crash can leave temporary service files; the application never declares
them completed transfers.

This milestone excludes compression, FFmpeg, chunk maps/Merkle trees, parallel
file downloads, automatic resume, a background sender daemon, mtime/ACL/executable-
bit preservation, and filesystem snapshots. Windows GUI, native dialogs, system
Credential Manager, and two physical devices require separate checks; successful
CI does not prove these runtime scenarios.

## Validation

Automated regressions use real TLS and cover repeated interruptions and restart
with the same profile key, nested trees/empty directories/zero-byte files,
persisting and revalidating prefixes, invalid offsets/order/hashes, data before
consent, Cancel versus Discard, and revoke/unblock without restoring the old epoch.
Separate storage tests cover authenticated metadata, publication crash states,
no-replace behavior, record limits, and cleanup. UI tests verify no automatic
resume, specific consent, offline/legacy/revoked states, repeated reconnects,
duplicate actions, and separate Discard confirmation.

The full command set remains unchanged: Rust tests, Rustfmt, strict Clippy,
TypeScript, UI tests, Vite, and a locked Tauri production build. Actual results and
the Linux/Windows CI link apply to a specific feature-branch commit; read them
separately from the guarantees and limitations described here.

## Local validation on 2026-11-14

CachyOS/Niri: the complete **121 Rust tests** and **43 UI tests** passed without
failures/ignored tests; Rustfmt, strict Clippy, both TypeScript checks, Vite
production build, and a locked Tauri release build also passed.

The native production GUI at 360×420 and a separate Rust probe sender used real
Secret Service, mDNS, and TLS. A 256 MiB folder with nested/empty directories and a
zero-byte file was interrupted after an acknowledged checkpoint. Both processes
restarted with the same profiles: trusted identity and verified progress persisted.
Continuation required a new checkbox confirmation and Accept resume in the GUI.
Checks verified final byte integrity, structure, a single atomic receipt, checkpoint
removal after CompleteAck, and normal process exit (code 0). The Rust probe
controlled the source; the GTK chooser was not automated. Test keyring entries
were removed after the scenario.

A separate sender TLS regression confirmed that after interrupting a 17 MiB file,
the verified 16 MiB prefix is not retransmitted; only the 1 MiB tail crosses the
network. Existing single-file/folder and >4 GiB sparse-file regressions remain.
Windows GUI and two physical devices remain separate runtime checks.
