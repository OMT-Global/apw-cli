use libc::{kill, SIGTERM};
use serde_json::Value;
use serial_test::serial;
use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;
use tempfile::TempDir;

const FAKE_APP_SCRIPT: &str = r#"#!/usr/bin/env python3
import json
import os
import pathlib
import socket
import sys
import time
import urllib.parse

RUNTIME_MODE = 0o700
FILE_MODE = 0o600
SOCKET_NAME = "broker.sock"
STATUS_NAME = "status.json"
CREDENTIALS_NAME = "credentials.json"
VERSION = "2.0.0"


def runtime_root():
    home = os.environ.get("HOME") or os.path.expanduser("~")
    return pathlib.Path(home) / ".apw" / "native-app"


def socket_path():
    return runtime_root() / SOCKET_NAME


def status_path():
    return runtime_root() / STATUS_NAME


def credentials_path():
    return runtime_root() / CREDENTIALS_NAME


def ensure_runtime():
    root = runtime_root()
    root.mkdir(parents=True, exist_ok=True)
    os.chmod(root, RUNTIME_MODE)


def ensure_credentials():
    path = credentials_path()
    if path.exists():
        return
    payload = {
        "domains": ["example.com"],
        "credentials": [
            {
                "domain": "example.com",
                "url": "https://example.com",
                "username": "demo@example.com",
                "password": "apw-demo-password",
            }
        ],
    }
    path.write_text(json.dumps(payload), encoding="utf-8")
    os.chmod(path, FILE_MODE)


def load_credentials():
    ensure_credentials()
    return json.loads(credentials_path().read_text(encoding="utf-8"))


def status_payload(transport):
    return {
        "serviceStatus": "running",
        "startedAt": "2026-01-01T00:00:00Z",
        "transport": transport,
        "bundleVersion": VERSION,
        "socketPath": str(socket_path()),
        "supportedDomains": load_credentials()["domains"],
        "authenticationServicesLinked": True,
        "pid": os.getpid(),
    }


def login_payload(raw_url, transport):
    parsed = urllib.parse.urlparse(raw_url)
    host = (parsed.hostname or "").lower()
    if not host:
        return {
            "ok": False,
            "code": 1,
            "error": "Invalid URL for native app login.",
        }
    if host != "example.com":
        return {
            "ok": False,
            "code": 3,
            "error": "The APW v2 bootstrap app currently supports only https://example.com.",
        }
    if os.environ.get("APW_FAKE_DENY") == "1":
        return {
            "ok": False,
            "code": 1,
            "error": "User denied the APW login request.",
        }

    credentials = load_credentials()["credentials"]
    credential = next((entry for entry in credentials if entry["domain"] == host), None)
    if credential is None:
        return {
            "ok": False,
            "code": 3,
            "error": f"No bootstrap credential is configured for {host}.",
        }
    return {
        "ok": True,
        "code": 0,
        "payload": {
            "status": "approved",
            "url": credential["url"],
            "domain": credential["domain"],
            "username": credential["username"],
            "password": credential["password"],
            "transport": transport,
            "userMediated": True,
        },
    }


def dispatch(command, payload, transport):
    if command == "status":
        return {"ok": True, "code": 0, "payload": status_payload(transport)}
    if command == "doctor":
        return {
            "ok": True,
            "code": 0,
            "payload": {
                "app": {
                    "bundleVersion": VERSION,
                    "bundlePath": str(pathlib.Path(__file__).resolve().parents[2]),
                    "lsuiElement": True,
                },
                "broker": status_payload(transport),
                "credentialsPath": str(credentials_path()),
                "guidance": [
                    "Run `apw login https://example.com` to exercise the bootstrap credential flow."
                ],
            },
        }
    if command == "login":
        return login_payload((payload or {}).get("url", ""), transport)
    return {"ok": False, "code": 1, "error": f"Unsupported native app command: {command}"}


def maybe_emit_direct_override():
    mode = os.environ.get("APW_FAKE_DIRECT_RESPONSE")
    if mode == "invalid_json":
        sys.stdout.write("{not-json")
        sys.stdout.flush()
        return True
    if mode == "missing_payload":
        sys.stdout.write(json.dumps({"ok": True, "code": 0}))
        sys.stdout.flush()
        return True
    return False


