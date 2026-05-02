use chrono::Utc;
use serde_json::Value;
use serial_test::serial;
use std::env;
use std::fs;
use std::os::unix::fs::symlink;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn run_command(home: &Path, args: &[&str]) -> (i32, String, String) {
    let path = Path::new(env!("CARGO_BIN_EXE_apw"));
    let output = Command::new(path)
        .env("HOME", home)
        .env("NO_COLOR", "1")
        .args(args)
        .output()
        .expect("failed to run rust cli");

    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).trim().to_string(),
        String::from_utf8_lossy(&output.stderr).trim().to_string(),
    )
}

fn with_temp_home<F, R>(run: F) -> R
where
    F: FnOnce(&Path) -> R,
{
    let temp = TempDir::new().expect("failed to create temp home");
    let previous_home = env::var("HOME").ok();
    env::set_var("HOME", temp.path());

    let result = run(temp.path());

    if let Some(previous_home) = previous_home {
        env::set_var("HOME", previous_home);
    } else {
        env::remove_var("HOME");
    }

    result
}

fn parse_json_output(value: &str) -> Value {
    serde_json::from_str(value).unwrap_or_else(|_| panic!("expected json response, got {}", value))
}

fn write_launch_failure_config(home: &Path, last_launch_error: &str) {
    let config = serde_json::json!({
        "schema": 1,
        "port": 10_000,
        "host": "127.0.0.1",
        "username": "",
        "sharedKey": "",
        "runtimeMode": "auto",
        "lastLaunchStatus": "failed",
        "lastLaunchError": last_launch_error,
        "lastLaunchStrategy": "direct",
        "secretSource": "file",
        "createdAt": Utc::now().to_rfc3339(),
    });

    fs::create_dir_all(home.join(".apw")).expect("failed to create config directory");
    fs::write(
        home.join(".apw/config.json"),
        serde_json::to_vec_pretty(&config).expect("failed to serialize config"),
    )
    .expect("failed to write config");
}

fn write_fallback_provider_config(home: &Path, provider_path: &str) {
    let config = serde_json::json!({
        "schema": 1,
        "port": 10_000,
        "host": "127.0.0.1",
        "username": "demo",
        "sharedKey": "demo-shared-key",
        "runtimeMode": "auto",
        "secretSource": "file",
        "fallbackProvider": "bitwarden",
        "fallbackProviderPath": provider_path,
        "createdAt": Utc::now().to_rfc3339(),
    });

    fs::create_dir_all(home.join(".apw")).expect("failed to create config directory");
    fs::write(
        home.join(".apw/config.json"),
        serde_json::to_vec_pretty(&config).expect("failed to serialize config"),
    )
    .expect("failed to write config");
}

fn write_bitwarden_provider(path: &Path) {
    fs::write(
        path,
        r#"#!/usr/bin/env python3
import json
import sys

if sys.argv[1:] == ["list", "items", "--search", "vault.example.com"]:
    print(json.dumps([
      {
        "login": {
          "username": "alice@example.com",
          "password": "secret-bitwarden",
          "uris": [{"uri": "https://vault.example.com/login"}]
        }
      }
    ]))
else:
    raise SystemExit(1)
"#,
    )
    .expect("failed to write fallback provider");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
        .expect("failed to chmod fallback provider");
}

fn install_native_app_no_results(home: &Path) {
    let executable = home
        .join(".apw")
        .join("native-app")
        .join("installed")
        .join("APW.app")
        .join("Contents")
        .join("MacOS")
        .join("APW");
    fs::create_dir_all(executable.parent().expect("missing executable parent"))
        .expect("failed to create native app bundle");
    fs::write(
        &executable,
        r#"#!/usr/bin/env python3
import json

print(json.dumps({"ok": False, "code": 3, "error": "no credential"}))
"#,
    )
    .expect("failed to write native app executable");
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755))
        .expect("failed to chmod native app executable");
}

#[test]
#[serial]
fn command_invalid_pin_is_rejected_without_network() {
    with_temp_home(|home| {
        let (status, stdout, stderr) = run_command(home, &["--json", "auth", "--pin", "12ab"]);
        assert_eq!(
            status, 2,
            "status={status}, stdout={stdout}, stderr={stderr}"
        );
        let output = parse_json_output(&stderr);
        assert_eq!(output["code"], 2);
        assert!(output["error"]
            .as_str()
            .unwrap_or("")
            .contains("PIN must be exactly 6 digits."));
    });
}

