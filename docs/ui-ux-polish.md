# Milestone 2.9 — UI/UX Polish

Milestone 2.8 was reviewed and merged through
[PR #12](https://github.com/InvarGervi123/InvarDrop/pull/12), after successful Linux
and Windows push and pull-request CI for `10d75e8`. This milestone refines the
existing interface and keeps the transfer architecture and protocol unchanged.

## Layout and appearance

The compact header, consistent cards, typography, inline SVG icons, button sizing,
and spacing work within a 360×420 client area and expand to larger windows.
The workspace scrolls independently of the header/footer and is keyboard focusable;
long device names, paths, fingerprints, and filenames wrap instead of overflowing.
Native controls and CSS retain automatic system light/dark appearance through
`color-scheme` and `prefers-color-scheme`; there is no theme override, subscription,
or new dependency. Reduced-motion and forced-colors preferences are respected.

Discovery starts with an explicit loading state, then displays either devices or
an explanation of how to find peers. Discovery failures explain automatic retries
and local-network checks. Optional processing settings are collapsed below the
nearby-device list until needed, leaving the main workflow easy to find.

## Trust, consent, and recovery

- Every discovery name is labeled **Unverified name**. An mDNS name cannot identify
  a trusted key. **Trusted key** appears only for authenticated connections or
  saved trusted fingerprints; the full comparison code remains mandatory for pairing.
- Pairing consent is bound to the connection and comparison code. Duplicate or
  opposite decisions are blocked while IPC is pending and until the backend state
  advances. No approval control receives automatic focus.
- Trust revocation now requires a separate confirmation. The prompt explains that
  revocation disconnects the peer and invalidates all saved recovery records in
  this profile, including other peers' records, while keeping completed files.
- Discard retains its separate confirmation and differs from Cancel. Inline
  destructive confirmations focus the safe Keep action and restore focus to their
  trigger when dismissed. Active recovery data still cannot be discarded.
- Progress bars expose readable verified/transferred and remaining byte counts to
  assistive technology. Percentage and speed still use real backend measurements.
  ETA uses seconds, minutes, or hours and disappears outside active data transfer,
  so completed or paused operations never display stale speed/ETA.
- Error notices retain backend details and add guidance for network failures,
  credential storage, profile locks, revocation, changed sources, and resource
  limits. Reconnect continues to pin the saved peer key.

## Optional processing

OGG detection still requires an explicit Keep originals or Convert decision.
Keep originals remains the default, and conversion is disabled until a local
FFmpeg executable is selected. The new read-only `processing_converter_status`
command reports only whether a selection exists; it never exposes the path,
launches FFmpeg, searches PATH, or promises codec/executable availability. The
selection lasts only for this application session. Actual conversion failures
remain explicit and preserve originals.

Processing uses readable waiting, converting, offering, prepared, extracting,
completed, failed, and cancelled labels. Cancellation is shown as pending until
the backend confirms its result. Snapshot polling schedules the next request only
after the preceding request settles and avoids re-rendering identical job lists.
Polling failures are visible and retry automatically. Newly requested processing
stages scroll into view once, without moving keyboard focus to approval controls
or moving the page on subsequent progress updates.

ZIP extraction remains a separately approved stage after verified receipt.
Consent is bound to the connection and transfer ID, and a synchronous guard blocks
duplicate extraction requests. Failed commands allow deliberate retry. The UI
explains that the ZIP stays saved, extraction creates a separate copy, and content
is never automatically opened or executed. Dismiss hides a processing result and
does not delete originals, receipts, or prepared data required by resume.

## Validation

Local validation on 2026-11-15 (CachyOS/Niri):

- 136 Rust tests, including the added session-only FFmpeg-selection regression.
- 77 UI tests across five files, including pairing consent, challenge changes,
  duplicate actions, missing FFmpeg, polling failures/backpressure, extraction
  retries, fresh receipt consent, stale ETA, loading/empty states, and safe focus
  during Revoke/Discard confirmations.
- Rustfmt, strict Clippy, both TypeScript checks, Vite production build, and locked
  Tauri production build without bundles.
- 48 WebKitGTK layout/theme/focus scenarios: eight application/component states
  at actual 360×420, 720×520, and 1024×768 sizes in both light and dark appearance.
  Real React-rendered markup and production CSS were used; layout fixtures did
  not simulate network functionality. No horizontal overflow was found. Theme
  changes were confined to the test process. The tested normal text/background
  pairs have contrast ratios of at least 5.91:1.

The native production GUI was exercised separately with a diagnostic Rust sender
using real Secret Service, discovery, authenticated TLS, and BLAKE3. The sender
selects test sources through the normal Rust API; it does not bypass pairing or
receiver consent. At 360×420, checks covered full-code pairing, Accept disabled
before consent, Decline without a receipt, fresh consent for a new offer, successful
ZIP extraction with nested/empty files, rejection of `../outside`, preservation of
both original ZIPs, and cleanup of incomplete extraction data. Cancelling the trust
confirmation kept the secure connection; explicitly confirming revocation closed
it while retaining completed receipts. The window also ran at 720×520, exited
normally with code 0, and isolated test credentials were removed.

Hosted Linux/Windows results apply to the specific feature-branch
commit in [GitHub Actions](https://github.com/InvarGervi123/InvarDrop/actions/workflows/ci.yml).

## Limitations

Native Windows GUI, WebView2/system-theme changes, Windows screen readers, physical
cross-device transfers, and native file/executable picker automation require
separate runtime testing. DOM tests and layout fixtures are not a complete
assistive-technology audit. Processing history and ZIP extraction offers still do
not survive restart; prepared transfer sources and verified checkpoints retain
Milestone 2.7/2.8 recovery behavior. FFmpeg is optional, external, explicitly selected,
and not bundled. Existing cancellation, finalization, local-account, power-loss,
and filesystem limitations in the transfer/processing documentation remain.
