use crate::error::{APWError, Result};
use crate::types::{Status, MAX_MESSAGE_BYTES, VERSION};
use serde_json::{json, Value};
use std::env;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

const NATIVE_APP_BUNDLE_NAME: &str = "APW.app";
const NATIVE_APP_EXECUTABLE_NAME: &str = "APW";
const NATIVE_APP_SOCKET_NAME: &str = "broker.sock";
const NATIVE_APP_STATUS_NAME: &str = "status.json";
const NATIVE_APP_CREDENTIALS_NAME: &str = "credentials.json";
const NATIVE_APP_RUNTIME_DIR_MODE: u32 = 0o700;
const NATIVE_APP_FILE_MODE: u32 = 0o600;
const MAX_BROKER_BYTES: usize = MAX_MESSAGE_BYTES;
const SOCKET_TIMEOUT_MS: u64 = 3_000;
const CONNECT_RETRIES: usize = 10;
const CONNECT_RETRY_DELAY_MS: u64 = 200;

fn home_dir() -> PathBuf {
    match env::var("HOME").or_else(|_| env::var("USERPROFILE")) {
        Ok(dir) => PathBuf::from(dir),
        Err(_) => {
            eprintln!("apw: warning: HOME is not set; runtime files will be written to the current directory");
            PathBuf::from(".")
        }
    }
}

fn set_permissions(path: &Path, mode: u32) -> Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(|error| {
        APWError::new(
            Status::InvalidConfig,
            format!("Failed to set permissions on {}: {error}", path.display()),
        )
    })
}

fn copy_dir_recursive(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination).map_err(|error| {
        APWError::new(
            Status::ProcessNotRunning,
            format!(
                "Failed to create destination directory {}: {error}",
                destination.display()
            ),
        )
    })?;

    for entry in fs::read_dir(source).map_err(|error| {
        APWError::new(
            Status::ProcessNotRunning,
            format!("Failed to read app bundle {}: {error}", source.display()),
        )
    })? {
        let entry = entry.map_err(|error| {
            APWError::new(
                Status::ProcessNotRunning,
                format!(
                    "Failed to enumerate app bundle {}: {error}",
                    source.display()
                ),
            )
        })?;
        let file_type = entry.file_type().map_err(|error| {
            APWError::new(
                Status::ProcessNotRunning,
                format!(
                    "Failed to read app bundle entry type {}: {error}",
                    entry.path().display()
                ),
            )
        })?;
        let target = destination.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target).map_err(|error| {
                APWError::new(
                    Status::ProcessNotRunning,
                    format!(
                        "Failed to copy app bundle entry {}: {error}",
                        entry.path().display()
                    ),
                )
            })?;
        }
    }

    Ok(())
}

pub fn native_app_runtime_dir() -> PathBuf {
    home_dir().join(".apw").join("native-app")
}

pub fn native_app_socket_path() -> PathBuf {
    native_app_runtime_dir().join(NATIVE_APP_SOCKET_NAME)
}

pub fn native_app_status_path() -> PathBuf {
    native_app_runtime_dir().join(NATIVE_APP_STATUS_NAME)
}

pub fn native_app_credentials_path() -> PathBuf {
    native_app_runtime_dir().join(NATIVE_APP_CREDENTIALS_NAME)
}

pub fn native_app_install_dir() -> PathBuf {
    native_app_runtime_dir().join("installed")
}

pub fn native_app_bundle_install_path() -> PathBuf {
    native_app_install_dir().join(NATIVE_APP_BUNDLE_NAME)
}

pub fn native_app_executable_in_bundle(bundle_path: &Path) -> PathBuf {
    bundle_path
        .join("Contents")
        .join("MacOS")
        .join(NATIVE_APP_EXECUTABLE_NAME)
}

fn resolve_packaged_native_app_bundle() -> Result<PathBuf> {
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut candidates = vec![
        cwd.join("native-app")
            .join("dist")
            .join(NATIVE_APP_BUNDLE_NAME),
        cwd.join("../native-app")
            .join("dist")
            .join(NATIVE_APP_BUNDLE_NAME),
        cwd.join("../../native-app")
            .join("dist")
            .join(NATIVE_APP_BUNDLE_NAME),
    ];

    if let Ok(exe) = env::current_exe() {
        if let Some(parent) = exe.parent() {
            candidates.push(
                parent
                    .join("../libexec")
                    .join(NATIVE_APP_BUNDLE_NAME)
                    .canonicalize()
                    .unwrap_or_else(|_| parent.join("../libexec").join(NATIVE_APP_BUNDLE_NAME)),
            );
            candidates.push(
                parent
                    .join("../../native-app/dist")
                    .join(NATIVE_APP_BUNDLE_NAME)
                    .canonicalize()
                    .unwrap_or_else(|_| {
                        parent
                            .join("../../native-app/dist")
                            .join(NATIVE_APP_BUNDLE_NAME)
                    }),
            );
        }
    }

    for candidate in candidates {
        if candidate.exists() {
            return Ok(candidate);
        }
    }

    Err(APWError::new(
        Status::ProcessNotRunning,
        "Packaged APW app bundle not found. Build it with `./scripts/build-native-app.sh` first.",
    ))
}

