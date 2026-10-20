# Orchard command contract

Every frontend call goes through `app/src/lib/api.ts` (`call(name, args)`). The
desktop app passes it to `Service::dispatch` through its one Tauri command,
`dispatch`; `orchard-server` through `POST /api/call/{name}`. Each command below is
a function in `src/commands/*.rs`, listed in the table in `src/dispatch.rs`;
arguments are sent in camelCase and map to the snake_case Rust parameters. DTOs live
in `src/dto.rs` and mirror the TS types field for field (serde camelCase, `Option`
for nullable). A test fails when `api.ts` and the dispatch table disagree.

`app_info` and `open_data_dir` are answered by the desktop app itself; the server
answers `app_info` and adds `new_export_dir` and `download_ticket` for remote
clients (see `crates/orchard-server/src/lib.rs`).

Errors are `CommandError { kind, message }` (`src/error.rs`), built from
`orchard_core::Error` so `kind` equals `Error::kind()`. The app adds one kind of its
own, `cancelled`, for tasks stopped through `cancel_task`.

Every command is wired to the Orchard crates. Data lives in
`orchard_core::data_dir()` (shared with the CLI, honors `ORCHARD_DATA_DIR`):
pairing records, `account.json`, `settings.json`, and new backups under `backups/`.

## Events

| Event | Payload | Emitted by |
|---|---|---|
| `orchard://task` | `TaskEvent { taskId, stage, done, total: u64 \| null, status: "running" \| "done" \| "error", message: string \| null }` | `events::run_task` for every task command |
| `orchard://devices` | `DeviceEvent { kind: "attached" \| "detached" \| "updated", udid }` | the usbmuxd watcher spawned at startup, plus pair, unpair and add Wi-Fi |
| `orchard://syslog` | `SyslogLine { udid, timestamp, process, pid: u32 \| null, level, message }` | the stream task spawned by `start_syslog` |
| `orchard://syslog-status` | `SyslogEnded { udid, message: string \| null }` | the stream task, when the device ends the stream (not on stop) |

Task commands take a client generated `taskId`, validated as `[A-Za-z0-9-]{1,64}`
when the arguments are deserialized (`events::TaskId`), and run their work through
`events::run_task`, which spawns it on the runtime, registers its abort handle
for `cancel_task`, forwards `Progress` callbacks as `running` events, and emits a
final `done` or `error` event. Blocking work (backup extraction) also polls
`Reporter::check()`. Stage units: `upload`, `download`, `export`, `send`, `backup`,
`restore` are bytes (percent when `total` is 100); `install` is percent;
`extract` is a file count; `license` and `prepare` have no total.

## Commands

