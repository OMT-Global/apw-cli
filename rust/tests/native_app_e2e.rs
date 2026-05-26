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
            "error": "Invalid URL for native app credential request.",
        }
    if parsed.scheme.lower() != "https":
        return {
            "ok": False,
            "code": 1,
            "error": "Native app credential requests require https URLs.",
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
            "intent": "login",
            "url": credential["url"],
            "domain": credential["domain"],
            "username": credential["username"],
            "password": credential["password"],
            "transport": transport,
            "userMediated": True,
        },
    }


def fill_payload(raw_url, transport):
    envelope = login_payload(raw_url, transport)
    if envelope.get("ok"):
        envelope["payload"]["intent"] = "fill"
    return envelope


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
    if command == "fill":
        return fill_payload((payload or {}).get("url", ""), transport)
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
    if mode == "flood":
        # Emit far more than MAX_MESSAGE_BYTES (32 KiB) without ever closing
        # stdout to a sentinel. The bounded read in the Rust CLI must cap
        # this without growing past the configured limit. See issue #42.
        chunk = b"A" * 4096
        for _ in range(64):
            sys.stdout.buffer.write(chunk)
            sys.stdout.buffer.flush()
        return True
    if mode == "hang":
        # Sleep past the direct-exec timeout to force the bounded helper to
        # terminate the child. The CLI must surface a CommunicationTimeout
        # rather than block. See issue #42.
        time.sleep(30)
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
    os.chmod(sock_path, FILE_MODE)
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
    let contents = workspace
        .join("native-app")
        .join("dist")
        .join("APW.app")
        .join("Contents");
    create_fake_bundle_contents(&contents);
}

fn create_fake_archive_bundle(workspace: &Path) {
    create_fake_bundle_contents(&workspace.join("APW.app").join("Contents"));
}

fn create_fake_bundle_contents(contents: &Path) {
    let macos = contents.join("MacOS");
    fs::create_dir_all(&macos).expect("failed to create fake app bundle");

    let executable = macos.join("APW");
    fs::write(&executable, FAKE_APP_SCRIPT).expect("failed to write fake app executable");
    let mut permissions = fs::metadata(&executable)
        .expect("failed to stat fake app executable")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&executable, permissions).expect("failed to chmod fake app executable");

    let info_plist = contents.join("Info.plist");
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
            if payload["payload"]["service"]["running"] == true {
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
        if payload["payload"]["service"]["live"]["transport"] == "unix_socket" {
            return payload;
        }
        thread::sleep(Duration::from_millis(200));
    }

    let payload = wait_for_status(fixture);
    assert_eq!(
        payload["payload"]["service"]["live"]["transport"], "unix_socket",
        "{payload:#?}"
    );
    payload
}

#[test]
#[serial]
fn doctor_bootstraps_runtime_without_installed_bundle() {
    let fixture = NativeAppFixture::new();
    let credentials_path = fixture.home().join(".apw/native-app/credentials.json");

    let output = run_apw(&fixture, &["--json", "doctor"], &[]);
    assert_eq!(output.status, 0, "{output:#?}");

    let payload = parse_success(&output);
    assert_eq!(payload["ok"], true);
    assert_eq!(payload["payload"]["installed"], false);
    assert_eq!(
        payload["payload"]["frameworks"]["authenticationServicesLinked"],
        true
    );
    assert!(!credentials_path.exists());

    let demo_output = run_apw(&fixture, &["--json", "doctor"], &[("APW_DEMO", "1")]);
    assert_eq!(demo_output.status, 0, "{demo_output:#?}");
    assert!(credentials_path.exists());
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
    assert_eq!(status_payload["payload"]["installed"], true);
    assert_eq!(status_payload["payload"]["bundleVersion"], "2.0.0");
    assert_eq!(status_payload["payload"]["service"]["running"], false);
}