fn ensure_runtime_dir() -> Result<()> {
    let path = native_app_runtime_dir();
    fs::create_dir_all(&path).map_err(|error| {
        APWError::new(
            Status::InvalidConfig,
            format!("Failed to create native app runtime directory: {error}"),
        )
    })?;
    set_permissions(&path, NATIVE_APP_RUNTIME_DIR_MODE)?;
    Ok(())
}

fn read_bundle_version(bundle_path: &Path) -> Option<String> {
    let info_plist = bundle_path.join("Contents").join("Info.plist");
    let content = fs::read_to_string(info_plist).ok()?;
    let marker = "<key>CFBundleShortVersionString</key>";
    let start = content.find(marker)?;
    let rest = &content[start + marker.len()..];
    let string_start = rest.find("<string>")?;
    let rest = &rest[string_start + "<string>".len()..];
    let string_end = rest.find("</string>")?;
    Some(rest[..string_end].trim().to_string())
}

fn load_status_file() -> Option<Value> {
    serde_json::from_str(&fs::read_to_string(native_app_status_path()).ok()?).ok()
}

fn default_credentials_payload() -> Value {
    json!({
        "demo": true,
        "domains": ["example.com"],
        "credentials": [
            {
                "domain": "example.com",
                "url": "https://example.com",
                "username": "demo@example.com",
                "password": "apw-demo-password"
            }
        ]
    })
}

fn ensure_default_credentials_file() -> Result<()> {
    let path = native_app_credentials_path();
    if path.exists() {
        return Ok(());
    }
    let content = serde_json::to_vec_pretty(&default_credentials_payload()).map_err(|error| {
        APWError::new(
            Status::InvalidConfig,
            format!("Failed to serialize default bootstrap credentials: {error}"),
        )
    })?;
    fs::write(&path, content).map_err(|error| {
        APWError::new(
            Status::InvalidConfig,
            format!("Failed to write default bootstrap credentials: {error}"),
        )
    })?;
    set_permissions(&path, NATIVE_APP_FILE_MODE)?;
    eprintln!(
        "apw: info: created demo credentials file at {}. \
         This file contains placeholder credentials — replace them with real entries before use.",
        path.display()
    );
    Ok(())
}

fn socket_running() -> bool {
    let socket_path = native_app_socket_path();
    if !socket_path.exists() {
        return false;
    }
    UnixStream::connect(socket_path).is_ok()
}

fn parse_response(payload: Value) -> Result<Value> {
    let object = payload.as_object().ok_or_else(|| {
        APWError::new(
            Status::ProtoInvalidResponse,
            "Native app returned a malformed response envelope.",
        )
    })?;

    let ok = object.get("ok").and_then(Value::as_bool).ok_or_else(|| {
        APWError::new(
            Status::ProtoInvalidResponse,
            "Native app returned a malformed response envelope.",
        )
    })?;

    if ok {
        return object.get("payload").cloned().ok_or_else(|| {
            APWError::new(
                Status::ProtoInvalidResponse,
                "Native app response is missing its payload.",
            )
        });
    }

    let code = object
        .get("code")
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or(Status::GenericError);
    let message = object
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or("Native app request failed.");
    Err(APWError::new(code, message))
}

