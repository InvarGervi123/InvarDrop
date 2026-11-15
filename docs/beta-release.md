# InvarDrop v0.1.0-beta.1

**Use it. It works. Forget it.** Minimal, private LAN file and folder sharing,
without accounts, cloud storage, telemetry, autostart services, or an updater.
This first public Beta is a prerelease for testing, not a production certification.

## Download and verify

Download the four x86_64 installers and `SHA256SUMS.txt` from the
[GitHub prerelease](https://github.com/InvarGervi123/InvarDrop/releases/tag/v0.1.0-beta.1).
Choose one package format for your platform. Verify the file before opening it:

```sh
# Linux: run in a directory containing the four installers and manifest.
sha256sum --check SHA256SUMS.txt
```

If downloading just one installer, compare `sha256sum <filename>` with its line
in the manifest. On Windows, use PowerShell:

```powershell
Get-FileHash -LiteralPath '.\<downloaded-installer-filename>' -Algorithm SHA256
```

Compare the full hash with `SHA256SUMS.txt`. Checksums detect corruption; they do
not replace code signing. Installers are unsigned. Do not bypass SmartScreen,
Defender, organizational policy, or other platform security warnings.

## Installation

- **Linux AppImage:** Ubuntu 24.04/glibc 2.39 or newer compatible x86_64 system,
  GTK 3/WebKitGTK 4.1 desktop environment, session D-Bus, unlocked Secret Service,
  and a working native file portal. Run
  `chmod u+x InvarDrop_0.1.0-beta.1_amd64.AppImage`, then open the AppImage.
  If FUSE is unavailable, use `--appimage-extract-and-run`.
- **Linux Debian package:** on a compatible Debian/Ubuntu system, run
  `sudo apt install ./InvarDrop_0.1.0-beta.1_amd64.deb`, then launch InvarDrop
  from the desktop menu or run `invardrop`. Missing system dependencies may need
  internet access during installation. Remove with `sudo apt remove invar-drop`.
- **Windows NSIS `.exe`:** current-user installation for the Windows 10/11
  x86_64 desktop target. **MSI `.msi`:** standard per-machine installation, which
  can require administrator approval. Choose one format, not both. Both embed
  Microsoft's offline WebView2 prerequisite installer; no FFmpeg is bundled.
  Uninstall through Windows Settings. Windows installer/GUI behavior remains
  unverified in a normal user environment; see the limitations below.

Removing the application preserves received files, identity/trust, recovery
data and keyring entries. Removing an AppImage removes only that executable.
Do not delete your profile to uninstall. Format changes and future upgrades need
separate validation. See [packaging details](packaging.md) for dependencies,
stable identifiers, MSI numeric version mapping and data-retention behavior.

## First transfer

1. Open InvarDrop on both computers on the same local network. Device discovery
   names are unverified. Do not disable firewalls to make discovery work.
2. Connect, compare **all** pairing-code groups over an independent channel, and
   explicitly approve pairing on both devices.
3. Select files or a folder. The receiver checks the actual name, file count and
   size, confirms consent, and accepts. Files are published only after BLAKE3
   verification; existing destinations are never overwritten.
4. Use Cancel to stop an operation. Interrupted transfers expose Resume/Discard;
   resume requires sender action and fresh receiver approval.

OGG conversion is opt-in and needs a separately selected local FFmpeg executable.
Keeping originals is the default. ZIP extraction requires separate approval after
verified receipt; failure retains the archive. Extracted content is never run.

## Validation and limitations

Native Ubuntu 24.04 and Windows Server 2022 CI build and audit packages, verify
checksums, and execute independent-device loopback TLS/restart tests. Local
CachyOS testing includes mDNS and a packaged Linux GUI workflow. These checks
do **not** establish full cross-platform compatibility or production readiness.

Still unverified: physical Linux-to-Windows transfers; Windows GUI on a real
user machine; Windows installation/uninstallation in a real environment;
Windows firewall behavior; and Windows Credential Manager in normal application
usage. Process tests substitute the keyring, and CI runs are not physical LAN,
power-loss, or all-filesystem durability tests. Packages are unsigned. ARM,
macOS, mobile platforms, Bluetooth, QR transfers, resume across incompatible
protocols, and additional UI languages are not supported by this Beta.

Read the [security threat model](security.md), [resume limitations](resumable-transfers.md),
[processing limits](optional-file-processing.md), and [test coverage](cross-device-testing.md).
This project has not undergone an independent security audit. Runtime use stays
local and account-free; build/install prerequisites and manually selected tools
are separate. The release notes identify the exact source commit and native CI
run; attached provenance and manifests identify the audited binary assets.