| Command | Args | Returns | Notes | Backed by |
|---|---|---|---|---|
| `list_devices` | none | `Device[]` | one entry per UDID, USB preferred; devices that do not answer are listed as unpaired; without usbmuxd only added Wi-Fi devices | `orchard_device::list_devices`, `DeviceConnection::info` |
| `device_info` | `udid: string` | `DeviceInfo` | `model` from `product_name`, falling back to the product type | `DeviceConnection::info` |
| `add_wifi_device` | `address: string` (IP or host) | `Device` | tries every stored pairing record at that address | `DeviceConnection::connect_wifi` |
| `pair_device` | `udid` | `null` | waits up to 60 s for Trust; emits `updated` | `DeviceConnection::pair` |
| `unpair_device` | `udid` | `null` | emits `updated` | `DeviceConnection::unpair` |
| `set_wifi_sync` | `udid, enabled: bool` | `null` | | `set_wifi_connections` |
| `take_screenshot` | `udid` | `string` (PNG base64, no prefix) | | `screenshot` |
| `save_screenshot` | `path, png: string` | `null` | local write | |
| `restart_device` | `udid` | `null` | | `restart` |
| `shutdown_device` | `udid` | `null` | | `shutdown` |
| `list_apps` | `udid` | `InstalledApp[]` | user and system apps; internal and hidden ones dropped | `list_apps(true)` |
| `app_icon` | `udid, bundleId` | `string \| null` (PNG data URL) | errors become `null` | `app_icon` |
| `uninstall_app` | `udid, bundleId` | `null` | | `uninstall_app` |
| `inspect_ipa` | `path` | `IpaInfo` | icon as PNG data URL | `orchard_ipa::inspect` |
| `install_ipa` | `taskId, udid, path` | `null` | task: `upload`, `install` | `install_ipa` |
| `store_account` | none | `StoreAccount \| null` | `backend: "ipatool" \| "native"`; country from the account or the sign-in choice | `AppStore::account` |
| `store_sign_in` | `email, password, code: string \| null, country, native: bool` | `StoreAccount` | `authCodeRequired`, `authBadCredentials`; country saved in settings; `native` forces the built-in client (needed for paid apps) | `AppStore::sign_in` / `sign_in_native` |
| `store_sign_out` | none | `null` | | `AppStore::sign_out` |
| `store_search` | `term, country, limit: u32` | `StoreApp[]` | `description` is the release notes | `AppStore::search` |
| `store_versions` | `appId: u64` | `StoreVersion[]` | newest first; `version` is always `null` | `AppStore::list_versions` |
| `store_purchase` | `appId, price, confirmation: PurchaseConfirmation \| null` | `PurchaseResult` | free apps: `purchased`; paid apps: first `needsConfirmation` with Apple's message, then call again with it. The UI calls it after `licenseRequired`, then retries | `AppStore::purchase` / `purchase_paid` |
| `store_download` | `taskId, appId, versionId: string \| null, destDir` | `string` (IPA path) | task: `license`, `download` | `AppStore::download` |
| `store_install` | `taskId, udid, appId, versionId` | `null` | task: `license`, `download`, `upload`, `install`; the temp dir is a `TempDir` owned by the task, removed on success, error and cancel; USB and Wi-Fi | `AppStore::download` + `install_ipa` |
| `list_files` | `udid, source: FileSource, path` | `FileEntry[]` | `modified` in ms | `DeviceFiles::list_dir` |
| `upload_files` | `taskId, udid, source, destDir, localPaths: string[]` | `null` | task: `upload`, folders recursive; local symlinks are skipped and counted in the done message | `DeviceFiles::{mkdir_all, upload}` |
| `download_files` | `taskId, udid, source, remotePaths: string[], localDir` | `null` | task: `download`, folders recursive; only regular files, links counted as skipped; never replaces a local item, an existing name becomes `name (n).ext` | `DeviceFiles::{stat, list_dir, download}` |
| `create_folder` | `udid, source, path` | `null` | | `mkdir_all` |
| `rename_path` | `udid, source, from, to` | `null` | | `rename` |
| `delete_paths` | `udid, source, paths: string[]` | `null` | recursive | `remove(path, true)` |
| `list_backups` | none | `BackupSummary[]` | `id` is the backup folder path; Orchard dir plus iTunes / Finder dirs | `default_backup_dirs`, `list_backups` |
| `create_backup` | `taskId, udid, mode: "full" \| "incremental"` | `null` | task: `prepare`, `backup`; into `<data_dir>/backups` | `orchard_backup::create_backup` |
| `open_backup` | `backupId, password: string \| null` | `BackupDomain[]` | `backupPassword`; kept open in `BackupState`; `size` is 0 | `Backup::open`, `domains` |
| `list_backup_files` | `backupId, domain, query: string \| null` | `BackupFile[]` | files only, at most 5000 | `Backup::files` |
| `extract_backup_files` | `taskId, backupId, fileIds: string[], destDir` | `null` | task: `extract`; writes `<destDir>/<domain>/<path>`, rejects escaping paths | `Backup::extract_file` |
| `restore_backup` | `taskId, udid, backupId, password` | `null` | task: `prepare`, `restore`; device reboots | `orchard_backup::restore_backup` |
| `list_photos` | `udid` | `PhotoItem[]` | | `orchard_library::list_camera_roll` |
| `export_photos` | `taskId, udid, paths: string[], destDir` | `null` | task: `export`; `<destDir>/<album>/<name>` | `orchard_library::export_photos` |
| `load_signing_identity` | `path, password` | `SigningIdentityInfo { id, path, commonName, teamId, expiresAt }` | the identity stays in `SigningState` under `id`; the password is used once and never stored; the path is remembered in settings | `SigningIdentity::from_p12` |
| `load_provisioning_profile` | `path` | `ProfileInfo { id, path, name, teamId, teamName, applicationIdentifier, bundleIdPattern, wildcard, devices, provisionsAllDevices, expiresAt }` | kept in `SigningState`; path remembered | `ProvisioningProfile::parse` |
| `check_resign` | `ipaPath, identityId, profileId, udid: string \| null, bundleId: string \| null` | `ResignIssue[]` (tagged by `kind`; times in ms) | `encrypted` is blocking in the UI | `orchard_ipa::check_resign` |
| `resign_ipa` | `taskId, ipaPath, identityId, profileId, bundleId, displayName, destPath` | `ResignReport { bundleId, teamId, bundles, removed, outputSize, path }` | task: `extract`, `sign`, `package` (files) | `orchard_ipa::resign_ipa` |
| `resign_and_install` | `taskId, udid, ipaPath, identityId, profileId, bundleId, displayName` | `null` | one task: `extract`, `sign`, `package`, `upload`, `install`; temp dir dropped on cancel | `resign_ipa` + `install_ipa` |
| `media_targets` | `udid` | `MediaTarget[] { bundleId, name, accepts: ("audio" \| "video" \| "book" \| "other")[], recognized }` | user apps with file sharing; `recognized: false` means the "any file" fallback | `orchard_library::media_targets` |
| `list_media` | `udid, bundleId, dir` (`/Documents` or below) | `MediaFile[]` (`FileEntry` plus `category`, null for folders) | | `orchard_library::list_media` on `FileLocation::AppDocuments` |
| `send_media` | `taskId, udid, bundleId, localPaths: string[], subdir: string \| null` | `SendSummary { uploaded, skipped, bytes, renamed: string[], skippedSymlinks, skippedInvalid }` | task: `send` (bytes); folders recursive, links skipped, same name and size skipped, clashes renamed `name (n)` | `orchard_library::send_media` |
| `start_syslog` | `udid` | `null` | replaces any running stream; runs until `stop_syslog`; a stop during connect wins (generation counter) | `DeviceConnection::syslog` |
| `stop_syslog` | none | `null` | | |
| `cancel_task` | `taskId` | `null` | aborts the task | |
| `get_settings` | none | `Settings { downloadDir, dataDir, storeBackend, storeSignInNote, signingP12Path, signingProfilePath }` (`storeSignInNote` is `ARGV_EXPOSURE_NOTE` when the backend is ipatool) | | |
| `set_download_dir` | `path: string \| null` | `Settings` | persisted to `<dataDir>/settings.json` | |
| `app_info` | none | `AppInfo { version, platform, arch }` | | |

