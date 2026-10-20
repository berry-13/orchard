//! IPA re-signing with a developer certificate and provisioning profile.

use std::path::PathBuf;
use std::sync::Arc;

use crate::events::Events;
use orchard_core::{Error, Progress, Udid};
use orchard_ipa::{ResignOptions, SigningIdentity};

use crate::commands::app::SettingsStore;
use crate::dto::{ProfileInfo, ResignIssue, ResignReport, SigningIdentityInfo};
use crate::error::CmdResult;
use crate::events::{Reporter, TaskId, TaskRegistry, run_task};
use crate::state::{DeviceState, SigningState, blocking};

pub fn to_issue(issue: orchard_ipa::ResignIssue) -> ResignIssue {
    use orchard_ipa::ResignIssue as I;
    match issue {
        I::Encrypted => ResignIssue::Encrypted,
        I::CertificateExpired { expired_at } => ResignIssue::CertificateExpired {
            expired_at: expired_at * 1000,
        },
        I::ProfileExpired { expired_at } => ResignIssue::ProfileExpired {
            expired_at: expired_at * 1000,
        },
        I::TeamMismatch {
            certificate,
            profile,
        } => ResignIssue::TeamMismatch {
            certificate,
            profile,
        },
        I::CertificateNotInProfile => ResignIssue::CertificateNotInProfile,
        I::BundleIdNotMatched { bundle_id, pattern } => {
            ResignIssue::BundleIdNotMatched { bundle_id, pattern }
        }
        I::DeviceNotProvisioned { udid } => ResignIssue::DeviceNotProvisioned { udid },
    }
}

pub fn to_profile_info(
    id: String,
    path: String,
    p: &orchard_ipa::ProvisioningProfile,
) -> ProfileInfo {
    ProfileInfo {
        id,
        path,
        name: p.name.clone(),
        team_id: p.team_id.clone(),
        team_name: p.team_name.clone(),
        application_identifier: p.application_identifier.clone(),
        bundle_id_pattern: p.bundle_id_pattern().to_owned(),
        wildcard: p.is_wildcard(),
        devices: p.devices.clone(),
        provisions_all_devices: p.provisions_all_devices,
        expires_at: p.expires_at * 1000,
    }
}

fn to_report(r: orchard_ipa::ResignReport, path: String) -> ResignReport {
    ResignReport {
        bundle_id: r.bundle_id,
        team_id: r.team_id,
        bundles: r.bundles.into_iter().map(|b| b.bundle_id).collect(),
        removed: r.removed,
        output_size: r.output_size,
        path,
    }
}

/// Trims an optional text field, treating blank as unset.
fn non_empty(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
}

/// Reads a `.p12` with its password and keeps the identity in app state.
/// The password is used once here and not stored.
pub async fn load_signing_identity(
    signing: &SigningState,
    settings: &SettingsStore,
    path: String,
    password: String,
) -> CmdResult<SigningIdentityInfo> {
    let file = path.clone();
    let identity = blocking(move || {
        let bytes = std::fs::read(&file)?;
        SigningIdentity::from_p12(&bytes, &password).map_err(Into::into)
    })
    .await?;
    let info = identity.info().clone();
    let id = signing.add_identity(identity);
    settings.set_signing_p12_path(path.clone()).await?;
    Ok(SigningIdentityInfo {
        id,
        path,
        common_name: info.common_name,
        team_id: info.team_id,
        expires_at: info.expires_at * 1000,
    })
}

/// Reads a `.mobileprovision` and keeps it in app state.
pub async fn load_provisioning_profile(
    signing: &SigningState,
    settings: &SettingsStore,
    path: String,
) -> CmdResult<ProfileInfo> {
    let file = path.clone();
    let profile = blocking(move || {
        let bytes = std::fs::read(&file)?;
        orchard_ipa::ProvisioningProfile::parse(&bytes).map_err(Into::into)
    })
    .await?;
    let info = to_profile_info(String::new(), path.clone(), &profile);
    let id = signing.add_profile(profile);
    settings.set_signing_profile_path(path).await?;
    Ok(ProfileInfo { id, ..info })
}

/// Problems with re-signing `ipa_path` (optionally under a new bundle id) and
/// installing it on `udid`. An empty list means good to go.
pub async fn check_resign(
    signing: &SigningState,
    ipa_path: String,
    identity_id: String,
    profile_id: String,
    udid: Option<String>,
    bundle_id: Option<String>,
) -> CmdResult<Vec<ResignIssue>> {
    let identity = signing.identity(&identity_id)?;
    let profile = signing.profile(&profile_id)?;
    let bundle_id = non_empty(bundle_id);
    blocking(move || {
        let mut info = orchard_ipa::inspect(&ipa_path)?;
        if let Some(id) = bundle_id {
            info.bundle_id = id;
        }
        let udid = udid.map(Udid);
        let issues = orchard_ipa::check_resign(&info, &identity, &profile, udid.as_ref());
        Ok(issues.into_iter().map(to_issue).collect())
    })
    .await
}

