const COMMANDS: &[&str] = &["announce", "stop"];

fn main() {
    tauri_plugin::Builder::new(COMMANDS).ios_path("ios").build();
}