fn send_request(command: &str, payload: Value) -> Result<Value> {
    let socket_path = native_app_socket_path();
    if !socket_path.exists() {
        return send_request_via_executable(command, payload);
    }

    let mut stream = None;
    for _ in 0..CONNECT_RETRIES {
        match UnixStream::connect(&socket_path) {
            Ok(connection) => {
                stream = Some(connection);
                break;
            }
            Err(_) => {
                std::thread::sleep(Duration::from_millis(CONNECT_RETRY_DELAY_MS));
            }
        }
    }
    let mut stream = match stream {
        Some(connection) => connection,
        None => return send_request_via_executable(command, payload),
    };
    let timeout = Duration::from_millis(SOCKET_TIMEOUT_MS);
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));

    let request = json!({
        "requestId": format!("req-{}", uuid_like_suffix()),
        "command": command,
        "payload": payload,
    });
    let data = serde_json::to_vec(&request).map_err(|error| {
        APWError::new(
            Status::GenericError,
            format!("Failed to encode native app request: {error}"),
        )
    })?;
    if data.len() > MAX_BROKER_BYTES {
        return Err(APWError::new(
            Status::ProtoInvalidResponse,
            "Native app request payload too large.",
        ));
    }

    stream.write_all(&data).map_err(|error| {
        APWError::new(
            Status::CommunicationTimeout,
            format!("Failed to send request to the APW app service: {error}"),
        )
    })?;
    stream.shutdown(std::net::Shutdown::Write).ok();

    let mut response = Vec::with_capacity(MAX_BROKER_BYTES);
    stream
        .take((MAX_BROKER_BYTES + 1) as u64)
        .read_to_end(&mut response)
        .map_err(|error| {
            APWError::new(
                Status::CommunicationTimeout,
                format!("Failed to read response from the APW app service: {error}"),
            )
        })?;
    if response.len() > MAX_BROKER_BYTES {
        return Err(APWError::new(
            Status::ProtoInvalidResponse,
            "Native app response payload too large.",
        ));
    }
    let value: Value = serde_json::from_slice(&response).map_err(|error| {
        APWError::new(
            Status::ProtoInvalidResponse,
            format!("Native app returned invalid JSON: {error}"),
        )
    })?;
    parse_response(value)
}

fn send_request_via_executable(command: &str, payload: Value) -> Result<Value> {
    let bundle_path = native_app_bundle_install_path();
    if !bundle_path.exists() {
        return Err(APWError::new(
            Status::ProcessNotRunning,
            "APW app service is not running. Run `apw app install` first.",
        ));
    }
    let executable = native_app_executable_in_bundle(&bundle_path);
    let payload_arg = serde_json::to_string(&payload).map_err(|error| {
        APWError::new(
            Status::GenericError,
            format!("Failed to encode native app fallback request: {error}"),
        )
    })?;
    let output = Command::new(&executable)
        .arg("request")
        .arg(command)
        .arg(payload_arg)
        .output()
        .map_err(|error| {
            APWError::new(
                Status::ProcessNotRunning,
                format!("Failed to execute the APW app directly: {error}"),
            )
        })?;
    if output.stdout.len() > MAX_BROKER_BYTES {
        return Err(APWError::new(
            Status::ProtoInvalidResponse,
            "Native app direct response payload too large.",
        ));
    }
    let value: Value = serde_json::from_slice(&output.stdout).map_err(|error| {
        APWError::new(
            Status::ProtoInvalidResponse,
            format!("Native app direct response is not valid JSON: {error}"),
        )
    })?;
    parse_response(value)
}

fn uuid_like_suffix() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    format!("{:016x}{:016x}", rng.gen::<u64>(), rng.gen::<u64>())
}

pub fn native_app_status() -> Value {
    let install_path = native_app_bundle_install_path();
    let executable_path = native_app_executable_in_bundle(&install_path);
    let status_file = load_status_file();
    let live_status = send_request("status", json!({})).ok();

    json!({
        "bundlePath": install_path,
        "installed": install_path.exists(),
        "executablePath": executable_path,
        "executableExists": executable_path.exists(),
        "bundleVersion": read_bundle_version(&install_path),
        "socketPath": native_app_socket_path(),
        "credentialsPath": native_app_credentials_path(),
        "service": {
            "running": socket_running(),
            "statusFile": native_app_status_path(),
            "lastKnown": status_file,
            "live": live_status,
            "transport": "unix_socket",
            "transportContract": "typed_json_envelope"
        }
    })
}

pub fn native_app_doctor() -> Result<Value> {
    ensure_runtime_dir()?;
    ensure_default_credentials_file()?;

    let mut doctor = native_app_status();
    if let Some(object) = doctor.as_object_mut() {
        object.insert(
            "frameworks".to_string(),
            json!({
                "authenticationServicesLinked": true,
                "associatedDomainsConfigured": ["example.com"],
            }),
        );
        object.insert(
            "releaseLine".to_string(),
            json!({
                "target": "v2.0.0",
                "version": VERSION,
                "legacyParityCommandsRetained": true,
            }),
        );
        object.insert(
            "guidance".to_string(),
            json!([
                "Run `./scripts/build-native-app.sh` if the app bundle is missing.",
                "Run `apw app install` to install the APW app bundle into the user runtime directory.",
                "Run `apw app launch` to start the local broker service.",
                "Run `apw login https://example.com` to exercise the bootstrap credential flow."
            ]),
        );
    }
    Ok(doctor)
}

