# Orchard

A fast, cross-platform iOS device manager written in Rust: the useful parts of
iTunes and iMazing, plus ipatool-style App Store downloads, in one tool.

## Goals

- Device management: discovery over USB and Wi-Fi, pairing, info, syslog, screenshots
- Files: AFC browsing, app containers, drag and drop transfer
- Apps: list, install, uninstall, back up, IPA download from the App Store, re-sign
- Backups: full and incremental, encrypted, browse and extract single files, restore
- Media: music, video, books and photos sync without the iTunes database lock-in
- Runs on Linux, macOS and Windows; CLI first, GUI on top of the same crates

## Layout

| Crate | Purpose |
|---|---|
| `orchard-core` | Shared types and errors |
| `orchard-device` | usbmuxd, lockdown, pairing, AFC, installation proxy |
| `orchard-backup` | mobilebackup2, Manifest.db parsing, restore |
| `orchard-ipa` | App Store auth and download, IPA inspection, signing |
| `orchard-library` | Media library and sync |
| `orchard-cli` | The `orchard` binary |
| `orchard-service` | Every app command behind one dispatcher, shared by the app and the server |
| `orchard-server` | The `orchard-server` binary: the commands over HTTP for web and iPad clients |
| `orchard-app` | Desktop app and iPad client (Tauri 2), frontend in `app/` (React, shadcn/ui) |

Device protocols come from [`idevice`](https://github.com/jkcoxson/idevice), pinned to an
exact version because its API changes on every 0.1.x release.

## App Store access

Apple now requires sign-in requests to carry a signature only Apple's own code can
produce. When [ipatool](https://github.com/majd/ipatool) is installed (on `PATH` or via
`ORCHARD_IPATOOL`), Orchard uses it for sign-in, purchase and download, then installs the
IPA itself over USB or Wi-Fi. Without it Orchard falls back to its native client, which
Apple may reject at sign-in. Search and lookup always work natively.

## Sideloading

`orchard ipa resign` (and the install sheet in the app) re-signs an IPA with your own
development certificate (`.p12`) and provisioning profile, then installs it. App Store
IPAs are FairPlay encrypted and cannot be re-signed; Orchard refuses them up front.

## Paid apps

ipatool only acquires free apps. Orchard's built-in client also buys paid ones:
`orchard store login --native` then `orchard store buy <app>` (or "Buy" in the app). Apple
answers with a price confirmation that is shown before anything is charged. This needs the
built-in sign-in, which Apple may reject (see App Store access).

## Media

Orchard does not add to the built-in Music, TV, Books or Ringtones libraries. That sync
runs over `com.apple.atc`, driven by Apple's closed `AirTrafficHost` library, and its
handshake uses Apple's Grappa signing (`AirFairSyncGrappaCreate`, `AirFairSyncCalcSig`, as
documented by [iOSAuthRuntime](https://github.com/iOS-Auth/iOSAuthRuntime)). There is no
open implementation: libimobiledevice
([#735](https://github.com/libimobiledevice/libimobiledevice/issues/735)) and
pymobiledevice3 ([#297](https://github.com/doronz88/pymobiledevice3/issues/297)) never
added it, and the tools that do it load Apple's binaries on Windows or macOS. The old
AFC-only tricks (`Books.plist`, `iTunes_Control`) predate the `MediaLibrary.sqlitedb`
library and are not known to work on current iOS.

Instead, `orchard media send` copies music, video and books into apps with file sharing
(VLC, Infuse, Documents, PDF Expert and others), and photos are exported from the camera
roll.

## Wi-Fi

Devices show up over Wi-Fi through usbmuxd when Wi-Fi sync is on (`orchard wifi on`). On
Linux, where usbmuxd has no network support, Orchard connects directly using the pairing
record it saved when pairing over USB, finding the device via Bonjour or `--ip`.

## Server and iPad

`orchard-server` runs Orchard on a machine the iPhone is plugged into (or on its network)
and serves the same UI to browsers and the iPad app, which are then only screens:

```sh
cd app && npm run build && cd ..
cargo run --release -p orchard-server -- --ui app/dist
```

It prints a connect code; enter it in the browser (`http://<server>:7420`) or the iPad
app once, and the client keeps a token. `orchard-server code` prints a new code for the
running server, `orchard-server revoke` signs every client out. A connected client can do
everything the desktop app can, with the server's file access, so the server only answers
the local network, loopback and Tailscale range unless started with `--allow-public` (use
that only behind HTTPS). Files a client picks are uploaded to the server; results such as
exported photos or downloaded IPAs come back as downloads.

USB means plugged into the server: iPadOS gives apps no access to a phone plugged into
the iPad. App Store sign-in through ipatool works on the server like on a desktop.

The iPad app is the Tauri app built for iOS, which needs a Mac with Xcode:

```sh
cd app && npm run tauri ios init   # once, creates crates/orchard-app/gen/apple
npm run tauri ios dev              # or: npm run tauri ios build
```

It cannot go through the App Store; sign it with your Apple ID (free accounts expire after
7 days) or install it with Orchard itself. `Info.ios.plist` allows plain HTTP to local
addresses, which the server uses; downloads open in Safari, which saves them to Files.

## Prior art to evaluate before writing protocol code

- [`idevice`](https://github.com/jkcoxson/idevice): pure Rust lockdown/usbmuxd/AFC services
- [libimobiledevice](https://github.com/libimobiledevice): the C reference implementation
- [pymobiledevice3](https://github.com/doronz88/pymobiledevice3): most complete protocol coverage
- [ipatool](https://github.com/majd/ipatool): App Store auth and IPA download flow
- [`apple-codesign`](https://github.com/indygreg/apple-platform-rs): pure Rust code signing
- [`plist`](https://crates.io/crates/plist): property list parsing

## Development

```sh
cargo build
cargo run -p orchard-cli -- --help
cargo run -p orchard-cli -- store search "notes"
cargo clippy --all-targets
cargo test

# desktop app
cd app && npm install --registry=https://registry.npmjs.org/ && npm run tauri dev
# UI only, against a mock backend, in a browser
cd app && npm run dev
```

Data (pairing records, App Store account, backups) lives in the platform data directory,
for example `~/.local/share/orchard`; set `ORCHARD_DATA_DIR` to override it.

On Linux, device access needs `usbmuxd` running (or Orchard's own mux, later).

## License

MIT, see [LICENSE](LICENSE).
