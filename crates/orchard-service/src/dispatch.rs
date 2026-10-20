//! The command table: one entry per command of the `Commands` type in
//! `app/src/lib/api.ts`, with the same names and camelCase arguments.

use std::path::PathBuf;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::Service;
use crate::commands;
use crate::dto::{BackupMode, FileSource};
use crate::error::{CmdResult, CommandError};
use crate::events::TaskId;

/// Parses the arguments of one command into a struct with the given fields.
macro_rules! args {
    ($a:ident, $args:ident, { $($field:ident : $ty:ty),* $(,)? }) => {
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Args { $($field: $ty),* }
        let $a: Args = parse($args)?;
    };
}

fn parse<T: DeserializeOwned>(args: Value) -> CmdResult<T> {
    // A command without arguments may be called with `null`.
    let args = if args.is_null() {
        Value::Object(Default::default())
    } else {
        args
    };
    serde_json::from_value(args)
        .map_err(|e| CommandError::new("invalidInput", format!("bad arguments: {e}")))
}

fn reply<T: Serialize>(result: CmdResult<T>) -> CmdResult<Value> {
    serde_json::to_value(result?)
        .map_err(|e| CommandError::new("unknown", format!("bad reply: {e}")))
}

impl Service {
    /// Runs `command` with its JSON arguments and returns its JSON result.
    pub async fn dispatch(&self, command: &str, args: Value) -> CmdResult<Value> {
        match command {
            "list_devices" => reply(commands::devices::list_devices(&self.devices).await),
            "device_info" => {
                args!(a, args, { udid: String });
                reply(commands::devices::device_info(&self.devices, a.udid).await)
            }
            "add_wifi_device" => {
                args!(a, args, { address: String });
                reply(
                    commands::devices::add_wifi_device(
                        self.events.clone(),
                        &self.devices,
                        a.address,
                    )
                    .await,
                )
            }
            "pair_device" => {
                args!(a, args, { udid: String });
                reply(
                    commands::devices::pair_device(self.events.clone(), &self.devices, a.udid)
                        .await,
                )
            }
            "unpair_device" => {
                args!(a, args, { udid: String });
                reply(
                    commands::devices::unpair_device(self.events.clone(), &self.devices, a.udid)
                        .await,
                )
            }
            "set_wifi_sync" => {
                args!(a, args, { udid: String, enabled: bool });
                reply(commands::devices::set_wifi_sync(&self.devices, a.udid, a.enabled).await)
            }
            "take_screenshot" => {
                args!(a, args, { udid: String });
                reply(commands::devices::take_screenshot(&self.devices, a.udid).await)
            }
            "save_screenshot" => {
                args!(a, args, { path: PathBuf, png: String });
                reply(commands::devices::save_screenshot(a.path, a.png).await)
            }
            "restart_device" => {
                args!(a, args, { udid: String });
                reply(commands::devices::restart_device(&self.devices, a.udid).await)
            }
            "shutdown_device" => {
                args!(a, args, { udid: String });
                reply(commands::devices::shutdown_device(&self.devices, a.udid).await)
            }
            "list_apps" => {
                args!(a, args, { udid: String });
                reply(commands::apps::list_apps(&self.devices, a.udid).await)
            }
            "app_icon" => {
                args!(a, args, { udid: String, bundle_id: String });
                reply(commands::apps::app_icon(&self.devices, a.udid, a.bundle_id).await)
            }
            "uninstall_app" => {
                args!(a, args, { udid: String, bundle_id: String });
                reply(commands::apps::uninstall_app(&self.devices, a.udid, a.bundle_id).await)
            }
            "inspect_ipa" => {
                args!(a, args, { path: String });
                reply(commands::apps::inspect_ipa(a.path).await)
            }
            "install_ipa" => {
                args!(a, args, { task_id: TaskId, udid: String, path: String });
                reply(
                    commands::apps::install_ipa(
                        self.events.clone(),
                        &self.tasks,
                        &self.devices,
                        a.task_id,
                        a.udid,
                        a.path,
                    )
                    .await,
                )
            }
            "store_account" => {
                reply(commands::store::store_account(&self.store, &self.settings).await)
            }
            "store_sign_in" => {
                args!(a, args, { email: String, password: String, code: Option<String>, country: String, native: Option<bool> });
                reply(
                    commands::store::store_sign_in(
                        &self.store,
                        &self.settings,
                        a.email,
                        a.password,
                        a.code,
                        a.country,
                        a.native,
                    )
                    .await,
                )
            }
            "store_sign_out" => reply(commands::store::store_sign_out(&self.store).await),
            "store_search" => {
                args!(a, args, { term: String, country: String, limit: u32 });
                reply(commands::store::store_search(&self.store, a.term, a.country, a.limit).await)
            }
            "store_versions" => {
                args!(a, args, { app_id: u64 });
                reply(commands::store::store_versions(&self.store, a.app_id).await)
            }
            "store_purchase" => {
                args!(a, args, { app_id: u64, price: Option<f64>, confirmation: Option<orchard_ipa::PurchaseConfirmation> });
                reply(
                    commands::store::store_purchase(&self.store, a.app_id, a.price, a.confirmation)
                        .await,
                )
            }
            "store_download" => {
                args!(a, args, { task_id: TaskId, app_id: u64, version_id: Option<String>, dest_dir: String });
                reply(
                    commands::store::store_download(
                        self.events.clone(),
                        &self.tasks,
                        &self.store,
                        a.task_id,
                        a.app_id,
                        a.version_id,
                        a.dest_dir,
                    )
                    .await,
                )
            }
            "store_install" => {
                args!(a, args, { task_id: TaskId, udid: String, app_id: u64, version_id: Option<String> });
                reply(
                    commands::store::store_install(
                        self.events.clone(),
                        &self.tasks,
                        &self.store,
                        &self.devices,
                        a.task_id,
                        a.udid,
                        a.app_id,
                        a.version_id,
                    )
                    .await,
                )
            }
            "list_files" => {
                args!(a, args, { udid: String, source: FileSource, path: String });
                reply(commands::files::list_files(&self.devices, a.udid, a.source, a.path).await)
            }
            "upload_files" => {
                args!(a, args, { task_id: TaskId, udid: String, source: FileSource, dest_dir: String, local_paths: Vec<String> });
                reply(
                    commands::files::upload_files(
                        self.events.clone(),
                        &self.tasks,
                        &self.devices,
                        a.task_id,
                        a.udid,
                        a.source,
                        a.dest_dir,
                        a.local_paths,
                    )
                    .await,
                )
            }
            "download_files" => {
                args!(a, args, { task_id: TaskId, udid: String, source: FileSource, remote_paths: Vec<String>, local_dir: String });
                reply(
                    commands::files::download_files(
                        self.events.clone(),
                        &self.tasks,
                        &self.devices,
                        a.task_id,
                        a.udid,
                        a.source,
                        a.remote_paths,
                        a.local_dir,
                    )
                    .await,
                )
            }
            "create_folder" => {
                args!(a, args, { udid: String, source: FileSource, path: String });
                reply(commands::files::create_folder(&self.devices, a.udid, a.source, a.path).await)
            }
            "rename_path" => {
                args!(a, args, { udid: String, source: FileSource, from: String, to: String });
                reply(
                    commands::files::rename_path(&self.devices, a.udid, a.source, a.from, a.to)
                        .await,
                )
            }
            "delete_paths" => {
                args!(a, args, { udid: String, source: FileSource, paths: Vec<String> });
                reply(commands::files::delete_paths(&self.devices, a.udid, a.source, a.paths).await)
            }
            "list_backups" => reply(commands::backups::list_backups(&self.devices).await),
            "create_backup" => {
                args!(a, args, { task_id: TaskId, udid: String, mode: BackupMode });
                reply(
                    commands::backups::create_backup(
                        self.events.clone(),
                        &self.tasks,
                        &self.devices,
                        a.task_id,
                        a.udid,
                        a.mode,
                    )
                    .await,
                )
            }
            "open_backup" => {
                args!(a, args, { backup_id: String, password: Option<String> });
                reply(
                    commands::backups::open_backup(
                        &self.devices,
                        &self.backups,
                        a.backup_id,
                        a.password,
                    )
                    .await,
                )
            }
            "list_backup_files" => {
                args!(a, args, { backup_id: String, domain: String, query: Option<String> });
                reply(
                    commands::backups::list_backup_files(
                        &self.backups,
                        a.backup_id,
                        a.domain,
                        a.query,
                    )
                    .await,
                )
            }
            "extract_backup_files" => {
                args!(a, args, { task_id: TaskId, backup_id: String, file_ids: Vec<String>, dest_dir: String });
                reply(
                    commands::backups::extract_backup_files(
                        self.events.clone(),
                        &self.tasks,
                        &self.backups,
                        a.task_id,
                        a.backup_id,
                        a.file_ids,
                        a.dest_dir,
                    )
                    .await,
                )
            }
            "restore_backup" => {
                args!(a, args, { task_id: TaskId, udid: String, backup_id: String, password: Option<String> });
                reply(
                    commands::backups::restore_backup(
                        self.events.clone(),
                        &self.tasks,
                        &self.devices,
                        a.task_id,
                        a.udid,
                        a.backup_id,
                        a.password,
                    )
                    .await,
                )
            }
            "list_photos" => {
                args!(a, args, { udid: String });
                reply(commands::photos::list_photos(&self.devices, a.udid).await)
            }
            "export_photos" => {
                args!(a, args, { task_id: TaskId, udid: String, paths: Vec<String>, dest_dir: String });
                reply(
                    commands::photos::export_photos(
                        self.events.clone(),
                        &self.tasks,
                        &self.devices,
                        a.task_id,
                        a.udid,
                        a.paths,
                        a.dest_dir,
                    )
                    .await,
                )
            }
            "media_targets" => {
                args!(a, args, { udid: String });
                reply(commands::media::media_targets(&self.devices, a.udid).await)
            }
            "list_media" => {
                args!(a, args, { udid: String, bundle_id: String, dir: String });
                reply(commands::media::list_media(&self.devices, a.udid, a.bundle_id, a.dir).await)
            }
            "send_media" => {
                args!(a, args, { task_id: TaskId, udid: String, bundle_id: String, local_paths: Vec<String>, subdir: Option<String> });
                reply(
                    commands::media::send_media(
                        self.events.clone(),
                        &self.tasks,
                        &self.devices,
                        a.task_id,
                        a.udid,
                        a.bundle_id,
                        a.local_paths,
                        a.subdir,
                    )
                    .await,
                )
            }
            "load_signing_identity" => {
                args!(a, args, { path: String, password: String });
                reply(
                    commands::sign::load_signing_identity(
                        &self.signing,
                        &self.settings,
                        a.path,
                        a.password,
                    )
                    .await,
                )
            }
            "load_provisioning_profile" => {
                args!(a, args, { path: String });
                reply(
                    commands::sign::load_provisioning_profile(
                        &self.signing,
                        &self.settings,
                        a.path,
                    )
                    .await,
                )
            }
            "check_resign" => {
                args!(a, args, { ipa_path: String, identity_id: String, profile_id: String, udid: Option<String>, bundle_id: Option<String> });
                reply(
                    commands::sign::check_resign(
                        &self.signing,
                        a.ipa_path,
                        a.identity_id,
                        a.profile_id,
                        a.udid,
                        a.bundle_id,
                    )
                    .await,
                )
            }
            "resign_ipa" => {
                args!(a, args, { task_id: TaskId, ipa_path: String, identity_id: String, profile_id: String, bundle_id: Option<String>, display_name: Option<String>, dest_path: String });
                reply(
                    commands::sign::resign_ipa(
                        self.events.clone(),
                        &self.tasks,
                        &self.signing,
                        a.task_id,
                        a.ipa_path,
                        a.identity_id,
                        a.profile_id,
                        a.bundle_id,
                        a.display_name,
                        a.dest_path,
                    )
                    .await,
                )
            }
            "resign_and_install" => {
                args!(a, args, { task_id: TaskId, udid: String, ipa_path: String, identity_id: String, profile_id: String, bundle_id: Option<String>, display_name: Option<String> });
                reply(
                    commands::sign::resign_and_install(
                        self.events.clone(),
                        &self.tasks,
                        &self.signing,
                        &self.devices,
                        a.task_id,
                        a.udid,
                        a.ipa_path,
                        a.identity_id,
                        a.profile_id,
                        a.bundle_id,
                        a.display_name,
                    )
                    .await,
                )
            }
            "start_syslog" => {
                args!(a, args, { udid: String });
                reply(
                    commands::console::start_syslog(
                        self.events.clone(),
                        &self.syslog,
                        &self.devices,
                        a.udid,
                    )
                    .await,
                )
            }
            "stop_syslog" => reply(commands::console::stop_syslog(&self.syslog).await),
            "cancel_task" => {
                args!(a, args, { task_id: TaskId });
                reply(commands::app::cancel_task(&self.tasks, a.task_id))
            }
            "get_settings" => reply(commands::app::get_settings(&self.settings, &self.store)),
            "set_download_dir" => {
                args!(a, args, { path: Option<String> });
                reply(commands::app::set_download_dir(&self.settings, &self.store, a.path).await)
            }
            _ => Err(CommandError::new(
                "invalidInput",
                format!("unknown command {command:?}"),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    /// Commands the desktop app or the server answer themselves.
    const TRANSPORT_COMMANDS: &[&str] = &[
        "app_info",
        "open_data_dir",
        "new_export_dir",
        "download_ticket",
    ];

    fn names_between<'a>(
        source: &'a str,
        start: &str,
        end: &str,
        pattern: fn(&str) -> Option<&str>,
    ) -> Vec<&'a str> {
        let body = &source[source.find(start).unwrap()..];
        let body = &body[..body.find(end).unwrap()];
        body.lines().filter_map(pattern).collect()
    }

    #[test]
    fn matches_the_frontend_command_table() {
        let ts = include_str!("../../../app/src/lib/api.ts");
        // Entries sit at two spaces; deeper lines are their argument types.
        let frontend = names_between(ts, "export interface Commands {", "\n}", |l| {
            l.strip_prefix("  ")
                .filter(|l| !l.starts_with(' '))
                .and_then(|l| l.split_once(':'))
                .map(|(name, _)| name)
                .filter(|n| n.chars().all(|c| c.is_ascii_lowercase() || c == '_'))
        });
        let rust = names_between(
            include_str!("dispatch.rs"),
            "match command {",
            "_ => Err",
            |l| {
                l.trim()
                    .strip_prefix('"')
                    .and_then(|l| l.split_once('"'))
                    .map(|(name, _)| name)
            },
        );
        assert!(frontend.len() > 40, "parsed {frontend:?}");
        for name in &frontend {
            assert!(
                rust.contains(name) || TRANSPORT_COMMANDS.contains(name),
                "{name} is in api.ts but not dispatched"
            );
        }
        for name in &rust {
            assert!(
                frontend.contains(name),
                "{name} is dispatched but not in api.ts"
            );
        }
    }
}