fn options(
    signing: &SigningState,
    identity_id: &str,
    profile_id: &str,
    bundle_id: Option<String>,
    display_name: Option<String>,
) -> CmdResult<ResignOptions> {
    let mut opts = ResignOptions::new(signing.identity(identity_id)?, signing.profile(profile_id)?);
    opts.bundle_id = non_empty(bundle_id);
    opts.display_name = non_empty(display_name);
    Ok(opts)
}

/// Runs the blocking re-sign; progress stages `extract`, `sign`, `package`.
async fn resign(
    rep: &Reporter,
    src: PathBuf,
    dst: PathBuf,
    opts: ResignOptions,
) -> CmdResult<orchard_ipa::ResignReport> {
    let callback = rep.callback();
    blocking(move || orchard_ipa::resign_ipa(&src, &dst, &opts, callback).map_err(Into::into)).await
}

/// Re-signs `ipa_path` into `dest_path`. Task stages: `extract`, `sign`, `package`.
pub async fn resign_ipa(
    app: Events,
    tasks: &TaskRegistry,
    signing: &SigningState,
    task_id: TaskId,
    ipa_path: String,
    identity_id: String,
    profile_id: String,
    bundle_id: Option<String>,
    display_name: Option<String>,
    dest_path: String,
) -> CmdResult<ResignReport> {
    if dest_path == ipa_path {
        return Err(
            Error::InvalidInput("choose a different file for the re-signed IPA".into()).into(),
        );
    }
    let opts = options(signing, &identity_id, &profile_id, bundle_id, display_name)?;
    run_task(&app, tasks, task_id, |rep| async move {
        let report = resign(&rep, ipa_path.into(), PathBuf::from(&dest_path), opts).await?;
        Ok(to_report(report, dest_path))
    })
    .await
}

/// Re-signs into a temporary file, then installs it, as one task. Stages:
/// `extract`, `sign`, `package`, `upload`, `install`.
pub async fn resign_and_install(
    app: Events,
    tasks: &TaskRegistry,
    signing: &SigningState,
    devices: &Arc<DeviceState>,
    task_id: TaskId,
    udid: String,
    ipa_path: String,
    identity_id: String,
    profile_id: String,
    bundle_id: Option<String>,
    display_name: Option<String>,
) -> CmdResult<()> {
    let opts = options(signing, &identity_id, &profile_id, bundle_id, display_name)?;
    let conn = devices.connection(&udid).await?;
    run_task(&app, tasks, task_id, |rep| async move {
        // Owned by the task future: dropping it (also on abort) removes the folder.
        let temp = tempfile::Builder::new()
            .prefix("orchard-resign-")
            .tempdir()?;
        let signed = temp.path().join("resigned.ipa");
        rep.progress(&Progress::new("extract", 0, None));
        resign(&rep, ipa_path.into(), signed.clone(), opts).await?;
        conn.install_ipa(&signed, rep.callback()).await?;
        Ok(())
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issues_use_milliseconds_and_kinds() {
        let v = serde_json::to_value(to_issue(orchard_ipa::ResignIssue::ProfileExpired {
            expired_at: 10,
        }))
        .unwrap();
        assert_eq!(
            v,
            serde_json::json!({"kind": "profileExpired", "expiredAt": 10_000})
        );
        let v = serde_json::to_value(to_issue(orchard_ipa::ResignIssue::BundleIdNotMatched {
            bundle_id: "a.b".into(),
            pattern: "c.*".into(),
        }))
        .unwrap();
        assert_eq!(v["kind"], "bundleIdNotMatched");
        assert_eq!(v["bundleId"], "a.b");
        assert_eq!(
            serde_json::to_value(to_issue(orchard_ipa::ResignIssue::Encrypted)).unwrap(),
            serde_json::json!({"kind": "encrypted"})
        );
    }

    #[test]
    fn blank_fields_are_unset() {
        assert_eq!(non_empty(Some("  ".into())), None);
        assert_eq!(non_empty(Some(" com.x ".into())).as_deref(), Some("com.x"));
    }

    #[test]
    fn unknown_ids_ask_to_reload() {
        let state = SigningState::default();
        let err = state.identity("identity-9").unwrap_err();
        assert_eq!(err.kind, "invalidInput");
    }
}