def write_status(payload):
    path = status_path()
    path.write_text(json.dumps(payload), encoding="utf-8")
    os.chmod(path, FILE_MODE)


def handle_request():
    if maybe_emit_direct_override():
        return 0
    command = sys.argv[2] if len(sys.argv) > 2 else ""
    payload = json.loads(sys.argv[3]) if len(sys.argv) > 3 else {}
    envelope = dispatch(command, payload, "direct_exec")
    envelope["requestId"] = "oneshot"
    sys.stdout.write(json.dumps(envelope))
    sys.stdout.write("\n")
    sys.stdout.flush()
    return 0


def handle_client(connection):
    data = b""
    while True:
        chunk = connection.recv(4096)
        if not chunk:
            break
        data += chunk
    if not data:
        return
    request = json.loads(data.decode("utf-8"))
    envelope = dispatch(request.get("command", ""), request.get("payload", {}), "unix_socket")
    envelope["requestId"] = request.get("requestId")
    connection.sendall(json.dumps(envelope).encode("utf-8"))


def serve():
    ensure_runtime()
    ensure_credentials()
    sock_path = socket_path()
    if sock_path.exists():
        sock_path.unlink()

    server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    server.bind(str(sock_path))
    server.listen(8)
    server.settimeout(1.0)
    write_status(
        {
            "serviceStatus": "running",
            "pid": os.getpid(),
            "transport": "unix_socket",
            "bundleVersion": VERSION,
            "socketPath": str(sock_path),
        }
    )

    deadline = time.time() + 20
    while time.time() < deadline:
        try:
            connection, _ = server.accept()
        except socket.timeout:
            continue
        with connection:
            handle_client(connection)
    server.close()
    if sock_path.exists():
        sock_path.unlink()
    return 0


def main():
    command = sys.argv[1] if len(sys.argv) > 1 else "serve"
    if command == "serve":
        return serve()
    if command == "request":
        ensure_runtime()
        ensure_credentials()
        return handle_request()
    sys.stderr.write(f"Unsupported APW app command: {command}\n")
    return 1


if __name__ == "__main__":
    sys.exit(main())
"#;

#[derive(Debug)]
struct CommandOutput {
    status: i32,
    stdout: String,
    stderr: String,
}

struct NativeAppFixture {
    home: TempDir,
    workspace: TempDir,
}

impl NativeAppFixture {
    fn new() -> Self {
        let home = TempDir::new().expect("failed to create temp home");
        let workspace = TempDir::new().expect("failed to create temp workspace");
        create_fake_bundle(workspace.path());
        Self { home, workspace }
    }

    fn home(&self) -> &Path {
        self.home.path()
    }

    fn workspace(&self) -> &Path {
        self.workspace.path()
    }
}

impl Drop for NativeAppFixture {
    fn drop(&mut self) {
        if let Ok(content) = fs::read_to_string(
            self.home()
                .join(".apw")
                .join("native-app")
                .join("status.json"),
        ) {
            if let Ok(payload) = serde_json::from_str::<Value>(&content) {
                if let Some(pid) = payload.get("pid").and_then(Value::as_i64) {
                    unsafe {
                        kill(pid as i32, SIGTERM);
                    }
                    thread::sleep(Duration::from_millis(100));
                }
            }
        }
    }
}

fn create_fake_bundle(workspace: &Path) {
    let bundle = workspace
        .join("native-app")
        .join("dist")
        .join("APW.app")
        .join("Contents");
    let macos = bundle.join("MacOS");
    fs::create_dir_all(&macos).expect("failed to create fake app bundle");

    let executable = macos.join("APW");
    fs::write(&executable, FAKE_APP_SCRIPT).expect("failed to write fake app executable");
    let mut permissions = fs::metadata(&executable)
        .expect("failed to stat fake app executable")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&executable, permissions).expect("failed to chmod fake app executable");

    let info_plist = bundle.join("Info.plist");
    fs::write(
        info_plist,
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key>
  <string>APW</string>
  <key>CFBundleIdentifier</key>
  <string>dev.omt.apw</string>
  <key>CFBundleName</key>
  <string>APW</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>2.0.0</string>
  <key>CFBundleVersion</key>
  <string>2.0.0</string>
  <key>LSUIElement</key>
  <true/>
</dict>
</plist>
"#,
    )
    .expect("failed to write fake app Info.plist");
}

