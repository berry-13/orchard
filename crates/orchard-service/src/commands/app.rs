//! App level commands: settings, info, task cancellation.

use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::dto::{Settings, StoreBackend};
use crate::error::{CmdResult, CommandError};
use crate::events::{TaskId, TaskRegistry};
use crate::state::StoreState;

/// Persisted part of the settings, stored as `settings.json` in the data dir.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredSettings {
    download_dir: Option<String>,
    /// Storefront country chosen at App Store sign-in, two letters.
    store_country: Option<String>,
    /// Last `.p12` used for re-signing (never its password).
    signing_p12_path: Option<String>,
    /// Last `.mobileprovision` used for re-signing.
    signing_profile_path: Option<String>,
}

/// Settings file and its cached contents. Managed Tauri state.
pub struct SettingsStore {
    data_dir: PathBuf,
    stored: Mutex<StoredSettings>,
}

impl SettingsStore {
    /// Loads `settings.json` from Orchard's data dir (shared with the CLI),
    /// defaulting when missing or invalid.
    pub fn load(data_dir: PathBuf) -> Self {
        let stored = std::fs::read(data_dir.join("settings.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            data_dir,
            stored: Mutex::new(stored),
        }
    }

    /// Orchard's data directory (pairing records, sessions, cache, settings).
    pub fn data_dir(&self) -> &std::path::Path {
        &self.data_dir
    }

    fn snapshot(&self, store_backend: StoreBackend) -> Settings {
        let stored = self.stored.lock().unwrap_or_else(|e| e.into_inner());
        Settings {
            download_dir: stored.download_dir.clone(),
            data_dir: self.data_dir.display().to_string(),
            store_sign_in_note: (store_backend == StoreBackend::Ipatool)
                .then(|| orchard_ipa::ARGV_EXPOSURE_NOTE.to_owned()),
            store_backend,
            signing_p12_path: stored.signing_p12_path.clone(),
            signing_profile_path: stored.signing_profile_path.clone(),
        }
    }

    /// Storefront country remembered from the last sign-in.
    pub fn store_country(&self) -> Option<String> {
        self.stored
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .store_country
            .clone()
    }

    async fn update(&self, change: impl FnOnce(&mut StoredSettings)) -> CmdResult<()> {
        let json = {
            let mut stored = self.stored.lock().unwrap_or_else(|e| e.into_inner());
            change(&mut stored);
            serde_json::to_vec_pretty(&*stored)
                .map_err(|e| CommandError::new("protocol", e.to_string()))?
        };
        tokio::fs::create_dir_all(&self.data_dir).await?;
        tokio::fs::write(self.data_dir.join("settings.json"), json).await?;
        Ok(())
    }

    pub async fn set_signing_p12_path(&self, path: String) -> CmdResult<()> {
        self.update(|s| s.signing_p12_path = Some(path)).await
    }

    pub async fn set_signing_profile_path(&self, path: String) -> CmdResult<()> {
        self.update(|s| s.signing_profile_path = Some(path)).await
    }

    pub async fn set_store_country(&self, country: String) -> CmdResult<()> {
        self.update(|s| s.store_country = Some(country)).await
    }
}

/// Flags a running task as cancelled; unknown ids are ignored.
pub fn cancel_task(tasks: &TaskRegistry, task_id: TaskId) -> CmdResult<()> {
    tasks.cancel(task_id.as_str());
    Ok(())
}

pub fn get_settings(settings: &SettingsStore, store: &StoreState) -> CmdResult<Settings> {
    Ok(settings.snapshot(store.backend()))
}

pub async fn set_download_dir(
    settings: &SettingsStore,
    store: &StoreState,
    path: Option<String>,
) -> CmdResult<Settings> {
    settings.update(|s| s.download_dir = path).await?;
    Ok(settings.snapshot(store.backend()))
}
