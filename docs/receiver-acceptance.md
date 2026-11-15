# Milestone 2.4 — Receiver Acceptance UI

The cancellation/finalization boundaries and current Milestone 2.5 protocol are
covered in [transfer-progress.md](transfer-progress.md). The historical context of
Milestone 2.4 (PR #8) is retained below.

## Incoming requests

The interface extends the existing Milestone 2.3 transfer workflow without
rewriting streaming, BLAKE3 verification, the fixed inbox, or no-clobber publication.
An incoming request clearly shows the sender's device name, filename, and
human-readable size, with waiting, accepted, declined, expired, completed, and
failed states. The name arrives over authenticated TLS but is self-reported by the
peer; it is not a key identity. Empty names, names over 80 UTF-8 bytes, control
characters, and bidi-control characters are rejected before reaching the view.

The sender uses its local discovery name. ALPN `invardrop/3`, hello version 3, and
`file_offer.sender_name` require both applications to update. Identity/trust from
2.2–2.3 remains valid without pairing again. A negative TLS test verifies rejection
of the 2.3 transport.

Accept requires a separate, initially unchecked `I want to save this file`
checkbox. Buttons use `type=button`, with no autofocus, form submission, or default
Enter action. Decline does not require the checkbox. A synchronous ref prevents
duplicate actions until a new snapshot arrives, and checkbox consent is bound to
the transfer ID, so it cannot carry over to a later request. An IPC error permits
retry; the backend independently validates phase, ID, direction, decision, and
deadline.

No sink is created before Accept. Decline saves no file. After 90 seconds, the
offer expires, the connection closes, and the UI shows `expired`. Premature payload
data closes the connection. Cancellation/finalization boundaries from
[transfers.md](transfers.md) remain applicable. Accepted is distinct from Completed:
completion requires hash verification and successful saving.

## Validation

`npm test` runs Vitest/jsdom/Testing Library tests in both CI jobs. DOM tests cover
the consent checkbox, duplicate actions, IPC failure, a new transfer ID, readable sizes, and
terminal states. Real TLS Rust regressions cover expiration without a file, late
approval, and unsafe sender names without exposing an offer. Existing negative
tests from 2.3 for rejection, corrupted/truncated data, disconnection, overwrites,
and replay remain.

The native `receiver_probe` uses the standard Service/Source API and discovers only
the explicitly specified discovery name. Its stdin commands are `status`,
`approve <receiver code>`, `send`, and `quit`; the code travels through a pipe and
is not logged. This allows testing the actual receiver Tauri IPC without
automating the GTK picker. The probe is not
part of the production UI.

## Local results — 2026-11-14

13 UI tests, 59 Rust tests, TypeScript, Rustfmt, strict Clippy
`--all-targets -- -D warnings`, and a locked production build passed. The Linux GUI
and diagnostic sender used real Secret
Service, mDNS, and TLS. Tests verified pairing using the full code without logging
it, the sender name, a 1 MiB offer with Accept disabled until consent, Decline with
no saved file, and a new offer accepted through IPC with Completed and identical
BLAKE3/bytes. The GUI was tested repeatedly at 360×420 and exited with code 0;
isolated test credentials were deleted. Existing CSS retains the system light/dark
theme behavior introduced in 2.1.

This validates the native receiver workflow with a programmatic sender. It does
not validate the sender picker, two GUI instances on physical devices, or the
Windows keyring. The 2.3 limitations around uninterruptible finalization, lost
acknowledgements, and crash cleanup remain. CI runs the checks on both platforms.
