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

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("rust crate should live under repo root")
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

fn write_supported_domains_config(home: &Path, domains: &[&str]) {
    let config = serde_json::json!({
        "schema": 1,
        "port": 10_000,
        "host": "127.0.0.1",
        "username": "",
        "sharedKey": "",
        "runtimeMode": "auto",
        "secretSource": "file",
        "supportedDomains": domains,
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
fn threat_model_documents_current_v2_security_boundary() {
    let threat_model = fs::read_to_string(repo_root().join("docs/THREAT_MODEL.md"))
        .expect("failed to read threat model");
    let posture = fs::read_to_string(repo_root().join("docs/SECURITY_POSTURE_AND_TESTING.md"))
        .expect("failed to read security posture doc");

    for required in [
        "AuthenticationServices",
        "associated-domain",
        "external fallback",
        "UNIX socket",
        "Retired surfaces",
        "supported v2 credential-broker boundary",
        "Security regression map",
        "AppleScript and Shortcuts/AppIntents automation requests",
        "prompt fatigue",
        "not sandboxed",
        "App Sandbox entitlements",
    ] {
        assert!(
            threat_model.contains(required),
            "threat model should document {required}"
        );
    }

    for retired in [
        "UDP listener attack surface",
        "Browser-extension trust boundary",
        "Apple's private-helper launch path",
    ] {
        assert!(
            !threat_model.contains(retired),
            "threat model should not describe retired legacy surface `{retired}` as current"
        );
    }

    assert!(
        posture.contains("threat-model drift checks"),
        "security posture should list the threat-model drift regression"
    );
    assert!(
        posture.contains("without the App") && posture.contains("Sandbox entitlement"),
        "security posture should document the current unsandboxed automation surface"
    );
    assert!(
        posture.contains("rate limiting/coalescing"),
        "security posture should track prompt-fatigue follow-up for automation"
    );
}

#[test]
#[serial]
fn doctor_ci_reports_unreachable_supported_domain_from_config() {
    with_temp_home(|home| {
        env::remove_var("APW_AASA_DOMAINS");
        write_supported_domains_config(home, &["definitely-not-a-real-host.invalid"]);

        let (status, stdout, stderr) = run_command(home, &["doctor", "--ci"]);

        assert_eq!(
            status, 0,
            "status={status}, stdout={stdout}, stderr={stderr}"
        );
        let output = parse_json_output(&stdout);
        let checks = output["payload"].as_array().expect("expected checks array");
        let associated_domains = checks
            .iter()
            .find(|check| check["name"] == "associated-domains")
            .expect("expected associated-domains check");
        assert_eq!(associated_domains["status"], "fail");
        assert!(associated_domains["message"]
            .as_str()
            .unwrap_or_default()
            .contains("definitely-not-a-real-host.invalid"));
    });
}

#[test]
#[serial]
fn login_invalid_url_rejected_before_broker_dependency() {
    with_temp_home(|home| {
        let (status, stdout, stderr) = run_command(home, &["--json", "login", "ftp://example.com"]);
        assert_eq!(
            status, 2,
            "status={status}, stdout={stdout}, stderr={stderr}"
        );
        let output = parse_json_output(&stderr);
        assert_eq!(output["code"], 2);
        assert_eq!(output["ok"], false);
        assert!(output["error"].as_str().unwrap_or("").contains("https URL"));
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
        assert_eq!(output["payload"]["installed"], false);
        assert!(output["payload"]["bundlePath"].is_string());
        assert!(output["payload"]["executablePath"].is_string());
        assert!(output["payload"]["socketPath"].is_string());
        assert!(output["payload"]["credentialsPath"].is_string());
        assert!(output["payload"]["brokerLogPath"].is_string());
        assert_eq!(output["payload"]["externalFallback"]["configured"], false);
        assert_eq!(
            output["payload"]["externalFallback"]["loginFlag"],
            "--external-fallback"
        );
        assert_eq!(
            output["payload"]["service"]["transportContract"],
            "typed_json_envelope"
        );
        assert!(output["payload"]["service"]["requestTimeoutMs"].is_u64());
        assert!(output["payload"]["daemon"].is_null());
        assert!(output["payload"]["host"].is_null());
        assert!(output["payload"]["bridge"].is_null());
        assert!(output["payload"]["session"].is_null());
    });
}

#[test]
#[serial]
fn login_rejects_relative_external_provider_path() {
    with_temp_home(|home| {
        install_native_app_no_results(home);
        write_fallback_provider_config(home, "bw");

        let (status, stdout, stderr) = run_command(
            home,
            &[
                "--json",
                "login",
                "--external-fallback",
                "https://vault.example.com",
            ],
        );

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

        let (status, stdout, stderr) = run_command(
            home,
            &[
                "--json",
                "login",
                "--external-fallback",
                "https://vault.example.com",
            ],
        );

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

        let (status, stdout, stderr) = run_command(
            home,
            &[
                "--json",
                "login",
                "--external-fallback",
                "https://vault.example.com",
            ],
        );

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

        let (status, stdout, stderr) = run_command(
            home,
            &[
                "--json",
                "login",
                "--external-fallback",
                "https://vault.example.com",
            ],
        );

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
fn status_json_reports_native_app_surface_after_command_failure() {
    with_temp_home(|home| {
        let (status, stdout, stderr) = run_command(home, &["status", "--json"]);
        assert_eq!(
            status, 0,
            "status={status}, stdout={stdout}, stderr={stderr}"
        );
        let initial = parse_json_output(&stdout);
        assert!(initial["payload"]["bundlePath"].is_string());
        assert_eq!(initial["payload"]["service"]["transport"], "unix_socket");
        assert_eq!(
            initial["payload"]["service"]["transportContract"],
            "typed_json_envelope"
        );

        let (login_status, login_stdout, login_stderr) =
            run_command(home, &["--json", "login", "ftp://example.com"]);
        assert_eq!(
            login_status, 2,
            "status={login_status}, stdout={login_stdout}, stderr={login_stderr}"
        );

        let (status_after, stdout_after, stderr_after) = run_command(home, &["status", "--json"]);
        assert_eq!(
            status_after, 0,
            "status={status_after}, stdout={stdout_after}, stderr={stderr_after}"
        );
        let after = parse_json_output(&stdout_after);
        assert!(after["payload"]["daemon"].is_null());
        assert_eq!(after["payload"]["service"]["transport"], "unix_socket");
        assert_eq!(
            after["payload"]["externalFallback"]["loginFlag"],
            "--external-fallback"
        );
    });
}
