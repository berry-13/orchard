use std::io::Write as _;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};
use orchard_core::{AuthError, Error, Progress, Result, Udid, data_dir};
use orchard_device::{DeviceConnection, FileLocation, PairOutcome, PairingStore};
use orchard_ipa::{
    AppStore, LookupKey, PaidPurchase, ProvisioningProfile, ResignOptions, SigningIdentity,
};

#[derive(Parser)]
#[command(name = "orchard", version, about = "iOS device manager")]
struct Cli {
    /// Device UDID; defaults to the only connected device.
    #[arg(short, long, global = true)]
    udid: Option<String>,
    /// Connect over Wi-Fi to this address, using the stored pairing record.
    #[arg(long, global = true)]
    ip: Option<IpAddr>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List devices on USB and Wi-Fi.
    Devices {
        /// Also browse the network for paired devices (seconds).
        #[arg(long)]
        wifi: Option<u64>,
    },
    /// Show device information.
    Info,
    /// Pair with the device (accept the Trust dialog on the device).
    Pair,
    /// Remove the pairing record.
    Unpair,
    /// Enable or disable Wi-Fi connections.
    Wifi { state: OnOff },
    /// Save a screenshot as PNG.
    Screenshot { out: PathBuf },
    /// Stream the system log.
    Syslog,
    /// Restart the device.
    Restart,
    /// Shut the device down.
    Shutdown,
    /// Installed apps: list, install an IPA, uninstall.
    #[command(subcommand)]
    Apps(AppsCmd),
    /// Local IPA files.
    #[command(subcommand)]
    Ipa(IpaCmd),
    /// App Store: sign in, search, download and install.
    #[command(subcommand)]
    Store(StoreCmd),
    /// Device files on the media partition or in an app's Documents.
    #[command(subcommand)]
    Files(FilesCmd),
    /// Create, browse, extract and restore backups.
    #[command(subcommand)]
    Backup(BackupCmd),
    /// Camera roll.
    #[command(subcommand)]
    Photos(PhotosCmd),
    /// Send music, video and books to apps with file sharing (VLC, Infuse, ...).
    #[command(subcommand)]
    Media(MediaCmd),
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum OnOff {
    On,
    Off,
}

#[derive(Subcommand)]
enum AppsCmd {
    /// List installed apps.
    List {
        #[arg(long)]
        system: bool,
    },
    /// Install or upgrade an IPA.
    Install { ipa: PathBuf },
    /// Remove an app and its data.
    Uninstall { bundle_id: String },
}

#[derive(Subcommand)]
enum IpaCmd {
    /// Show IPA metadata.
    Inspect { ipa: PathBuf },
    /// Re-sign with your own certificate and provisioning profile.
    Resign {
        ipa: PathBuf,
        /// Output IPA.
        #[arg(short, long)]
        out: PathBuf,
        /// Signing certificate and key (.p12).
        #[arg(long)]
        p12: PathBuf,
        /// .p12 password; read from ORCHARD_P12_PASSWORD when omitted.
        #[arg(long)]
        password: Option<String>,
        /// Provisioning profile for the main app.
        #[arg(long)]
        profile: PathBuf,
        /// Extra profiles for extensions, matched by bundle id.
        #[arg(long = "extension-profile")]
        extension_profiles: Vec<PathBuf>,
        #[arg(long)]
        bundle_id: Option<String>,
        #[arg(long)]
        display_name: Option<String>,
    },
}

#[derive(Subcommand)]
enum StoreCmd {
    /// Sign in with an Apple ID.
    Login {
        email: String,
        /// Read from ORCHARD_PASSWORD when omitted.
        #[arg(long)]
        password: Option<String>,
        /// Two-factor code.
        #[arg(long)]
        code: Option<String>,
        /// Use Orchard's own client even when ipatool is installed (needed by `buy`).
        #[arg(long)]
        native: bool,
    },
    /// Buy a paid app (asks for confirmation before charging).
    Buy {
        app: String,
        /// Confirm without prompting.
        #[arg(long)]
        yes: bool,
    },
    /// Forget the stored account.
    Logout,
    /// Search the App Store.
    Search {
        term: String,
        #[arg(long, default_value = "us")]
        country: String,
        #[arg(long, default_value_t = 10)]
        limit: u32,
    },
    /// Download an IPA by app id or bundle id.
    Download {
        app: String,
        #[arg(long)]
        version: Option<String>,
        #[arg(short, long, default_value = ".")]
        out: PathBuf,
    },
    /// Download and install on the device.
    Install {
        app: String,
        #[arg(long)]
        version: Option<String>,
    },
    /// List available version ids.
    Versions { app: String },
}

#[derive(Subcommand)]
enum FilesCmd {
    /// List a folder.
    Ls {
        #[arg(default_value = "/")]
        path: String,
        /// App bundle id; browses its Documents folder.
        #[arg(long)]
        app: Option<String>,
    },
    /// Copy a device file to this computer.
    Pull {
        remote: String,
        local: PathBuf,
        #[arg(long)]
        app: Option<String>,
    },
    /// Copy a local file to the device.
    Push {
        local: PathBuf,
        remote: String,
        #[arg(long)]
        app: Option<String>,
    },
    /// Delete a file or folder.
    Rm {
        path: String,
        #[arg(short, long)]
        recursive: bool,
        #[arg(long)]
        app: Option<String>,
    },
    /// Create a folder and its parents.
    Mkdir {
        path: String,
        #[arg(long)]
        app: Option<String>,
    },
}

#[derive(Subcommand)]
enum BackupCmd {
    /// List local backups.
    List,
    /// Back up the device into Orchard's backup folder.
    Create {
        #[arg(long)]
        full: bool,
    },
    /// List files in a backup.
    Files {
        backup: PathBuf,
        #[arg(long)]
        domain: Option<String>,
        #[arg(long)]
        search: Option<String>,
        #[arg(long)]
        password: Option<String>,
    },
    /// Extract a domain (optionally a path prefix) to a folder.
    Extract {
        backup: PathBuf,
        domain: String,
        out: PathBuf,
        #[arg(long)]
        prefix: Option<String>,
        #[arg(long)]
        password: Option<String>,
    },
    /// Restore a backup onto the device.
    Restore {
        backup: PathBuf,
        #[arg(long)]
        password: Option<String>,
    },
}

#[derive(Subcommand)]
enum MediaCmd {
    /// List apps that accept files.
    Targets,
    /// List media already in an app.
    Ls {
        bundle_id: String,
        #[arg(default_value = "/Documents")]
        dir: String,
    },
    /// Copy files or folders into an app's Documents.
    Send {
        bundle_id: String,
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        /// Subfolder of Documents.
        #[arg(long)]
        into: Option<String>,
    },
}

#[derive(Subcommand)]
enum PhotosCmd {
    /// List camera roll files.
    List,
    /// Export the camera roll, skipping files already exported.
    Export { out: PathBuf },
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<()> {
    let data = data_dir();
    let store = PairingStore::new(&data);
    let target = Target {
        udid: cli.udid,
        ip: cli.ip,
    };
    match cli.command {
        Command::Devices { wifi } => {
            for device in orchard_device::list_devices().await.unwrap_or_default() {
                println!("{}\t{}", device.udid, device.connection);
            }
            if let Some(secs) = wifi {
                let found =
                    orchard_device::discover_wifi_devices(&store, Duration::from_secs(secs))
                        .await?;
                for device in found {
                    let ip = device
                        .preferred_address()
                        .map_or_else(|| "?".into(), |ip| ip.to_string());
                    println!("{}\tmdns {ip}", device.udid);
                }
            }
        }
        Command::Info => {
            let info = target.connect(&store).await?.info().await?;
            println!("{}", to_json(&info)?);
        }
        Command::Pair => {
            let mut conn = target.connect(&store).await?;
            eprintln!("Tap Trust on the device and enter the passcode.");
            match conn
                .pair(&store, "orchard", Duration::from_secs(60))
                .await?
            {
                PairOutcome::Paired => println!("paired"),
                PairOutcome::TrustPending => return Err(device("no answer on the device")),
                PairOutcome::Denied => return Err(device("pairing was denied")),
                PairOutcome::PasscodeRequired => return Err(device("unlock the device first")),
            }
        }
        Command::Unpair => target.connect(&store).await?.unpair(&store).await?,
        Command::Wifi { state } => {
            let conn = target.connect(&store).await?;
            conn.set_wifi_connections(matches!(state, OnOff::On))
                .await?;
        }
        Command::Screenshot { out } => {
            let png = target.connect(&store).await?.screenshot().await?;
            std::fs::write(&out, png)?;
        }
        Command::Syslog => {
            let mut log = target.connect(&store).await?.syslog().await?;
            while let Some(line) = log.next_line().await {
                println!("{}", line?);
            }
        }
        Command::Restart => target.connect(&store).await?.restart().await?,
        Command::Shutdown => target.connect(&store).await?.shutdown().await?,
        Command::Apps(cmd) => apps(cmd, &target, &store).await?,
        Command::Ipa(IpaCmd::Resign {
            ipa,
            out,
            p12,
            password,
            profile,
            extension_profiles,
            bundle_id,
            display_name,
        }) => {
            let password = password
                .or_else(|| std::env::var("ORCHARD_P12_PASSWORD").ok())
                .unwrap_or_default();
            let identity = SigningIdentity::from_p12(&std::fs::read(&p12)?, &password)?;
            let profile = ProvisioningProfile::parse(&std::fs::read(&profile)?)?;
            let info = orchard_ipa::inspect(&ipa)?;
            for issue in orchard_ipa::check_resign(&info, &identity, &profile, None) {
                eprintln!("warning: {}", to_json(&issue)?.replace('\n', " "));
            }
            let mut options = ResignOptions::new(identity, profile);
            for path in extension_profiles {
                options
                    .extension_profiles
                    .push(ProvisioningProfile::parse(&std::fs::read(&path)?)?);
            }
            options.bundle_id = bundle_id;
            options.display_name = display_name;
            let summary = orchard_ipa::resign_ipa(&ipa, &out, &options, report)?;
            finish_progress();
            println!("{}", to_json(&summary)?);
        }
        Command::Ipa(IpaCmd::Inspect { ipa }) => {
            let mut info = orchard_ipa::inspect(&ipa)?;
            info.icon_png = None;
            println!("{}", to_json(&info)?);
        }
        Command::Store(cmd) => store_cmd(cmd, &target, &store, &data).await?,
        Command::Files(cmd) => files(cmd, &target, &store).await?,
        Command::Backup(cmd) => backup(cmd, &target, &store, &data).await?,
        Command::Media(cmd) => media(cmd, &target, &store).await?,
        Command::Photos(cmd) => {
            let conn = target.connect(&store).await?;
            let mut media = conn.files(FileLocation::Media).await?;
            let items = orchard_library::list_camera_roll(&mut media).await?;
            match cmd {
                PhotosCmd::List => {
                    for item in &items {
                        println!("{}\t{}", item.path, item.size);
                    }
                }
                PhotosCmd::Export { out } => {
                    let summary =
                        orchard_library::export_photos(&mut media, &items, &out, report).await?;
                    finish_progress();
                    println!("{}", to_json(&summary)?);
                }
            }
        }
    }
    Ok(())
}

async fn media(cmd: MediaCmd, target: &Target, store: &PairingStore) -> Result<()> {
    let conn = target.connect(store).await?;
    match cmd {
        MediaCmd::Targets => {
            for t in orchard_library::media_targets(&conn).await? {
                let accepts: Vec<String> = t.accepts.iter().map(|c| format!("{c:?}")).collect();
                println!("{}\t{}\t{}", t.bundle_id, t.name, accepts.join(","));
            }
        }
        MediaCmd::Ls { bundle_id, dir } => {
            let mut docs = conn.files(FileLocation::AppDocuments(bundle_id)).await?;
            for f in orchard_library::list_media(&mut docs, &dir).await? {
                let category = f
                    .category
                    .map_or_else(|| "dir".into(), |c| format!("{c:?}"));
                println!("{}\t{}\t{}", category, f.entry.size, f.entry.name);
            }
        }
        MediaCmd::Send {
            bundle_id,
            paths,
            into,
        } => {
            let mut docs = conn.files(FileLocation::AppDocuments(bundle_id)).await?;
            let summary =
                orchard_library::send_media(&mut docs, &paths, into.as_deref(), report).await?;
            finish_progress();
            println!("{}", to_json(&summary)?);
        }
    }
    Ok(())
}

async fn apps(cmd: AppsCmd, target: &Target, store: &PairingStore) -> Result<()> {
    let conn = target.connect(store).await?;
    match cmd {
        AppsCmd::List { system } => {
            for app in conn.list_apps(system).await? {
                println!(
                    "{}\t{}\t{}",
                    app.bundle_id,
                    app.short_version.as_deref().unwrap_or("-"),
                    app.name
                );
            }
        }
        AppsCmd::Install { ipa } => {
            conn.install_ipa(&ipa, report).await?;
            finish_progress();
        }
        AppsCmd::Uninstall { bundle_id } => conn.uninstall_app(&bundle_id).await?,
    }
    Ok(())
}

async fn store_cmd(
    cmd: StoreCmd,
    target: &Target,
    store: &PairingStore,
    data: &Path,
) -> Result<()> {
    let app_store = AppStore::new(data)?;
    match cmd {
        StoreCmd::Login {
            email,
            password,
            code,
            native,
        } => {
            let password = password
                .or_else(|| std::env::var("ORCHARD_PASSWORD").ok())
                .ok_or_else(|| {
                    Error::InvalidInput("pass --password or set ORCHARD_PASSWORD".into())
                })?;
            let signed_in = if native {
                app_store
                    .sign_in_native(&email, &password, code.as_deref())
                    .await
            } else {
                app_store.sign_in(&email, &password, code.as_deref()).await
            };
            match signed_in {
                Ok(account) => println!("signed in as {}", account.email),
                Err(Error::Auth(AuthError::CodeRequired)) => {
                    return Err(Error::InvalidInput(
                        "two-factor code sent to your devices; rerun with --code".into(),
                    ));
                }
                Err(e) => return Err(e),
            }
        }
        StoreCmd::Logout => app_store.sign_out().await?,
        StoreCmd::Buy { app, yes } => {
            let id = resolve_app(&app_store, &app).await?;
            let country = app_store
                .account()
                .await
                .ok()
                .flatten()
                .and_then(|a| a.country)
                .unwrap_or_else(|| "us".into());
            let listing = app_store
                .lookup(&LookupKey::Id(id), &country)
                .await?
                .ok_or_else(|| Error::NotFound(format!("app {app}")))?;
            if listing.price <= 0.0 {
                app_store.purchase(id).await?;
                println!("got {}", listing.name);
                return Ok(());
            }
            match app_store.purchase_paid(id, listing.price, None).await? {
                PaidPurchase::Purchased => {}
                PaidPurchase::NeedsConfirmation(confirmation) => {
                    eprintln!("{}", confirmation.message);
                    if !yes && !confirm(&format!("{}? [y/N] ", confirmation.ok_label))? {
                        return Err(Error::InvalidInput("purchase cancelled".into()));
                    }
                    if let PaidPurchase::NeedsConfirmation(_) = app_store
                        .purchase_paid(id, listing.price, Some(&confirmation))
                        .await?
                    {
                        return Err(Error::Store("Apple asked for another confirmation".into()));
                    }
                }
            }
            println!(
                "bought {} for {}",
                listing.name,
                listing
                    .formatted_price
                    .as_deref()
                    .unwrap_or("its listed price")
            );
        }
        StoreCmd::Search {
            term,
            country,
            limit,
        } => {
            for app in app_store.search(&term, &country, limit).await? {
                println!(
                    "{}\t{}\t{}\t{}",
                    app.id, app.bundle_id, app.version, app.name
                );
            }
        }
        StoreCmd::Download { app, version, out } => {
            let id = resolve_app(&app_store, &app).await?;
            let path = app_store
                .download(id, version.as_deref(), &out, report)
                .await?;
            finish_progress();
            println!("{}", path.display());
        }
        StoreCmd::Install { app, version } => {
            let id = resolve_app(&app_store, &app).await?;
            let conn = target.connect(store).await?;
            let dir = tempfile::tempdir()?;
            let path = app_store
                .download(id, version.as_deref(), dir.path(), report)
                .await?;
            conn.install_ipa(&path, report).await?;
            finish_progress();
        }
        StoreCmd::Versions { app } => {
            let id = resolve_app(&app_store, &app).await?;
            println!("{}", to_json(&app_store.list_versions(id).await?)?);
        }
    }
    Ok(())
}

async fn files(cmd: FilesCmd, target: &Target, store: &PairingStore) -> Result<()> {
    let conn = target.connect(store).await?;
    let location =
        |app: Option<String>| app.map_or(FileLocation::Media, FileLocation::AppDocuments);
    match cmd {
        FilesCmd::Ls { path, app } => {
            let mut fs = conn.files(location(app)).await?;
            for entry in fs.list_dir(&path).await? {
                let marker = if entry.is_dir() { "/" } else { "" };
                println!("{}\t{}{marker}", entry.size, entry.name);
            }
        }
        FilesCmd::Pull { remote, local, app } => {
            let mut fs = conn.files(location(app)).await?;
            fs.download(&remote, &local, report).await?;
            finish_progress();
        }
        FilesCmd::Push { local, remote, app } => {
            let mut fs = conn.files(location(app)).await?;
            fs.upload(&local, &remote, report).await?;
            finish_progress();
        }
        FilesCmd::Rm {
            path,
            recursive,
            app,
        } => {
            conn.files(location(app))
                .await?
                .remove(&path, recursive)
                .await?
        }
        FilesCmd::Mkdir { path, app } => conn.files(location(app)).await?.mkdir_all(&path).await?,
    }
    Ok(())
}

async fn backup(cmd: BackupCmd, target: &Target, store: &PairingStore, data: &Path) -> Result<()> {
    let root = data.join("backups");
    match cmd {
        BackupCmd::List => {
            for dir in orchard_backup::default_backup_dirs(Some(&root)) {
                for b in orchard_backup::list_backups(&dir)? {
                    println!(
                        "{}\t{}\t{}\t{}{}",
                        b.path.display(),
                        b.device_name.as_deref().unwrap_or("-"),
                        b.ios_version.as_deref().unwrap_or("-"),
                        b.last_backup_date.as_deref().unwrap_or("-"),
                        if b.encrypted { "\tencrypted" } else { "" }
                    );
                }
            }
        }
        BackupCmd::Create { full } => {
            let conn = target.connect(store).await?;
            let summary = orchard_backup::create_backup(&conn, &root, full, report).await?;
            finish_progress();
            println!("{}", summary.path.display());
        }
        BackupCmd::Files {
            backup,
            domain,
            search,
            password,
        } => {
            let backup = orchard_backup::Backup::open(&backup, password.as_deref())?;
            let query = orchard_backup::FileQuery {
                domain,
                search,
                ..Default::default()
            };
            for f in backup.files(&query)? {
                println!("{}\t{}\t{}", f.domain, f.size, f.relative_path);
            }
        }
        BackupCmd::Extract {
            backup,
            domain,
            out,
            prefix,
            password,
        } => {
            let backup = orchard_backup::Backup::open(&backup, password.as_deref())?;
            let summary = backup.extract(&domain, prefix.as_deref().unwrap_or(""), &out, report)?;
            finish_progress();
            println!("{}", to_json(&summary)?);
        }
        BackupCmd::Restore { backup, password } => {
            let source = backup
                .file_name()
                .and_then(|n| n.to_str())
                .map(str::to_owned);
            let parent = backup
                .parent()
                .ok_or_else(|| Error::InvalidInput("backup path has no parent".into()))?;
            let settings = orchard_backup::RestoreSettings {
                password,
                ..Default::default()
            };
            let conn = target.connect(store).await?;
            orchard_backup::restore_backup(&conn, parent, source.as_deref(), &settings, report)
                .await?;
            finish_progress();
        }
    }
    Ok(())
}

struct Target {
    udid: Option<String>,
    ip: Option<IpAddr>,
}

impl Target {
    async fn connect(&self, store: &PairingStore) -> Result<DeviceConnection> {
        let udid = match &self.udid {
            Some(udid) => Udid(udid.clone()),
            None => {
                let mut udids: Vec<Udid> = orchard_device::list_devices()
                    .await?
                    .into_iter()
                    .map(|d| d.udid)
                    .collect();
                udids.dedup();
                match udids.len() {
                    1 => udids.remove(0),
                    0 => return Err(Error::NotFound("connected device".into())),
                    _ => return Err(Error::InvalidInput("several devices, pass --udid".into())),
                }
            }
        };
        match self.ip {
            Some(ip) => DeviceConnection::connect_wifi(&udid, ip, store),
            None => DeviceConnection::connect(&udid, store).await,
        }
    }
}

/// Accepts a numeric App Store id or a bundle id.
async fn resolve_app(app_store: &AppStore, app: &str) -> Result<u64> {
    if let Ok(id) = app.parse() {
        return Ok(id);
    }
    let country = app_store
        .account()
        .await
        .ok()
        .flatten()
        .and_then(|a| a.country)
        .unwrap_or_else(|| "us".into());
    app_store
        .lookup(&LookupKey::BundleId(app.to_owned()), &country)
        .await?
        .map(|a| a.id)
        .ok_or_else(|| Error::NotFound(format!("app {app}")))
}

fn confirm(prompt: &str) -> Result<bool> {
    eprint!("{prompt}");
    std::io::stderr().flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes"))
}

fn device(msg: &str) -> Error {
    Error::Device(msg.into())
}

fn to_json(value: &impl serde::Serialize) -> Result<String> {
    serde_json::to_string_pretty(value).map_err(|e| Error::Protocol(e.to_string()))
}

fn report(p: Progress) {
    let line = match p.total {
        Some(total) if total > 0 => format!("{} {:>3}%", p.stage, p.done * 100 / total),
        _ => format!("{} {}", p.stage, p.done),
    };
    eprint!("\r{line:<24}");
    let _ = std::io::stderr().flush();
}

fn finish_progress() {
    eprintln!();
}