#[test]
#[serial]
fn app_install_finds_release_archive_sibling_bundle() {
    let home = TempDir::new().expect("failed to create temp home");
    let archive = TempDir::new().expect("failed to create temp archive");
    let archive_apw = archive.path().join("apw");
    fs::copy(apw_path(), &archive_apw).expect("failed to copy apw binary into archive fixture");
    let mut permissions = fs::metadata(&archive_apw)
        .expect("failed to stat copied apw binary")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&archive_apw, permissions).expect("failed to chmod copied apw binary");
    create_fake_archive_bundle(archive.path());

    let output = Command::new(&archive_apw)
        .current_dir(archive.path())
        .env("HOME", home.path())
        .env("NO_COLOR", "1")
        .args(["--json", "app", "install"])
        .output()
        .expect("failed to run apw app install from archive fixture");
    let install = CommandOutput {
        status: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).trim().to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
    };
    assert_eq!(install.status, 0, "{install:#?}");

    let payload = parse_success(&install);
    assert_eq!(payload["payload"]["status"], "installed");
    assert_eq!(payload["payload"]["version"], "2.0.0");
    assert!(home
        .path()
        .join(".apw/native-app/installed/APW.app/Contents/MacOS/APW")
        .exists());
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
    assert_eq!(status_payload["payload"]["service"]["running"], true);
    assert_eq!(
        status_payload["payload"]["service"]["live"]["serviceStatus"],
        "running"
    );
    assert_eq!(
        status_payload["payload"]["service"]["live"]["transport"],
        "unix_socket"
    );

    let login = run_apw(&fixture, &["--json", "login", "https://example.com"], &[]);
    assert_eq!(login.status, 0, "{login:#?}");
    let login_payload = parse_success(&login);
    assert_eq!(login_payload["payload"]["status"], "approved");
    assert_eq!(login_payload["payload"]["domain"], "example.com");
    assert_eq!(login_payload["payload"]["username"], "demo@example.com");
    assert_eq!(login_payload["payload"]["password"], "apw-demo-password");
    assert_eq!(login_payload["payload"]["intent"], "login");
    assert_eq!(login_payload["payload"]["transport"], "unix_socket");
}

#[test]
#[serial]
fn fill_uses_fill_intent_over_socket() {
    let fixture = NativeAppFixture::new();

    let install = run_apw(&fixture, &["--json", "app", "install"], &[]);
    assert_eq!(install.status, 0, "{install:#?}");

    let launch = run_apw(&fixture, &["--json", "app", "launch"], &[]);
    assert_eq!(launch.status, 0, "{launch:#?}");
    wait_for_socket_transport(&fixture);

    let fill = run_apw(&fixture, &["--json", "fill", "https://example.com"], &[]);
    assert_eq!(fill.status, 0, "{fill:#?}");
    let payload = parse_success(&fill);
    assert_eq!(payload["payload"]["intent"], "fill");
    assert_eq!(payload["payload"]["domain"], "example.com");
    assert_eq!(payload["payload"]["transport"], "unix_socket");
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
    assert_eq!(payload["payload"]["intent"], "login");
    assert_eq!(payload["payload"]["userMediated"], true);
}