#[test]
#[serial]
fn command_invalid_url_rejected_before_auth_dependency() {
    with_temp_home(|home| {
        let (status, stdout, stderr) = run_command(home, &["--json", "pw", "list", "bad host"]);
        assert_eq!(
            status, 1,
            "status={status}, stdout={stdout}, stderr={stderr}"
        );
        let output = parse_json_output(&stderr);
        assert_eq!(output["code"], 1);
        assert_eq!(output["ok"], false);
        assert!(
            output["error"]
                .as_str()
                .unwrap_or("")
                .contains("Invalid URL")
                || output["error"]
                    .as_str()
                    .unwrap_or("")
                    .contains("Invalid URL host.")
        );
    });
}

#[test]
#[serial]
fn status_json_has_stable_shape() {
    with_temp_home(|home| {
        let (status, stdout, stderr) = run_command(home, &["status", "--json"]);
        assert_eq!(
            status, 0,
            "status={status}, stdout={stdout}, stderr={stderr}"
        );
        let output = parse_json_output(&stdout);
        assert_eq!(output["ok"], true);
        assert_eq!(output["payload"]["releaseLine"]["target"], "v2.0.0");
        assert!(output["payload"]["app"].is_object());
        assert_eq!(output["payload"]["app"]["installed"], false);
        assert!(output["payload"]["daemon"]["host"].is_string());
        assert!(output["payload"]["daemon"]["port"].is_u64());
        assert!(output["payload"]["host"].is_object());
        assert!(output["payload"]["host"]["status"].is_null());
        assert!(output["payload"]["host"]["bundleVersion"].is_null());
        assert!(output["payload"]["host"]["connectedAt"].is_null());
        assert!(output["payload"]["host"]["lastError"].is_null());
        assert!(output["payload"]["bridge"].is_object());
        assert!(output["payload"]["bridge"]["status"].is_null());
        assert!(output["payload"]["bridge"]["browser"].is_null());
        assert!(output["payload"]["bridge"]["connectedAt"].is_null());
        assert!(output["payload"]["bridge"]["lastError"].is_null());
        assert_eq!(output["payload"]["session"]["authenticated"], false);
        assert!(output["payload"]["session"]["createdAt"].is_string());
        assert!(output["payload"]["session"]["expired"].is_boolean());
    });
}

#[test]
#[serial]
fn login_rejects_relative_external_provider_path() {
    with_temp_home(|home| {
        install_native_app_no_results(home);
        write_fallback_provider_config(home, "bw");

        let (status, stdout, stderr) =
            run_command(home, &["--json", "login", "https://vault.example.com"]);

        assert_eq!(
            status, 102,
            "status={status}, stdout={stdout}, stderr={stderr}"
        );
        let output = parse_json_output(&stderr);
        assert_eq!(output["code"], 102);
        let error = output["error"].as_str().unwrap_or_default();
        assert!(error.contains("absolute executable path"));
        assert!(error.contains("relative paths are not allowed"));
    });
}

#[test]
#[serial]
fn login_rejects_tilde_external_provider_path() {
    with_temp_home(|home| {
        install_native_app_no_results(home);
        write_fallback_provider_config(home, "~/bin/bw");

        let (status, stdout, stderr) =
            run_command(home, &["--json", "login", "https://vault.example.com"]);

        assert_eq!(
            status, 102,
            "status={status}, stdout={stdout}, stderr={stderr}"
        );
        let output = parse_json_output(&stderr);
        assert_eq!(output["code"], 102);
        let error = output["error"].as_str().unwrap_or_default();
        assert!(error.contains("absolute executable path"));
        assert!(error.contains("`~`"));
    });
}

#[test]
#[serial]
fn login_rejects_world_writable_external_provider_path() {
    with_temp_home(|home| {
        install_native_app_no_results(home);
        let provider_dir = TempDir::new().expect("failed to create provider tempdir");
        let provider_path = provider_dir.path().join("bw");
        write_bitwarden_provider(&provider_path);
        fs::set_permissions(&provider_path, fs::Permissions::from_mode(0o777))
            .expect("failed to chmod fallback provider");
        write_fallback_provider_config(home, &provider_path.display().to_string());

        let (status, stdout, stderr) =
            run_command(home, &["--json", "login", "https://vault.example.com"]);

        assert_eq!(
            status, 102,
            "status={status}, stdout={stdout}, stderr={stderr}"
        );
        let output = parse_json_output(&stderr);
        assert_eq!(output["code"], 102);
        let error = output["error"].as_str().unwrap_or_default();
        assert!(error.contains("insecure permissions"));
        assert!(error.contains("0755 or more restrictive"));
    });
}

