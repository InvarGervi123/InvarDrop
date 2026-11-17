# Automatic Downloads saving

The application saves newly completed and verified transfers to the system
Downloads directory, under `InvarDrop/received-<transfer-id>/<original-name>`.
Each operation has a separate receipt directory. Original names and nested
structure are preserved; receiving the same filename again never replaces an
older receipt. A retry of the same operation reuses a matching verified receipt
and rejects an edited or conflicting destination.

## Platforms

- Linux: Tauri's `download_dir()` resolves the configured XDG Downloads location.
  The implementation does not assume `$HOME/Downloads`.
- Windows: the same API resolves the system-known `FOLDERID_Downloads`, including
  redirected Downloads. Staging uses a protected owner/SYSTEM-only DACL rather
  than inheriting access from a possibly shared parent.
- Android 10 / API 29 and newer: the app inserts its own rows into
  `MediaStore.Downloads`, with `RELATIVE_PATH` under `Download/InvarDrop` and
  `IS_PENDING=1`. No broad storage permission, legacy-storage opt-out, or direct
  public-file writes are added. Android's physical directory name is `Download`;
  the UI uses the familiar Downloads label. Android 7–9 retain the private
  verified receipt but report that public saving failed; there is no insecure
  fallback or automatic storage-permission request.

The final public location is shown in the transfer card. The private receipt
remains authoritative for recovery and optional ZIP extraction. Existing
receipts, identities, keys and trusted peers are not moved, migrated, or deleted.
There is no bulk export of historical private receipts. Headless test/probe
services keep their isolated private behavior; the application explicitly wires
in the system Downloads destination.

## Integrity, approval and publication

Receiver approval is still mandatory before any transfer payload is saved.
Declined, expired and cancelled requests never invoke Downloads publication.
TLS 1.3, pairing, trust checks, framing, BLAKE3 proofs and encrypted checkpoints
are unchanged. Old file and folder protocols also use this post-verification
publication path.

Incoming bytes and interrupted transfers stay in private profile storage. After
BLAKE3 verification and private atomic finalization, a bounded worker copies the
verified receipt. Sizes and counters are 64-bit and buffers are 64 KiB. There is
no whole-file memory allocation.

Desktop copies are staged in a private sibling of the configured Downloads
folder, outside Downloads itself. The copy is verified against the original
BLAKE3 manifest and synced before atomic no-replace directory rename. Files,
folders and empty directories appear together only after successful verification.
A different-volume rename or unavailable private staging parent fails closed;
it never falls back to a visible, partially written file. Normal failures remove
staging. A hard process/OS crash can leave a private `.invardrop-save-*` sibling
for manual cleanup; it is not a published receipt and cannot overwrite user data.

For Android, Rust verifies the original BLAKE3 proof while deriving SHA-256 for
the native copy. Kotlin verifies that SHA-256 and length during streaming, syncs
the file, reads the pending destination back, and only then clears `IS_PENDING`.
The extra digest is local to copying; it does not replace or change the BLAKE3
transfer protocol. The OS allocates a unique MediaStore filename on collisions.
A private atomic SharedPreferences journal retains owned row URIs for restart
recovery. Pending rows are never treated as completed downloads. Failed copies
remove only owned pending rows; published or user-edited files are never opened
for writing or deleted. An uncertain publish result is recovered by readback.
An orphan inserted immediately before a crash can remain pending until Android's
expiry maintenance; a 24-hour expiration is requested for pending rows.

Android folders preserve relative structure. Empty directories use validated
app-created directories under the public receipt; if scoped-storage policy
rejects creation, saving fails without requesting broader access. MediaStore
publishes individual verified files, so a folder's public visibility is not one
atomic operation. A later failure can leave some already verified files visible;
the complete private folder remains intact and retry fills the remaining files.
No incomplete or corrupted file is published. Folder creation and MediaStore
behavior still require verification on a physical Android device.

Only successful saving marks the transfer complete in the application and sends
the completion acknowledgement. A save failure retains verified private data
and reports an actionable error. Version 6 recovery can retry saving after a
trusted reconnect, without retransmitting bytes already received. A completely
received operation was already approved; recovery only finishes its publication.
Interrupted payloads still require the existing fresh resume approval. Revoked
peers cannot use recovery. Existing commit/cancellation boundaries and protocol
wait limits remain: once private commit starts, cancellation cannot undo a
publication. Very slow large finalizations can exceed the existing five-minute
connection wait while the owned worker finishes; version 6 can recover the lost
acknowledgement by reconnecting. Older protocols have no durable retry ticket.

Private and public verified copies both consume disk space. Private copies are
retained to preserve recovery and processing behavior; this change adds no
history-deletion policy. Downloads follow normal OS/user access and retention
rules, including retention after app uninstallation. Identity/trust material
stays private and is never exported.

## Focused validation

- Rust filesystem regressions: duplicate names and idempotent recovery, edited
  destination preservation, corrupted sources, failed writes, staging cleanup
  and Unix mode 0700, symlinks/collisions, nested/empty folders and zero-byte files,
  unsafe metadata and IDs, and sparse sizes above 4 GiB without giant reads.
- Real loopback TLS with independent profiles/identities and isolated Downloads:
  approval/rejection, two receipts with the same filename, private/public byte
  comparisons, legacy single/bundle transfers, restart after a failed public save
  with the sender source removed, and cancellation at the pre-commit boundary.
- Pure JVM publication regressions: pending visibility boundary, source corruption,
  truncation/oversize, destination readback corruption, restart idempotency,
  preservation of edited public files, partial-row retry, zero-byte files and
  commit failure. Run with `python3 scripts/test-android-import.py`; this also
  retains the existing SAF import regressions.
- UI tests verify public destination display while keeping the private ZIP
  processing source, plus existing acceptance/cancellation/error interactions.

Native Windows Downloads and loopback TLS regressions now pass in CI. Basic
Android/Linux transfers and Downloads saving on a Galaxy S25 are user-confirmed.
These results do not establish Windows GUI/installer behavior, all Android
MediaStore failure cases or background reliability. See the
[current verification record and remaining device checks](android-stabilization.md)
for commit-specific CI evidence and the distinction between automated and
physical-device coverage.
