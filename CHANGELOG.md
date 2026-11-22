# Changelog

## 0.1.1-beta.2 — release candidate

- Choose files and a nearby device in either order, then send. The app opens an authenticated connection automatically and reuses trust from previous pairing.
- Nearby devices show readable labels and device icons. Android and desktop use a high-contrast, OLED-first interface.
- The local network is preferred for new connections. Compatible Linux and Android devices can use experimental Bluetooth discovery and transfer when the local network is unavailable. Windows uses LAN.
- Incoming requests and transfer results appear in Activity. A verified transfer stays completed when its connection closes.
- Android imports files through the system picker and publishes verified receipts to Downloads. Android 17 prompts for local network access when required. The app excludes private files, trust and recovery state from Android backups and device migration.
- Settings contains trusted devices, transfer recovery, Bluetooth controls, diagnostics, and optional desktop audio conversion.
- Discovery uses shorter Bluetooth scan windows and fewer idle checks. Power consumption has not been measured on physical devices.

First-time pairing still requires comparing the entire code on both screens. The receiver approves each transfer, including a resumed transfer. See the [installation guide and known limitations](docs/release-candidate.md).

## 0.1.0-beta.1 — 2026-11-15

The first public desktop prerelease introduced private LAN sharing for Linux and Windows, with verified file and folder transfers, explicit consent, and recovery checkpoints. See the [published prerelease](https://github.com/InvarGervi123/InvarDrop/releases/tag/v0.1.0-beta.1).