fn apw_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_apw"))
}

fn run_apw(fixture: &NativeAppFixture, args: &[&str], extra_env: &[(&str, &str)]) -> CommandOutput {
    let mut command = Command::new(apw_path());
    command
        .current_dir(fixture.workspace())
        .env("HOME", fixture.home())
        .env("NO_COLOR", "1")
        .args(args);

    for (key, value) in extra_env {
        command.env(key, value);
    }

    let output = command.output().expect("failed to run apw");
    CommandOutput {
        status: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).trim().to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
    }
}

fn parse_success(output: &CommandOutput) -> Value {
    serde_json::from_str(&output.stdout)
        .unwrap_or_else(|_| panic!("expected success JSON, got {}", output.stdout))
}

fn parse_error(output: &CommandOutput) -> Value {
    serde_json::from_str(&output.stderr)
        .unwrap_or_else(|_| panic!("expected error JSON, got {}", output.stderr))
}

fn wait_for_status(fixture: &NativeAppFixture) -> Value {
    for _ in 0..20 {
        let status = run_apw(fixture, &["status", "--json"], &[]);
        if status.status == 0 {
            let payload = parse_success(&status);
            if payload["payload"]["app"]["service"]["running"] == true {
                return payload;
            }
        }
        thread::sleep(Duration::from_millis(200));
    }

    let status = run_apw(fixture, &["status", "--json"], &[]);
    assert_eq!(status.status, 0, "{status:#?}");
    parse_success(&status)
}

fn wait_for_socket_transport(fixture: &NativeAppFixture) -> Value {
    for _ in 0..20 {
        let payload = wait_for_status(fixture);
        if payload["payload"]["app"]["service"]["live"]["transport"] == "unix_socket" {
            return payload;
        }
        thread::sleep(Duration::from_millis(200));
    }

    let payload = wait_for_status(fixture);
    assert_eq!(
        payload["payload"]["app"]["service"]["live"]["transport"], "unix_socket",
        "{payload:#?}"
    );
    payload
}

#[test]
#[serial]
fn doctor_bootstraps_runtime_without_installed_bundle() {
    let fixture = NativeAppFixture::new();

    let output = run_apw(&fixture, &["--json", "doctor"], &[]);
    assert_eq!(output.status, 0, "{output:#?}");

    let payload = parse_success(&output);
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["payload"]["installed"], false);
    assert_eq!(
        payload["payload"]["frameworks"]["authenticationServicesLinked"],
        true
    );
    assert!(fixture
        .home()
        .join(".apw/native-app/credentials.json")
        .exists());
}

#[test]
#[serial]
fn app_install_copies_packaged_bundle_and_updates_status() {
    let fixture = NativeAppFixture::new();

    let install = run_apw(&fixture, &["--json", "app", "install"], &[]);
    assert_eq!(install.status, 0, "{install:#?}");

    let payload = parse_success(&install);
    assert_eq!(payload["payload"]["status"], "installed");
    assert_eq!(payload["payload"]["version"], "2.0.0");
    assert_eq!(payload["payload"]["doctor"]["installed"], true);
    assert!(fixture
        .home()
        .join(".apw/native-app/installed/APW.app/Contents/MacOS/APW")
        .exists());

    let status_payload = wait_for_status(&fixture);
    assert_eq!(status_payload["payload"]["app"]["installed"], true);
    assert_eq!(status_payload["payload"]["app"]["bundleVersion"], "2.0.0");
    assert_eq!(
        status_payload["payload"]["app"]["service"]["running"],
        false
    );
}

