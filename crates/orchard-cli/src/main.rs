use std::process::ExitCode;

fn main() -> ExitCode {
    let cmd = std::env::args().nth(1);
    match cmd.as_deref() {
        Some("devices") => match orchard_device::list_devices() {
            Ok(devices) => {
                for udid in devices {
                    println!("{udid}");
                }
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        },
        _ => {
            eprintln!("usage: orchard <devices>");
            ExitCode::from(2)
        }
    }
}
