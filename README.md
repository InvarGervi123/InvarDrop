# InvarDrop

Minimalist LAN file sharing for Linux and Windows, with experimental Android
ARM64 support. Desktop builds are CI targets. Basic bidirectional Android/Linux
transfers and automatic Downloads saving have been manually verified on a
Galaxy S25, as reported by the user; see the
[Android verification scope and limitations](docs/android-stabilization.md).

**Original author: Invar Gervi.** Free and open source under [Apache-2.0](LICENSE).

## Beta release — v0.1.0-beta.1

**Use it. It works. Forget it.** Fast, minimal LAN sharing without accounts,
cloud services, telemetry, or unnecessary background services.

Download the [first public Beta](https://github.com/InvarGervi123/InvarDrop/releases/tag/v0.1.0-beta.1).
See [installation, checksum verification and known limitations](docs/beta-release.md).
Linux x86_64: AppImage / Debian; Windows x86_64: NSIS / MSI. This is a testing
prerelease, not a production-readiness or full cross-platform compatibility claim.
The installers are unsigned; do not bypass platform security warnings.

Milestone 3.1 is merged through PR #15. See
[cross-device testing coverage](docs/cross-device-testing.md),
[packaging details](docs/packaging.md), and [UI/UX validation](docs/ui-ux-polish.md).

Real automatic mDNS/DNS-SD discovery is implemented in Rust. Nearby instances
appear and disappear in the React UI without refresh. Each run advertises a
random session ID and an anonymous readable name. Discovery names remain unverified.

Secure pairing uses mutual TLS 1.3, a persistent P-256 identity and an explicit
comparison of the full code on both devices. Trust is saved only after both
approvals. Reconnect verifies the saved public key; the UI supports disconnect,
trust revocation and explicitly allowing a fresh pairing after revocation.
The OS credential store protects the master key for encrypted local identity and
trust state. Linux needs an available, unlocked Secret Service; Windows uses
Credential Manager; Android uses Android Keystore. Storage failure disables
secure connections without a plaintext
fallback.

Trusted devices can send a file, multiple files or a complete nested folder over
their existing TLS connection. Native dialogs select files or a folder; the receiver
explicitly approves the name, file count and total size of each operation.
Empty directories and zero-byte files are supported, with a 1 TiB total limit,
4096 entries and depth 16. Files stream in bounded 64 KiB chunks, with incremental BLAKE3
verification before finalization. Incomplete data stays private. Verified receipts
are saved automatically to the system Downloads directory under
`InvarDrop/received-<transfer-id>/<original-name>`; the UI shows the public location.
See [Downloads saving, recovery and platform limitations](docs/downloads.md).
Existing files are never replaced. File names and destinations cannot be supplied
as arbitrary filesystem paths through IPC or by the peer.

Milestone 2.7 uses ALPN `invardrop/6` for resumable transfers. It also negotiates
`invardrop/5` for ordinary 2.6 file/folder transfers and `invardrop/4` for 2.5 single
files up to 4 GiB. Identities and trust remain compatible. Discovery remains
unauthenticated.

Interrupted version 6 transfers preserve verified prefixes in private staging,
with authenticated restart checkpoints every 16 MiB and at each file boundary.
Trusted reconnect exposes Resume and Discard controls; it never starts payload
automatically. Resume requires sender action and fresh receiver approval. Every
source and saved prefix is rehashed before reuse, and the whole verified result
is published without replacing an existing destination. Cancel removes incomplete
data and resets the position; Discard removes the dormant recovery record.
Revoking trust invalidates old checkpoints. Limits: 8 recovery records, 2 TiB total
declared size, and 7 days of inactivity. Source and staging verification can require
substantial disk IO; resume avoids retransmitting verified data, not rereading it.

Optional OGG-to-MP3 conversion and post-transfer ZIP extraction are implemented
with separate explicit approval. FFmpeg is selected locally and is not bundled.
See [processing dependencies and security limits](docs/optional-file-processing.md).
Compression and OS sharing integration are **not implemented**. See [resumable transfers](docs/resumable-transfers.md)
for consent, encrypted metadata, recovery, cleanup, limits and security considerations.
See [large files and folders](docs/large-files-folders.md) for the 2.6
protocol and storage foundations.

See [discovery architecture and testing](docs/discovery.md) for protocol,
privacy decisions, expiration/retry behavior, manual tests and limitations.
See [secure pairing and threat model](docs/security.md) for the pairing
transport protocol, storage policy, approval boundaries and validation results.
See [single-file transfer](docs/transfers.md) for the initial wire format,
safe storage, cancellation boundaries, tests and limitations.

The intended product has no accounts, cloud services, advertisements or paid APIs.
Original files must never be modified by future preparation features.

Incoming requests show the device-reported sender name, readable size and explicit
Accept/Decline states. Acceptance requires confirming the specific file; expired
requests cannot be approved. See [receiver acceptance](docs/receiver-acceptance.md).

Transfers display measured bytes, data percentage, MiB/s and ETA when reliable,
with a Cancel action on both sides. A cancelled transfer closes the TLS session.
See [progress and cancellation](docs/transfer-progress.md) for finalization
races, testing and limitations.

## Development

Use Node.js 24 LTS (see `.node-version`), npm and stable Rust with rustfmt and Clippy.
Installing dependencies requires internet access; runtime discovery needs only
a local network and does not use an external service.

### CachyOS / Arch Linux

First review the official [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/).
Install system packages only with the machine owner's approval:

```sh
sudo pacman -S --needed rustup base-devel webkit2gtk-4.1 openssl librsvg
rustup default stable
rustup component add rustfmt clippy
```

Restart the terminal if Rust is not in PATH. A working desktop session is needed
to open the application. Native file selection uses the desktop file chooser
through XDG Desktop Portal (with the dialog library's system-helper fallback).
Tray/menu and packaging dependencies will be added only
when those features are enabled.

### Windows

Install Node.js 24 LTS, Rust via rustup using the MSVC toolchain, Microsoft C++
Build Tools with the Desktop development with C++ workload, and WebView2 Runtime.
See the official Tauri prerequisites for detailed installation instructions.

### Commands

```sh
npm ci
npm run check
npm run build
npm run tauri dev -- -- --locked
```

`npm run dev` starts only the frontend preview on loopback port 1420.
`npm run tauri dev -- -- --locked` starts the actual native application and its development server.

Build the desktop executable without installers:

```sh
npm run tauri build -- --no-bundle -- --locked
```

Output: `src-tauri/target/release/invardrop` on Linux, `invardrop.exe` on Windows.
These executables require the platform runtime libraries. Build the native installation
packages (Linux: AppImage and Debian; Windows: NSIS and MSI) with:

```sh
npm run test:packaging
npm run tauri build -- --ci -- --locked
```

Build/package tooling requires network access on a fresh cache. See
[packaging instructions](docs/packaging.md) for the Ubuntu 24.04 baseline,
WebView2 offline prerequisite, data retention and unsigned testing artifacts.

## Verification

```sh
npm run check
npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo clippy --locked --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --locked --manifest-path src-tauri/Cargo.toml
```

Manual discovery smoke check: launch with `npm run tauri dev -- -- --locked`; confirm the
local anonymous name and discovery status. Start a second instance, check mutual
discovery without refresh, then close it and check disappearance. Verify window
resizing and normal close behavior. Repeat with the release executable.
Discovery and secure pairing have Rust tests, including real TCP/TLS connections
and encrypted storage. Native launch, system keyring and real-network discovery
need separate checks; CI compilation is not proof that a GUI or cross-device
connection was exercised. See [security validation](docs/security.md) for the
milestone 2.2 two-profile runtime scenario and its limitations.

## Structure and security boundaries

- `src/`: React discovery/pairing/transfer UI, snapshots and explicit actions.
- `src-tauri/src/discovery/`: protocol, bounded device tracker and networking worker.
- `src-tauri/src/secure/`: identities, encrypted trust storage, verified TLS,
  pairing and transfer actors, streaming file IO.
- `src-tauri/capabilities/`: event listen/unlisten only; no shell/filesystem access.
- `.github/workflows/ci.yml`: native Linux and Windows build checks.

The production content security policy permits local assets and Tauri IPC only.
Discovery and secure transport run in Rust. The UI receives display data and
submits pairing decisions by connection ID; it never receives private keys,
master keys or routable endpoints. Native file selection and all file IO run in Rust;
Audio conversion invokes an explicitly selected FFmpeg process; ZIP parsing stays
in Rust. No database dependency or automatic processing is introduced.

## CI and distribution

GitHub Actions checks pull requests and pushes to `main` / `feature/**` on Ubuntu 24.04 and
Windows Server 2022. It type-checks, builds the native executable, checks Rust
formatting, runs Clippy, Cargo tests and installer safety checks, inspects package
payloads and uploads native installers, licenses, instructions, audit reports and
SHA-256 manifests as testing artifacts retained for seven days.
Actions are pinned by commit SHA; workflow permissions are read-only.
Dependabot proposes dependency and workflow updates weekly.

Node dependencies are locked in `package-lock.json`; Rust dependencies are locked
in `src-tauri/Cargo.lock`. Native build, lint and test commands use `--locked`
so dependency resolution cannot silently change. Both lockfiles belong in Git.

Public source: [InvarGervi123/InvarDrop](https://github.com/InvarGervi123/InvarDrop).
The first Beta is published manually from an exact verified commit. CI uploads
testing artifacts; it does not publish releases or automatically update applications.

## Local validation — milestone 2.1, 2026-11-12

Validated on CachyOS x86_64, Niri/Wayland, WebKitGTK 2.54.2, Rust/Cargo 1.99.1,
Node.js 26.13.1 and npm 12.2.2. CI uses Node.js 24.

- Frontend type-check, Vite configuration type-check and production build passed.
- Rustfmt and Clippy (`--all-targets`, warnings denied) passed; 13 Rust tests passed.
- Locked native release build passed; no additional system packages were needed.
- Independent native processes discovered one another through real mDNS, excluded
  themselves, kept live peers past the stale timeout and removed stopped/crashed peers.
- A socket-denial test confirmed unavailable status, automatic retries and clean stop.
- Production GUI rendered the real peer list at 360 × 420 and closed with exit 0.
- The npm installation audit reported zero known vulnerabilities. Cargo security
  audit was not run because cargo-audit is not installed.

See the [CI workflow](https://github.com/InvarGervi123/InvarDrop/actions/workflows/ci.yml)
for hosted Linux/Windows build results and [discovery test notes](docs/discovery.md)
for exact scope. Two separate physical devices, Windows GUI/multicast, installer
packaging, Wi-Fi/VPN transitions and packet-loss behavior remain unverified.
CI builds are not a substitute for those runtime/network tests.

## Automatic appearance

The UI uses light/dark CSS custom properties and `prefers-color-scheme`.
`color-scheme: light dark` also lets the WebView adapt native controls and
scrollbars. There is no saved theme override, theme toggle, JavaScript theme subscription or
additional theme dependency. WebView media-query updates repaint the UI without a
reload. Tauri keeps its default system window theme.

Windows uses the system appearance exposed by WebView2. On Linux, WebKitGTK uses
GTK appearance. On CachyOS/Niri, keep the GTK theme and desktop color-scheme
consistent: a hard-coded dark GTK theme can keep WebKitGTK dark even when the
portal reports a light preference. Configure this in the desktop's theme tooling;
InvarDrop does not change the user's desktop settings.

Production GUI validation on CachyOS/Niri confirmed dark → light → dark in the
same process/window, using matching GTK and desktop preferences. Original desktop
settings were restored afterward. Both palettes fit at 360 × 420; text/background
contrast is at least 5.97:1 for all text color pairs. Windows runtime testing is
still pending; no claim of a local Windows test is made.

Optional OGG-to-MP3 preparation and post-transfer ZIP extraction require separate
explicit approval. FFmpeg is optional, selected locally, and not bundled.
See [processing dependencies, security limits and tests](docs/optional-file-processing.md).
