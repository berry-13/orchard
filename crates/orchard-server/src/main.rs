use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use orchard_server::{AppState, Auth, Files, auth::CODE_LIFETIME_SECS, event_channel, router};
use orchard_service::Service;

/// Orchard as a network service for the web and iPad clients.
#[derive(Parser)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    /// Address to listen on.
    #[arg(long, default_value = "0.0.0.0:7420")]
    listen: SocketAddr,
    /// The built web app (`npm run build` in app/) to serve.
    #[arg(long, env = "ORCHARD_UI_DIR")]
    ui: Option<PathBuf>,
    /// Also serve clients outside the local network and VPN ranges. Only do
    /// this behind HTTPS.
    #[arg(long)]
    allow_public: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Print a new connect code for the running server.
    Code,
    /// Sign out every connected client.
    Revoke,
}

fn server_dir() -> PathBuf {
    orchard_core::data_dir().join("server")
}

fn print_code(code: &str) {
    println!(
        "Connect code: {code} (valid for {} minutes; `orchard-server code` prints a new one)",
        CODE_LIFETIME_SECS / 60
    );
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let dir = server_dir();
    match cli.command {
        Some(Command::Code) => {
            print_code(&Auth::new_code(&dir)?);
            return Ok(());
        }
        Some(Command::Revoke) => {
            println!("Signed out {} client(s)", Auth::revoke_all(&dir)?);
            return Ok(());
        }
        None => {}
    }

    let (events, sink) = event_channel();
    let service = Arc::new(Service::new(orchard_core::data_dir(), sink).map_err(|e| e.message)?);
    tokio::spawn(service.watch_devices());
    let state = Arc::new(AppState {
        service,
        auth: Auth::open(&dir)?,
        files: Files::new(&dir)?,
        events,
        allow_public: cli.allow_public,
    });
    if let Some(ui) = &cli.ui
        && !ui.join("index.html").is_file()
    {
        return Err(format!("{} has no index.html; build the app first", ui.display()).into());
    }

    let listener = tokio::net::TcpListener::bind(cli.listen).await?;
    println!("Orchard server on http://{}", listener.local_addr()?);
    if cli.ui.is_none() {
        println!("No web UI (pass --ui app/dist to serve it); API only");
    }
    print_code(&Auth::new_code(&dir)?);
    let app = router(state, cli.ui.as_deref());
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}
