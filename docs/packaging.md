# Milestone 3.0 — Installers and Packaging (Step 12)

Milestone 2.9 was reviewed and merged through
[PR #13](https://github.com/InvarGervi123/InvarDrop/pull/13), after successful Linux
and Windows checks for `4147e3d` and the PR. This milestone adds installation
packages. The application version remains **0.1.0**, not 3.0; milestones are
engineering checkpoints, not release version numbers. No GitHub Release or
automatic installer distribution is configured.

## Targets and prerequisites

| Platform | Formats | Scope |
| --- | --- | --- |
| Linux x86_64 | `.deb`, `.AppImage` | CI build baseline: Ubuntu 24.04, glibc 2.39 and WebKitGTK 4.1 |
| Windows x86_64 | NSIS `-setup.exe`, WiX `.msi` | Native Windows Server 2022 CI builds; target desktop: Windows 10/11 with WebView2 |

Linux needs a working graphical session, GTK 3, WebKitGTK 4.1, a session D-Bus,
an unlocked Secret Service credential store, and a functioning desktop file picker
(normally XDG Desktop Portal). A missing/locked credential store disables secure
connections; no plaintext identity fallback is introduced. Debian packages declare
system library dependencies. AppImage carries framework libraries but is not a
universal Linux compatibility layer. Older glibc systems, ARM, macOS, RPM, and
installer migrations between formats are outside this milestone.

[Tauri's Linux guidance](https://v2.tauri.app/distribute/appimage/) requires building
on the oldest intended compatible base. CI artifacts use Ubuntu 24.04. Packages
built locally on newer distributions can require newer libraries and must not be
presented as the canonical Ubuntu-compatible build. Other distributions require
runtime verification; see the actual validation scope below.

## Linux installation and removal

Download only the intended CI artifact, verify its SHA-256 against the included
manifest, and review its provenance. A checksum detects corruption; it does not
replace a trusted signature or establish the publisher's identity.

For the Debian package on a compatible system:

```sh
sudo apt install ./InvarDrop_0.1.0_amd64.deb
invardrop
sudo apt remove invar-drop
```

Installing with apt may fetch missing system dependencies. Fully offline Debian
installation requires those dependencies to be installed or available locally.
The desktop menu entry is InvarDrop, with the application icon, Network/File
Transfer categories, and AppStream metadata. The executable is `/usr/bin/invardrop`. There are no custom
maintainer scripts, services, firewall changes, file associations, or autostart.

For AppImage:

```sh
chmod u+x InvarDrop_0.1.0_amd64.AppImage
./InvarDrop_0.1.0_amd64.AppImage
```

If FUSE is unavailable, the runtime also supports extraction/run without a mount:

```sh
./InvarDrop_0.1.0_amd64.AppImage --appimage-extract-and-run
```

AppImage does not automatically install a desktop shortcut. Delete the AppImage
to remove that copy. Moving or deleting its executable does not move/delete the
application profile. Do not disable WebKit sandboxing or platform protections to
work around an unsupported host.

## Windows installation and removal

Choose one format, not both. NSIS installs the application for the current user;
MSI uses Tauri's standard per-machine installation and can require administrator
approval. Application icons, publisher metadata, Start Menu integration, English
installer UI, and a stable WiX upgrade code are configured. Downgrades are disabled
within Tauri's supported installer paths. Switching between NSIS and MSI requires
removing the previous installation first; profile data is retained.

Both formats embed Microsoft's Evergreen **offline WebView2 installer**. Missing
WebView2 can therefore be installed without fetching its bootstrapper at install
time. This necessarily increases package size. Existing compatible system runtimes
are reused; the Microsoft prerequisite installer retains its own platform behavior.
No fixed browser runtime is frozen into InvarDrop. Application use remains local,
offline and account-free. Build-time downloads are separate from application use.
See [Tauri's Windows installer options](https://v2.tauri.app/distribute/windows-installer/).

Uninstall through Windows Settings or the installer. The reviewed NSIS template is
adapted to remove both its app-data deletion checkbox and deletion block. The
pinned upstream template is SHA-256 verified before transformation; a Tauri CLI
upgrade requires deliberate review and repinning. MSI uses the standard Tauri
component-based uninstall without custom profile-cleanup actions. No installer
removes received files, pairing/trust state, recovery checkpoints or keyring entries.
A full identity reset is separate maintenance, not an uninstall side effect.

**These testing installers are unsigned.** No signing certificate, private key or
password is stored in the repository or artifacts. SmartScreen or organizational
policy may block an unsigned installer. Do not bypass warnings, disable Defender,
or weaken policy; use an approved signing and validation process before wider
distribution. Native Windows installation, upgrade, uninstall, UAC, WebView2 setup
and GUI behavior have not been exercised on a physical Windows system or VM.

## Build and artifact inspection

```sh
npm ci
npm run check
npm test
npm run test:packaging
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo clippy --locked --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --locked --manifest-path src-tauri/Cargo.toml
npm run tauri build -- --ci -- --locked
```

Platform-specific Tauri configuration selects the two native formats. Build on the
corresponding OS; Windows packaging is not claimed to be cross-compiled locally.
FFmpeg is installed separately for conversion regression tests but **never bundled**.
The existing explicit local executable selection and prepared-file resume behavior
are unchanged. TLS 1.3, BLAKE3, identity, trust, consent and transfer protocols are
unchanged. The bundle identifier remains `io.github.invargervi.invardrop` so package
formats reuse the existing profile and credential binding.

The application-resource map contains only LICENSE and this packaging guide.
Installer audits inspect payload inventories, executable modes, desktop metadata,
license files, forbidden development/profile paths and private-key text markers.
Windows inspection reads MSI tables and extracts archives without running an
installer. A separate Windows filesystem regression covers archive filenames with
square brackets; hashing and signature checks use literal paths. In the audit, the embedded Microsoft runtime's Authenticode signature must validate.
Linux CI also exercises real dpkg install/remove on an ephemeral runner with
profile, received-file and recovery sentinels. These checks do not replace a
security audit or physical runtime testing.

CI uploads only installers, the license, documentation, payload audit reports and
SHA-256 manifests as test artifacts retained for seven days. No release publishing,
update service or new application permission is added. Generated package data stays under ignored `target` directories; Tauri may also
cache external build tools in the user cache. Before bundling, only the generated
AppImage AppDir and its `appimage_deb` intermediate tree are cleaned so removed metadata/resources cannot leak into a later
package. This never cleans an application profile or received directory.

Lockfiles, pinned Actions, and a version/hash-checked NSIS template make inputs
reviewable. The Rust stable toolchain, OS packages, WebView2 redistributable,
external AppImage tooling and timestamps can change. Bundled third-party runtimes retain their own licenses; the application license
does not relicense WebView2 or system libraries. Redistribution notices and
obligations require separate review before public release. Bit-for-bit reproducibility
and code-signing provenance are not claimed; every CI build records output hashes.

## Validation scope

Results apply to the feature-branch commit in
[GitHub Actions](https://github.com/InvarGervi123/InvarDrop/actions/workflows/ci.yml).
Local and hosted test counts, actual packaged startup checks, and any packaging
limitations are reported separately from the target-platform table above.

Local verification on CachyOS/Niri (2026-11-15): 136 Rust tests, 77 UI tests,
and four installer-template safety tests passed, together with TypeScript,
Rustfmt, strict Clippy, the native production build and both Linux bundle formats.
The local Debian package was approximately 4.1 MiB and AppImage 110 MiB; these
newer-host builds are not the canonical Ubuntu 24.04 artifacts.

The executable extracted from the inspected Debian package and the actual AppImage
runtime (`--appimage-extract-and-run`) were each tested in the native desktop.
Both used isolated Secret Service profiles and a diagnostic sender over real
mDNS/TLS. Checks covered full-code pairing, rejection without saving, explicit
receiver acceptance, verified ZIP receipt, separate extraction consent, nested/
empty-file extraction, malicious-path rejection, original-archive preservation,
and explicit trust revocation. Both exited normally; test credentials were removed.
No WebKit sandbox override was used. This is packaged-application runtime testing,
not a claim of a local dpkg installation on CachyOS or Windows installer execution.

The Ubuntu 24.04 CI AppImage from `c79fabe` was also downloaded, checked against
both the artifact digest and installer SHA-256 manifest, and exercised on the
native CachyOS desktop with the same GUI/TLS scenario. It exited normally without
a sandbox override. This additional check does not establish compatibility with
every Linux distribution or a physical second device.
