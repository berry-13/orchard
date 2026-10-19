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

A GUI crate (`orchard-app`) will be added once the CLI covers the core flows.

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
cargo run -p orchard-cli -- devices
cargo clippy --all-targets
cargo test
```

On Linux, device access needs `usbmuxd` running (or Orchard's own mux, later).

## License

MIT, see [LICENSE](LICENSE).
