//! Orchard's commands, independent of how they are called. The desktop app
//! invokes [`Service::dispatch`] through Tauri, the server through HTTP; both
//! deliver [`Events`] to their own clients.

pub mod commands;
pub mod dispatch;
pub mod dto;
pub mod error;
pub mod events;
pub mod paths;
pub mod state;

use std::path::PathBuf;
use std::sync::Arc;

use commands::app::SettingsStore;
use commands::console::SyslogState;
use error::CmdResult;
pub use events::Events;
use events::TaskRegistry;
use state::{BackupState, DeviceState, SigningState, StoreState};

/// Everything the commands share, created once per process.
pub struct Service {
    events: Events,
    tasks: TaskRegistry,
    syslog: SyslogState,
    backups: BackupState,
    signing: SigningState,
    devices: Arc<DeviceState>,
    store: StoreState,
    settings: SettingsStore,
}

impl Service {
    /// State lives under `data_dir`; events go to `events`.
    pub fn new(data_dir: PathBuf, events: Events) -> CmdResult<Self> {
        Ok(Self {
            tasks: TaskRegistry::default(),
            syslog: SyslogState::default(),
            backups: BackupState::default(),
            signing: SigningState::default(),
            devices: Arc::new(DeviceState::new(&data_dir)),
            store: StoreState::new(&data_dir)?,
            settings: SettingsStore::load(data_dir),
            events,
        })
    }

    pub fn data_dir(&self) -> &std::path::Path {
        self.settings.data_dir()
    }

    /// Follows USB attach and detach events forever; spawn it once.
    pub fn watch_devices(&self) -> impl Future<Output = ()> + Send + 'static {
        state::watch_devices(self.events.clone(), self.devices.clone())
    }
}