#[test]
#[serial]
fn login_rejects_external_provider_symlink_to_insecure_target() {
    with_temp_home(|home| {
        install_native_app_no_results(home);
        let provider_dir = TempDir::new().expect("failed to create provider tempdir");
        let provider_path = provider_dir.path().join("bw-real");
        let provider_link = provider_dir.path().join("bw-link");
        write_bitwarden_provider(&provider_path);
        fs::set_permissions(&provider_path, fs::Permissions::from_mode(0o777))
            .expect("failed to chmod fallback provider");
        symlink(&provider_path, &provider_link).expect("failed to create provider symlink");
        write_fallback_provider_config(home, &provider_link.display().to_string());

        let (status, stdout, stderr) =
            run_command(home, &["--json", "login", "https://vault.example.com"]);

        assert_eq!(
            status, 102,
            "status={status}, stdout={stdout}, stderr={stderr}"
        );
        let output = parse_json_output(&stderr);
        assert_eq!(output["code"], 102);
        let error = output["error"].as_str().unwrap_or_default();
        assert!(error.contains(&provider_path.display().to_string()));
        assert!(error.contains("insecure permissions"));
    });
}

#[test]
#[serial]
fn status_binary_with_nonexistent_home_directory_isolated() {
    // Ensure we are validating no panic/unsafe path handling on empty state with
    // an unusual HOME directory.
    let home = Path::new("/unlikely/path/that/does/not/exist/for/security/tests");
    fs::remove_dir_all(home).ok();
    let status = run_command(home, &["status", "--json"]);
    assert_eq!(status.0, 0, "status={}", status.0);
    assert!(status.1.contains("\"ok\":true"));
}

#[test]
#[serial]
fn pw_list_reports_failed_launch_state_before_invalid_session() {
    with_temp_home(|home| {
        write_launch_failure_config(home, "helper test failure");
        let (status, stdout, stderr) = run_command(home, &["--json", "pw", "list", "example.com"]);
        assert_eq!(
            status, 103,
            "status={status}, stdout={stdout}, stderr={stderr}"
        );
        let output = parse_json_output(&stderr);
        assert_eq!(output["code"], 103);
        assert_eq!(output["ok"], false);
        let error = output["error"].as_str().unwrap_or_default();
        assert!(error.contains("helper test failure"));
        assert!(error.contains("daemon.preflight.status="));
    });
}

#[test]
#[serial]
fn status_json_preserves_failed_launch_metadata_after_command_failure() {
    with_temp_home(|home| {
        write_launch_failure_config(
            home,
            "Helper process was terminated by SIGKILL (Code Signature Constraint Violation).",
        );

        let (status, stdout, stderr) = run_command(home, &["status", "--json"]);
        assert_eq!(
            status, 0,
            "status={status}, stdout={stdout}, stderr={stderr}"
        );
        let initial = parse_json_output(&stdout);
        assert_eq!(initial["payload"]["daemon"]["runtimeMode"], "auto");
        assert_eq!(initial["payload"]["daemon"]["lastLaunchStatus"], "failed");
        assert_eq!(initial["payload"]["daemon"]["lastLaunchStrategy"], "direct");

        let (pw_status, pw_stdout, pw_stderr) =
            run_command(home, &["--json", "pw", "list", "example.com"]);
        assert_eq!(
            pw_status, 103,
            "status={pw_status}, stdout={pw_stdout}, stderr={pw_stderr}"
        );

        let (status_after, stdout_after, stderr_after) = run_command(home, &["status", "--json"]);
        assert_eq!(
            status_after, 0,
            "status={status_after}, stdout={stdout_after}, stderr={stderr_after}"
        );
        let after = parse_json_output(&stdout_after);
        assert_eq!(after["payload"]["daemon"]["runtimeMode"], "auto");
        assert_eq!(after["payload"]["daemon"]["lastLaunchStatus"], "failed");
        assert_eq!(
            after["payload"]["daemon"]["lastLaunchError"],
            "Helper process was terminated by SIGKILL (Code Signature Constraint Violation)."
        );
        assert_eq!(after["payload"]["session"]["authenticated"], false);
    });
}
