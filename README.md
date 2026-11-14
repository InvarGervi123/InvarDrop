# InvarDrop

Minimalist LAN file sharing application in development. Target platforms: Linux
and Windows, with Android planned later. Both are CI build targets; native
discovery has been exercised locally on CachyOS.

**Original author: Invar Gervi.** Free and open source under [Apache-2.0](LICENSE).

## Current status — milestone 2.4 (feature branch)

Real automatic mDNS/DNS-SD discovery is implemented in Rust. Nearby instances
appear and disappear in the React UI without refresh. Each run advertises a
random session ID and an anonymous readable name. Discovery names remain unverified.

Secure pairing uses mutual TLS 1.3, a persistent P-256 identity and an explicit
comparison of the full code on both devices. Trust is saved only after both
approvals. Reconnect verifies the saved public key; the UI supports disconnect,
trust revocation and explicitly allowing a fresh pairing after revocation.
The OS credential store protects the master key for encrypted local identity and
trust state. Linux needs an available, unlocked Secret Service; Windows uses
Credential Manager. Storage failure disables secure connections without a plaintext
fallback.

Trusted devices can send one ordinary file at a time over their existing TLS
connection. The sender selects a file with a native dialog; the receiver must
accept each offer. Files stream in bounded 64 KiB chunks, with incremental BLAKE3
verification before finalization. Received files go to a private, separate receipt
directory under the profile's `received` folder; the UI shows the local path.
Existing files are never replaced. File names and destinations cannot be supplied
as arbitrary filesystem paths through IPC or by the peer.

Both devices need milestone 2.4: ALPN `invardrop/3` and hello version 3 intentionally
reject old transport versions; stored identities and trust remain compatible.
Folder transfer, resume, advanced progress, conversion/FFmpeg, extraction and OS
sharing integration are **not implemented**. Discovery remains unauthenticated.

See [discovery architecture and testing (Russian)](docs/discovery.md) for protocol,
privacy decisions, expiration/retry behavior, manual tests and limitations.
See [secure pairing and threat model (Russian)](docs/security.md) for the pairing
transport protocol, storage policy, approval boundaries and validation results.
See [single-file transfer (Russian)](docs/transfers.md) for the current wire format,
safe storage, cancellation boundaries, tests and limitations.

The intended product has no accounts, cloud services, advertisements or paid APIs.
Original files must never be modified by future preparation features.

Incoming requests show the device-reported sender name, readable size and explicit
Accept/Decline states. Acceptance requires confirming the specific file; expired
requests cannot be approved. See [receiver acceptance (Russian)](docs/receiver-acceptance.md).

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
These executables require the platform runtime libraries; they are not portable installers.

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
conversion and database dependencies are not introduced in this milestone.

## CI and distribution

GitHub Actions checks pull requests and pushes to `main` / `feature/**` on Ubuntu 24.04 and
Windows Server 2022. It type-checks, builds the native executable, checks Rust
formatting, runs Clippy and Cargo tests, then uploads per-platform executables with the license and README (retained for
seven days).
Actions are pinned by commit SHA; workflow permissions are read-only.
Dependabot proposes dependency and workflow updates weekly.

Node dependencies are locked in `package-lock.json`; Rust dependencies are locked
in `src-tauri/Cargo.lock`. Native build, lint and test commands use `--locked`
so dependency resolution cannot silently change. Both lockfiles belong in Git.

Public source: [InvarGervi123/InvarDrop](https://github.com/InvarGervi123/InvarDrop).
No release publication is configured. A later, separately approved release
workflow may package installers and create a draft GitHub Release.

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
