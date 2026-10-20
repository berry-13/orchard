const COMMANDS: &[&str] = &["announce", "stop", "start_device_relay", "stop_device_relay"];

fn main() {
    tauri_plugin::Builder::new(COMMANDS).ios_path("ios").build();
}