#[test]
#[serial]
fn fill_works_via_direct_fallback_when_service_not_running() {
    let fixture = NativeAppFixture::new();

    let install = run_apw(&fixture, &["--json", "app", "install"], &[]);
    assert_eq!(install.status, 0, "{install:#?}");

    let fill = run_apw(&fixture, &["--json", "fill", "https://example.com"], &[]);
    assert_eq!(fill.status, 0, "{fill:#?}");
    let payload = parse_success(&fill);
    assert_eq!(payload["payload"]["transport"], "direct_exec");
    assert_eq!(payload["payload"]["intent"], "fill");
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
fn native_credential_commands_reject_non_https_urls() {
    let fixture = NativeAppFixture::new();

    let install = run_apw(&fixture, &["--json", "app", "install"], &[]);
    assert_eq!(install.status, 0, "{install:#?}");

    for command in ["login", "fill"] {
        let result = run_apw(&fixture, &["--json", command, "ftp://example.com"], &[]);
        assert_eq!(result.status, 2, "{result:#?}");
        let payload = parse_error(&result);
        assert!(payload["error"]
            .as_str()
            .unwrap_or_default()
            .contains("require an https URL"));
    }
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

#[test]
#[serial]
fn doctor_bundle_writes_deterministic_redacted_archive() {
    // Issue #56: --bundle writes a tar.gz with a stable layout and never
    // includes credentials.json contents. We populate
    // ~/.apw/native-app/credentials.json with plausible secret material
    // and assert that the secret never appears anywhere in the archive.
    let fixture = NativeAppFixture::new();
    let runtime = fixture.home().join(".apw/native-app");
    fs::create_dir_all(&runtime).expect("failed to create runtime");
    fs::write(
        runtime.join("credentials.json"),
        r#"{"secret":"apw-redaction-sentinel-must-never-leak","domain":"example.com"}"#,
    )
    .expect("failed to write credentials");
    let mut perms = fs::metadata(runtime.join("credentials.json"))
        .unwrap()
        .permissions();
    perms.set_mode(0o600);
    fs::set_permissions(runtime.join("credentials.json"), perms).unwrap();

    let bundle_dir = TempDir::new().expect("failed to create bundle output dir");
    let bundle_path = bundle_dir.path().join("apw-doctor-bundle.tar.gz");

    let output = run_apw(
        &fixture,
        &[
            "--json",
            "doctor",
            "--bundle",
            bundle_path.to_str().unwrap(),
        ],
        &[],
    );
    assert_eq!(output.status, 0, "{output:#?}");

    let payload = parse_success(&output);
    assert_eq!(
        payload["payload"]["bundlePath"],
        serde_json::Value::String(bundle_path.display().to_string())
    );
    let files: Vec<String> = payload["payload"]["filesIncluded"]
        .as_array()
        .expect("filesIncluded array")
        .iter()
        .map(|v| v.as_str().unwrap_or_default().to_string())
        .collect();
    let expected = [
        "manifest.json",
        "doctor.json",
        "environment.json",
        "os.json",
        "native-app/file-listing.json",
    ];
    for name in expected {
        assert!(
            files.iter().any(|f| f == name),
            "expected {name} in {files:?}"
        );
    }
    assert!(
        payload["payload"]["redactionChecks"]
            .as_u64()
            .unwrap_or_default()
            >= 10
    );

    assert!(bundle_path.exists(), "bundle file should exist");
    let mode = fs::metadata(&bundle_path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "bundle file should be 0600");

    let extract_dir = TempDir::new().expect("failed to create extract dir");
    let extract_status = Command::new("tar")
        .arg("-xzf")
        .arg(&bundle_path)
        .arg("-C")
        .arg(extract_dir.path())
        .status()
        .expect("failed to run tar");
    assert!(extract_status.success(), "tar extraction failed");

    let bundle_root = extract_dir.path().join("apw-doctor-bundle");
    assert!(bundle_root.is_dir(), "expected bundle root dir");
    for name in expected {
        let p = bundle_root.join(name);
        assert!(p.exists(), "expected bundle to contain {name}");
    }

    let mut accumulator: Vec<u8> = Vec::new();
    fn collect_bytes(path: &Path, acc: &mut Vec<u8>) {
        if path.is_file() {
            if let Ok(b) = fs::read(path) {
                acc.extend_from_slice(&b);
            }
        } else if path.is_dir() {
            for entry in fs::read_dir(path).into_iter().flatten().flatten() {
                collect_bytes(&entry.path(), acc);
            }
        }
    }
    collect_bytes(&bundle_root, &mut accumulator);
    let combined = String::from_utf8_lossy(&accumulator);
    assert!(
        !combined.contains("apw-redaction-sentinel-must-never-leak"),
        "credentials.json contents must not appear in the bundle"
    );

    let listing: Value = serde_json::from_slice(
        &fs::read(bundle_root.join("native-app/file-listing.json")).unwrap(),
    )
    .unwrap();
    let entries = listing["entries"].as_array().expect("entries array");
    let has_credentials = entries
        .iter()
        .any(|e| e["path"].as_str() == Some("credentials.json"));
    assert!(
        has_credentials,
        "file listing must reference credentials.json by name"
    );
    let credentials_entry = entries
        .iter()
        .find(|e| e["path"].as_str() == Some("credentials.json"))
        .unwrap();
    assert_eq!(credentials_entry["type"], "file");
    assert!(credentials_entry["size"].as_u64().unwrap_or(0) > 0);
}

#[test]
#[serial]
fn doctor_ci_and_bundle_fail_before_writing_archive() {
    // Issue #56 review follow-up: the operator-facing CLI must not accept
    // `--ci --bundle` and then return before writing the requested archive.
    let fixture = NativeAppFixture::new();
    let bundle_dir = TempDir::new().expect("failed to create bundle output dir");
    let bundle_path = bundle_dir.path().join("should-not-exist.tar.gz");

    let output = run_apw(
        &fixture,
        &[
            "--json",
            "doctor",
            "--ci",
            "--bundle",
            bundle_path.to_str().unwrap(),
        ],
        &[],
    );

    assert_ne!(output.status, 0, "expected clap conflict, got {output:#?}");
    assert!(
        output.stderr.contains("cannot be used with")
            || output.stderr.contains("cannot be used at the same time"),
        "expected conflict error, got: {}",
        output.stderr
    );
    assert!(
        !bundle_path.exists(),
        "bundle file must not exist when --ci conflicts with --bundle"
    );
}

#[test]
#[serial]
fn direct_fallback_bounds_oversized_stdout() {
    // Issue #42: a fallback executable that streams unbounded output must
    // be capped before the CLI buffers the full payload. Previously the
    // CLI used Command::output() and only checked the length after the
    // child exited.
    let fixture = NativeAppFixture::new();

    let install = run_apw(&fixture, &["--json", "app", "install"], &[]);
    assert_eq!(install.status, 0, "{install:#?}");

    let flood = run_apw(
        &fixture,
        &["--json", "login", "https://example.com"],
        &[("APW_FAKE_DIRECT_RESPONSE", "flood")],
    );
    assert_eq!(flood.status, 104, "{flood:#?}");
    let payload = parse_error(&flood);
    let error = payload["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("too large"),
        "expected oversize error, got {error}"
    );
}

#[test]
#[serial]
fn doctor_bundle_fails_closed_when_diagnostic_field_looks_secret_like() {
    // Issue #56: if any diagnostic string looks token-like, the bundle
    // must abort instead of shipping incompletely-redacted material. We
    // inject the sentinel via APW_RUNNER_LABELS, which flows verbatim
    // into the CI runner-labels diagnostic.
    let fixture = NativeAppFixture::new();
    let bundle_dir = TempDir::new().expect("failed to create bundle output dir");
    let bundle_path = bundle_dir.path().join("should-not-exist.tar.gz");

    let output = run_apw(
        &fixture,
        &[
            "--json",
            "doctor",
            "--bundle",
            bundle_path.to_str().unwrap(),
        ],
        &[
            ("CI", "true"),
            ("APW_RUNNER_LABELS", "apw-demo-password-leaked-into-labels"),
        ],
    );
    assert_ne!(output.status, 0, "expected failure, got {output:#?}");

    let err = parse_error(&output);
    let message = err["error"].as_str().unwrap_or_default();
    assert!(
        message.contains("Aborting bundle"),
        "expected fail-closed message, got: {message}"
    );
    assert!(
        !bundle_path.exists(),
        "bundle file must not exist after fail-closed abort"
    );
}

#[test]
#[serial]
fn direct_fallback_terminates_hung_child() {
    // Issue #42: a fallback executable that never exits must be terminated
    // by the CLI rather than blocking it forever. The bounded helper
    // surfaces a CommunicationTimeout (code 101).
    let fixture = NativeAppFixture::new();

    let install = run_apw(&fixture, &["--json", "app", "install"], &[]);
    assert_eq!(install.status, 0, "{install:#?}");

    let started = std::time::Instant::now();
    let hung = run_apw(
        &fixture,
        &["--json", "login", "https://example.com"],
        &[("APW_FAKE_DIRECT_RESPONSE", "hang")],
    );
    let elapsed = started.elapsed();

    assert_eq!(hung.status, 101, "{hung:#?}");
    let payload = parse_error(&hung);
    let error = payload["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("did not respond"),
        "expected timeout error, got {error}"
    );
    // The fake app sleeps 30s; the CLI must terminate it well before that.
    assert!(
        elapsed < Duration::from_secs(15),
        "expected timeout within 15s, took {elapsed:?}"
    );
}
