# Milestone 2.5 — Transfer Progress, Speed, and Cancellation

Current Milestone 2.7 supplement: [resumable transfers](resumable-transfers.md).
The Milestone 2.6 large-file/folder extensions are documented in
[large-files-folders.md](large-files-folders.md). This document retains the
historical Milestone 2.5 context.

## Actual byte counts and estimates

The existing TLS 1.3 transport, 64 KiB chunks, and BLAKE3 verification are preserved;
no new libraries were added. The sender counts payload after completed `send/flush`
frames; the receiver counts it after `Sink.write/flush`. These are local byte
counts, excluding TCP retransmissions and TLS overhead, rather than durable peer
acknowledgements. In-flight values can differ between sides. On completion, both
reach the file size, and originals remain unchanged.

Rust exposes `transferred`, `bytesPerSecond`, and `etaSeconds`. Counters advance
only per actual chunk, while full snapshots/events are limited to one every 250 ms
during activity. There is no timer while waiting for approval or after completion,
and the frontend adds no timers in this milestone. Speed uses an approximately
1-second window with up to 5 monotonic samples. ETA appears only after 1 second at a
positive speed with bytes remaining, and only if the estimate is at most one day.
It disappears when progress stalls. These are throughput estimates, not fsync
guarantees.

Payload progress can reach 100% before Completed: BLAKE3, saving, and acknowledgement
still follow. An empty file stays at 0% until Completed, then reaches 100%, without
invented speed/ETA. The existing 4 GiB limit remains in this milestone.

## Cancellation protocol

Both sides have `Cancel transfer` bound to the connection ID and transfer ID.
Receiver Decline keeps the connection; Cancel closes TLS and requires reconnect
before another file.
Duplicate/stale cancellation cannot affect a new transfer. The UI shows cancelling,
then cancelled after stopping; network failures show Failed.

A separate watch channel delivers cancellation without queuing behind file writes
or UI actions. At a frame boundary, the peer sends `file_cancel(id)` with a 500 ms
deadline, then performs TLS shutdown within at most 1 s. The reader handles cancel
outside the file queue, including during finalization. Mid-frame cancellation
cannot append a control message safely and closes the connection. The peer may
therefore see Failed instead of Cancelled, but no incomplete file is published.

ALPN `invardrop/4` and hello version 4 require both applications to update.
Identities and stored trust remain unchanged.

## Cancellation and finalization races

`CommitGate` atomically chooses between cancellation and publication. Cancellation
before publication prevents `persist_noclobber`, deletes the tempfile, and forbids a
late Completed result. Once commit starts, cancellation is rejected with an explicit
error; the UI displays Saving verified file and hides Cancel, avoiding a false
Cancelled state.

Cancellation remains possible during flush/fsync. A tracked task owns finalization,
waits for the actual OS result, and checks the gate before publication. Transport
loss cancels an unpublished commit. No-clobber publication, size/hash validation,
safe names, and explicit receiver approval are preserved.

## Validation results

66 Rust tests, 23 UI tests, Rustfmt, strict Clippy, TypeScript, and a locked Tauri
production build passed. Regressions cover empty/multichunk files and exact totals;
real TLS progress monotonicity, snapshot throttling, and ETA; sender cancellation under backpressure;
receiver cancellation after bytes arrive; stale/repeated cancellation; deterministic
races before/after the commit gate; disconnection during finalization; cleanup;
and no false completion.

The native Linux GUI at 360×420 used real keyring, mDNS, and TLS with a diagnostic
sender. Tests covered Decline without a file, a successful 1 MiB transfer with hash/
byte comparison, and a subsequent transfer with actual partial progress followed
by receiver Cancel. Only the test sender paused after sending real data; no OS
network settings or simulated progress were used. Staging was removed, the earlier
receipt remained, the peer showed Cancelled, and the GUI exited with code 0.
Sender Cancel is covered by Rust and UI tests. The two separate test profiles used
separate credentials, which were deleted afterwards.

## Remaining limitations

Windows GUI/keyring, the native sender picker, and two physical devices have not
been verified. There is no resume or distributed transaction in this milestone.
If the sender cancels while waiting after the receiver has published but before
acknowledgement, the valid file may remain; it is not deleted automatically.

Blocking OS I/O/fsync cannot be aborted instantly, so Cancelling can wait for I/O.
A crash, abnormal shutdown, or deletion error can leave staging behind, without
reporting false completion. Speed varies with network conditions, ETA is approximate,
and no distributed atomicity is promised.
