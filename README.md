# InvarDrop

Minimalist LAN file sharing application in development. Target platforms: Linux
and Windows, with Android planned later. CachyOS has been tested locally; Windows
validation is pending a successful CI run.

**Original author: Invar Gervi.** Free and open source under [Apache-2.0](LICENSE).

## Current status — milestone 2.1 (feature branch)

Real automatic mDNS/DNS-SD discovery is implemented in Rust. Nearby instances
appear and disappear in the React UI without refresh. Each run advertises a
random session ID and an anonymous readable name; discovered peers are unpaired.

Pairing, receiver approval, encryption, file/folder transfer, progress, integrity
checks, resuming, conversion, extraction and OS sharing integration are **not
implemented**. Discovery is unauthenticated and does not grant trust or access.
No file-transfer listener is opened in this milestone.

See [discovery architecture and testing (Russian)](docs/discovery.md) for protocol,
privacy decisions, expiration/retry behavior, manual tests and limitations.

The intended product has no accounts, cloud services, advertisements or paid APIs.
Original files must never be modified by future preparation features.

## Development

Use Node.js 24 LTS (see `.node-version`), npm and stable Rust with rustfmt and Clippy.
Installing dependencies requires internet access; the application shell does not
need an external service.

### CachyOS / Arch Linux

First review the official [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/).
Install system packages only with the machine owner's approval:

```sh
sudo pacman -S --needed rustup base-devel webkit2gtk-4.1 openssl librsvg
rustup default stable
rustup component add rustfmt clippy
```

Restart the terminal if Rust is not in PATH. A working desktop session is needed
to open the application. Tray/menu and packaging dependencies will be added only
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

Manual smoke check: launch with `npm run tauri dev -- -- --locked`; confirm the InvarDrop title,
readable development status, absence of device lists and transfer controls,
window resizing, and normal close behavior. Repeat with the release executable.
Discovery has Rust unit tests. Native launch and real-network discovery are
manual checks; CI compilation is not proof that a GUI or cross-device discovery
was exercised. See the discovery document for the current test scenario.

## Structure and security boundaries

- `src/`: React presentation, read-only snapshot command and discovery event subscription.
- `src-tauri/src/discovery/`: protocol, bounded device tracker and networking worker.
- `src-tauri/capabilities/`: event listen/unlisten only; no shell/filesystem access.
- `.github/workflows/ci.yml`: native Linux and Windows build checks.

The production content security policy permits local assets and Tauri IPC only.
Discovery runs in Rust. The UI receives only the data it displays. Future
transport and filesystem services also belong in Rust; conversion and database
dependencies are not introduced in this milestone.

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

No release publication is configured in milestone 1. A later release workflow
will build tagged versions, package installers, include license/author notices,
produce checksums and create a draft GitHub Release. The repository must be
created and publishing approved separately.

## Local validation — 2026-11-12

Validated on CachyOS x86_64, Niri/Wayland, WebKitGTK 2.54.2, Rust/Cargo 1.99.1,
Node.js 26.13.1 and npm 12.2.2. CI uses Node.js 24 as declared in `.node-version`;
that Node version and Windows have not been run locally.

- Frontend type-check, Vite configuration type-check and production build passed.
- Rustfmt and Clippy (`--all-targets`, warnings denied) passed.
- Locked native release build passed; no additional system packages were needed.
- Cargo unit and documentation test harnesses passed with **zero test cases**.
- npm audit reported zero known vulnerabilities; Cargo security audit was not run
  because cargo-audit is not installed.
- Both release and development GUI windows rendered correctly in native WebKitGTK
  and closed normally. Release resizing to 360 × 420 was checked. A subsequent theme/layout update
  fits the entire interface, including author credit, at that size.
- CI YAML was parsed and its OS matrix, read-only permissions and action SHA pins
  were checked locally. GitHub Actions itself has not been run.

No installer packaging, Windows runtime validation or network functionality was
validated in this milestone. No GitHub remote was created and nothing was pushed.

## Automatic appearance

The UI uses light/dark CSS custom properties and `prefers-color-scheme`.
`color-scheme: light dark` also lets the WebView adapt native controls and
scrollbars. There is no saved override, theme toggle, JavaScript subscription or
additional dependency. WebView media-query updates repaint the UI without a
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
