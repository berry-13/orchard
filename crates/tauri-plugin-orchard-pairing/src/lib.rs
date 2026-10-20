//! iOS side of Orchard's Wi-Fi pairing. The work is in Swift
//! (`ios/Sources`): `announce` publishes a pairing host the server set up
//! (`start_wifi_pairing`) on this device's Wi-Fi with Bonjour and relays the
//! connection a device opens to it to the server; `stop` ends it. The page
//! calls them as `plugin:orchard-pairing|announce` and `...|stop`; elsewhere
//! the plugin does nothing.

// `ios_plugin_binding!` declares the Swift entry point, an extern "C" fn.
#![cfg_attr(target_os = "ios", allow(unsafe_code))]

use tauri::Runtime;
use tauri::plugin::{Builder, TauriPlugin};

#[cfg(target_os = "ios")]
tauri::ios_plugin_binding!(init_plugin_orchard_pairing);

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("orchard-pairing")
        .setup(|_app, _api| {
            #[cfg(target_os = "ios")]
            _api.register_ios_plugin(init_plugin_orchard_pairing)?;
            Ok(())
        })
        .build()
}