#[test]
#[serial]
fn launch_status_and_login_work_over_socket() {
    let fixture = NativeAppFixture::new();

    let install = run_apw(&fixture, &["--json", "app", "install"], &[]);
    assert_eq!(install.status, 0, "{install:#?}");

    let launch = run_apw(&fixture, &["--json", "app", "launch"], &[]);
    assert_eq!(launch.status, 0, "{launch:#?}");
    let launch_payload = parse_success(&launch);
    assert!(
        launch_payload["payload"]["status"] == "launched"
            || launch_payload["payload"]["status"] == "starting"
    );

    let status_payload = wait_for_socket_transport(&fixture);
    assert_eq!(status_payload["payload"]["app"]["service"]["running"], true);
    assert_eq!(
        status_payload["payload"]["app"]["service"]["live"]["serviceStatus"],
        "running"
    );
    assert_eq!(
        status_payload["payload"]["app"]["service"]["live"]["transport"],
        "unix_socket"
    );

    let login = run_apw(&fixture, &["--json", "login", "https://example.com"], &[]);
    assert_eq!(login.status, 0, "{login:#?}");
    let login_payload = parse_success(&login);
    assert_eq!(login_payload["payload"]["status"], "approved");
    assert_eq!(login_payload["payload"]["domain"], "example.com");
    assert_eq!(login_payload["payload"]["username"], "demo@example.com");
    assert_eq!(login_payload["payload"]["password"], "apw-demo-password");
    assert_eq!(login_payload["payload"]["transport"], "unix_socket");
}

#[test]
#[serial]
fn login_works_via_direct_fallback_when_service_not_running() {
    let fixture = NativeAppFixture::new();

    let install = run_apw(&fixture, &["--json", "app", "install"], &[]);
    assert_eq!(install.status, 0, "{install:#?}");

    let login = run_apw(&fixture, &["--json", "login", "https://example.com"], &[]);
    assert_eq!(login.status, 0, "{login:#?}");
    let payload = parse_success(&login);
    assert_eq!(payload["payload"]["transport"], "direct_exec");
    assert_eq!(payload["payload"]["userMediated"], true);
}

#[test]
#[serial]
fn login_reports_operator_facing_failures() {
    let fixture = NativeAppFixture::new();

    let not_installed = run_apw(&fixture, &["--json", "login", "https://example.com"], &[]);
    assert_eq!(not_installed.status, 103, "{not_installed:#?}");
    let not_installed_payload = parse_error(&not_installed);
    assert!(not_installed_payload["error"]
        .as_str()
        .unwrap_or_default()
        .contains("Run `apw app install` first."));

    let install = run_apw(&fixture, &["--json", "app", "install"], &[]);
    assert_eq!(install.status, 0, "{install:#?}");

    let unsupported = run_apw(
        &fixture,
        &["--json", "login", "https://unsupported.example"],
        &[],
    );
    assert_eq!(unsupported.status, 3, "{unsupported:#?}");
    let unsupported_payload = parse_error(&unsupported);
    assert!(unsupported_payload["error"]
        .as_str()
        .unwrap_or_default()
        .contains("supports only https://example.com"));

    let denied = run_apw(
        &fixture,
        &["--json", "login", "https://example.com"],
        &[("APW_FAKE_DENY", "1")],
    );
    assert_eq!(denied.status, 1, "{denied:#?}");
    let denied_payload = parse_error(&denied);
    assert!(denied_payload["error"]
        .as_str()
        .unwrap_or_default()
        .contains("User denied"));
}

#[test]
#[serial]
fn direct_fallback_maps_malformed_response_to_proto_invalid_response() {
    let fixture = NativeAppFixture::new();

    let install = run_apw(&fixture, &["--json", "app", "install"], &[]);
    assert_eq!(install.status, 0, "{install:#?}");

    let invalid_json = run_apw(
        &fixture,
        &["--json", "login", "https://example.com"],
        &[("APW_FAKE_DIRECT_RESPONSE", "invalid_json")],
    );
    assert_eq!(invalid_json.status, 104, "{invalid_json:#?}");
    let invalid_json_payload = parse_error(&invalid_json);
    assert!(invalid_json_payload["error"]
        .as_str()
        .unwrap_or_default()
        .contains("not valid JSON"));

    let missing_payload = run_apw(
        &fixture,
        &["--json", "login", "https://example.com"],
        &[("APW_FAKE_DIRECT_RESPONSE", "missing_payload")],
    );
    assert_eq!(missing_payload.status, 104, "{missing_payload:#?}");
    let missing_payload_error = parse_error(&missing_payload);
    assert!(missing_payload_error["error"]
        .as_str()
        .unwrap_or_default()
        .contains("missing its payload"));
}
