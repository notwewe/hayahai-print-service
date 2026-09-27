# HayahAI Print Service

HayahAI Print Service is the workstation agent for managed receipt printing in HayahAI TMS. It accepts signed jobs from one paired shipping-line Client API and submits them to an installed operating-system printer queue.

It supports Windows x64 and macOS Intel/Apple Silicon with ESC/POS-compatible 58mm and 80mm printers. USB and network printers work after they are installed as normal OS queues.

## How it works

- The service has no inbound port and makes outbound HTTPS requests only.
- Pairing creates an Ed25519 key on the workstation. The private key stays in Windows Credential Manager or macOS Keychain.
- Requests include a timestamp and strictly increasing signed counter. The server rejects expired, replayed, changed, or revoked requests.
- Print artifacts are verified by SHA-256 before submission. Windows sends exact ESC/POS bytes through Winspool. macOS sends the matching PDF through the installed CUPS raster driver with the managed paper and cutter settings. Receipt content and print bytes are excluded from logs.
- One claimed job is attempted once. A partial write or missing acknowledgement is reported as `unknown` and is never automatically retried.

## Install and pair

1. Install the printer and its driver in Windows or macOS.
2. Download the latest installer from the GitHub Releases page.
3. Approve the first installation through Windows **Run anyway** or macOS **Privacy & Security**.
4. In TMS, open **Preferences → Printing Presets**, generate a pairing link, and open it on this workstation.
5. Select the reported printer queue, bind a preset, print the calibration sheet, and confirm its measurements.

After pairing, the window hides and the service remains available from the tray/menu bar. It starts at login and checks for signed updates at startup and daily. Updates wait while a print job is active.

To replace an expired or revoked connection, open **Status and pairing** from the tray menu, select **Pair a new link**, and paste a newly generated TMS pairing link. The existing connection remains saved unless the replacement pairing succeeds.

## Development

Prerequisites: Node.js 22, pnpm 10, stable Rust, and the [Tauri 2 platform prerequisites](https://v2.tauri.app/start/prerequisites/).

```bash
pnpm install
pnpm tauri dev
pnpm check
```

Development pairing permits `http://localhost`. Release builds require an HTTPS Client API URL.

## Release

The release workflow builds a Windows x64 NSIS installer and a universal macOS DMG. Configure these GitHub Actions secrets before tagging a release:

- `TAURI_SIGNING_PRIVATE_KEY`
- `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`

The public updater key is compiled into the application. Keep the private key only in GitHub Actions secrets.