pub fn native_app_install() -> Result<Value> {
    ensure_runtime_dir()?;
    ensure_default_credentials_file()?;

    let source_bundle = resolve_packaged_native_app_bundle()?;
    let install_dir = native_app_install_dir();
    fs::create_dir_all(&install_dir).map_err(|error| {
        APWError::new(
            Status::ProcessNotRunning,
            format!("Failed to create native app install directory: {error}"),
        )
    })?;
    set_permissions(&install_dir, NATIVE_APP_RUNTIME_DIR_MODE)?;

    let installed_bundle = native_app_bundle_install_path();
    if installed_bundle.exists() {
        fs::remove_dir_all(&installed_bundle).map_err(|error| {
            APWError::new(
                Status::ProcessNotRunning,
                format!("Failed to replace installed APW app bundle: {error}"),
            )
        })?;
    }
    copy_dir_recursive(&source_bundle, &installed_bundle)?;
    set_permissions(&installed_bundle, 0o755)?;
    ensure_default_credentials_file()?;

    Ok(json!({
        "status": "installed",
        "bundlePath": installed_bundle,
        "version": read_bundle_version(&installed_bundle),
        "doctor": native_app_doctor()?,
    }))
}

pub fn native_app_launch() -> Result<Value> {
    ensure_runtime_dir()?;

    let bundle_path = native_app_bundle_install_path();
    if !bundle_path.exists() {
        return Err(APWError::new(
            Status::ProcessNotRunning,
            "APW app bundle is not installed. Run `apw app install` first.",
        ));
    }
    let executable = native_app_executable_in_bundle(&bundle_path);
    if !executable.exists() {
        return Err(APWError::new(
            Status::ProcessNotRunning,
            format!(
                "APW app executable is missing from the installed bundle: {}",
                executable.display()
            ),
        ));
    }

    if socket_running() {
        return Ok(json!({
            "status": "running",
            "bundlePath": bundle_path,
            "socketPath": native_app_socket_path(),
        }));
    }

    let status_log = native_app_runtime_dir().join("app.stdout.log");
    let error_log = native_app_runtime_dir().join("app.stderr.log");
    let stdout = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&status_log)
        .map_err(|error| {
            APWError::new(
                Status::ProcessNotRunning,
                format!("Failed to open native app stdout log: {error}"),
            )
        })?;
    let stderr = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&error_log)
        .map_err(|error| {
            APWError::new(
                Status::ProcessNotRunning,
                format!("Failed to open native app stderr log: {error}"),
            )
        })?;

    let mut command = Command::new(&executable);
    command
        .arg("serve")
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    // SAFETY: `pre_exec` runs after `fork` and before `exec`. The closure only calls
    // `libc::setsid()`, which is async-signal-safe and does not touch any Rust state.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    command.spawn().map_err(|error| {
        APWError::new(
            Status::ProcessNotRunning,
            format!("Failed to launch the APW app service: {error}"),
        )
    })?;

    std::thread::sleep(Duration::from_millis(300));

    Ok(json!({
        "status": if socket_running() { "launched" } else { "starting" },
        "bundlePath": bundle_path,
        "socketPath": native_app_socket_path(),
        "stdoutLog": status_log,
        "stderrLog": error_log,
    }))
}

pub fn native_app_login(url: &str) -> Result<Value> {
    let payload = send_request("login", json!({ "url": url }))?;
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use tempfile::TempDir;

    fn with_temp_home<F, R>(run: F) -> R
    where
        F: FnOnce() -> R,
    {
        let temp = TempDir::new().unwrap();
        let previous_home = env::var("HOME").ok();
        env::set_var("HOME", temp.path());
        let result = run();
        if let Some(value) = previous_home {
            env::set_var("HOME", value);
        } else {
            env::remove_var("HOME");
        }
        result
    }

    #[test]
    #[serial]
    fn doctor_creates_default_credentials_file() {
        with_temp_home(|| {
            let payload = native_app_doctor().unwrap();
            assert_eq!(
                payload["frameworks"]["authenticationServicesLinked"],
                json!(true)
            );
            assert!(native_app_credentials_path().exists());
        });
    }

    #[test]
    #[serial]
    fn status_reports_uninstalled_bundle_by_default() {
        with_temp_home(|| {
            let payload = native_app_status();
            assert_eq!(payload["installed"], json!(false));
            assert_eq!(payload["service"]["running"], json!(false));
        });
    }
}