Every name from a device or a backup that becomes a local path (download
names, photo album and file names, backup domains and relative paths) goes
through `paths::safe_name` / `paths::safe_join`, which reject empty names, `.`,
`..`, separators, `:` and NUL.

`FileSource` is `{ kind: "media" } | { kind: "appDocuments", bundleId } | { kind: "appContainer", bundleId }`.

## Gaps in the crates

- `BackupDomain.size`: `orchard_backup::DomainInfo` has no byte size, so it is 0 (the UI hides it).
- `StoreVersion.version`: `AppStore::list_versions` returns ids only, so display versions are `null`.
- Backup domain labels for apps are bundle ids; the backup crate has no app names.

## Managed state

- `events::TaskRegistry`: abort handles and cancel flags of running tasks.
- `state::DeviceState` (in an `Arc`): pairing store, per-UDID connection cache (dropped on detach), added Wi-Fi addresses, device summaries.
- `state::StoreState`: the `AppStore` facade.
- `state::BackupState`: opened backups.
- `state::SigningState`: loaded signing identities and profiles, by id.
- `commands::console::SyslogState`: the single running syslog stream.
- `commands::app::SettingsStore`: `settings.json`.

## Running

From `app/`: `npm run tauri -- dev` (starts Vite on 1420, then the window) or
`npm run tauri -- build`. Outside Tauri, `npm run dev` serves the UI with the
mock backend (`app/src/lib/mock.ts`).
