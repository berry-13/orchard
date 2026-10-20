#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use orchard_core::{AuthError, Error, Progress};
use orchard_ipa::{AppRef, Ipatool, StoreIdentity};

/// A fake ipatool that records its argv and answers like ipatool 2.x in JSON mode.
const SCRIPT: &str = r#"#!/bin/sh
printf '%s\n' "$@" > "$ARGS_FILE"
case "$1 $2" in
"--version ")
  echo "ipatool version 2.2.0"; exit 0 ;;
"auth login")
  case "$*" in
  *"--auth-code"*)
    echo '{"level":"info","name":"Jane Appleseed","email":"jane@example.com","success":true,"time":"2026-10-19T10:00:00Z"}' ;;
  *" -p wrong "*)
    echo '{"level":"error","error":"Your Apple ID or password was entered incorrectly.","success":false,"time":"2026-10-19T10:00:00Z"}'; exit 1 ;;
  *)
    echo '{"level":"info","message":"preparing authentication; the first login may take a few minutes"}'
    echo '{"level":"info","message":"2FA code is required; run the command again and supply a code using the `--auth-code` flag"}' ;;
  esac ;;
"auth info")
  if [ -f "$STATE/signed-out" ]; then
    echo '{"level":"error","error":"failed to get account: failed to get item: The specified item could not be found in the keyring","success":false}'; exit 1
  fi
  echo '{"level":"info","name":"Jane Appleseed","email":"jane@example.com","success":true}' ;;
"auth revoke")
  touch "$STATE/signed-out"
  echo '{"level":"info","success":true}' ;;
"purchase -i")
  echo '{"level":"error","error":"failed to purchase item with param '"'"'STDQ'"'"': password token is expired","success":false}' >&2; exit 1 ;;
"purchase -b")
  echo '{"level":"info","alreadyOwned":true,"success":true}' ;;
"list-versions -b")
  echo '{"level":"info","externalVersionIdentifiers":["100","200","300"],"bundleID":"com.example.demo","success":true}' ;;
"download -i")
  if [ "$3" = "1" ]; then
    echo '{"level":"error","error":"license is required","success":false}'; exit 1
  fi
  out="$6/com.example.demo_$3_1.2.3.ipa"
  printf 'PK-fake-ipa' > "$out"
  echo '{"level":"info","output":"'"$out"'","purchased":false,"success":true}' ;;
*)
  echo '{"level":"error","error":"unknown command","success":false}'; exit 1 ;;
esac
"#;

struct Fake {
    _dir: tempfile::TempDir,
    root: std::path::PathBuf,
    tool: Ipatool,
}

impl Fake {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let bin = root.join("ipatool.sh");
        std::fs::write(&bin, SCRIPT).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::create_dir(root.join("state")).unwrap();
        // The script reads its paths from the environment set below.
        let wrapper = root.join("ipatool");
        std::fs::write(
            &wrapper,
            format!(
                "#!/bin/sh\nARGS_FILE='{0}/args' STATE='{0}/state' exec '{1}' \"$@\"\n",
                root.display(),
                bin.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
        let tool = Ipatool::with_binary(&wrapper, &root.join("data"));
        Self {
            _dir: dir,
            root,
            tool,
        }
    }

    fn argv(&self) -> Vec<String> {
        std::fs::read_to_string(self.root.join("args"))
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn passphrase(&self) -> String {
        std::fs::read_to_string(self.root.join("data/ipatool-passphrase")).unwrap()
    }

    fn expect_argv(&self, command: &[&str]) {
        let mut expected: Vec<String> = command.iter().map(|s| s.to_string()).collect();
        expected.extend(
            [
                "--format",
                "json",
                "--non-interactive",
                "--keychain-passphrase",
            ]
            .map(String::from),
        );
        expected.push(self.passphrase());
        assert_eq!(self.argv(), expected);
    }
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

// One test so the fake binary is never written while another test forks.
#[tokio::test]
async fn bridge_maps_ipatool_runs() {
    let fake = Fake::new();
    let tool = &fake.tool;

    assert_eq!(tool.version().await.unwrap(), "2.2.0");
    assert_eq!(fake.argv(), ["--version"]);

    // Passphrase: created once with 0600, then reused.
    let first = tool.passphrase().unwrap();
    assert_eq!(first.len(), 48);
    assert_eq!(mode(&fake.root.join("data/ipatool-passphrase")), 0o600);
    assert_eq!(tool.passphrase().unwrap(), first);

    let err = tool
        .login("jane@example.com", "secret", None)
        .await
        .unwrap_err();
    assert!(
        matches!(err, Error::Auth(AuthError::CodeRequired)),
        "{err:?}"
    );
    fake.expect_argv(&["auth", "login", "-e", "jane@example.com", "-p", "secret"]);

    let identity = tool
        .login("jane@example.com", "secret", Some("123456"))
        .await
        .unwrap();
    assert_eq!(
        identity,
        StoreIdentity {
            email: "jane@example.com".into(),
            name: "Jane Appleseed".into(),
        }
    );
    fake.expect_argv(&[
        "auth",
        "login",
        "-e",
        "jane@example.com",
        "-p",
        "secret",
        "--auth-code",
        "123456",
    ]);

    let err = tool
        .login("jane@example.com", "wrong", None)
        .await
        .unwrap_err();
    assert!(
        matches!(err, Error::Auth(AuthError::BadCredentials(ref m)) if m.contains("incorrectly")),
        "{err:?}"
    );

    assert_eq!(tool.info().await.unwrap(), Some(identity));
    fake.expect_argv(&["auth", "info"]);

    let err = tool.purchase(&AppRef::Id(42)).await.unwrap_err();
    assert!(
        matches!(err, Error::Auth(AuthError::SessionExpired)),
        "{err:?}"
    );
    fake.expect_argv(&["purchase", "-i", "42"]);
    tool.purchase(&AppRef::BundleId("com.example.demo".into()))
        .await
        .unwrap();
    fake.expect_argv(&["purchase", "-b", "com.example.demo"]);

    let versions = tool
        .list_versions(&AppRef::BundleId("com.example.demo".into()))
        .await
        .unwrap();
    assert_eq!(versions.external_version_ids, ["100", "200", "300"]);
    assert_eq!(versions.latest, "300");
    fake.expect_argv(&["list-versions", "-b", "com.example.demo"]);

    let dest = fake.root.join("downloads");
    let mut events: Vec<Progress> = Vec::new();
    let path = tool
        .download(&AppRef::Id(42), Some("200"), &dest, |p| events.push(p))
        .await
        .unwrap();
    assert_eq!(path, dest.join("com.example.demo_42_1.2.3.ipa"));
    assert_eq!(std::fs::read(&path).unwrap(), b"PK-fake-ipa");
    fake.expect_argv(&[
        "download",
        "-i",
        "42",
        "--purchase",
        "-o",
        dest.to_str().unwrap(),
        "--external-version-id",
        "200",
    ]);
    assert_eq!(
        events,
        [
            Progress::new("download", 0, None),
            Progress::new("download", 11, Some(11)),
        ]
    );

    let err = tool
        .download(&AppRef::Id(1), None, &dest, |_| {})
        .await
        .unwrap_err();
    assert!(matches!(err, Error::LicenseRequired), "{err:?}");

    tool.logout().await.unwrap();
    fake.expect_argv(&["auth", "revoke"]);
    assert_eq!(tool.info().await.unwrap(), None);
}
