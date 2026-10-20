use std::process::ExitCode;

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let cmd = std::env::args().nth(1);
    match cmd.as_deref() {
        Some("devices") => match orchard_device::list_devices().await {
            Ok(devices) => {
                for device in devices {
                    println!("{}", device.udid);
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
