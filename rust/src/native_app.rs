use crate::error::{APWError, Result};
use crate::logging;
use crate::types::{ExternalFallbackProvider, Status, MAX_MESSAGE_BYTES, VERSION};
use crate::utils::read_config_file_or_empty;
use serde_json::{json, Value};
use std::env;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::FileTypeExt;
use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::Duration;

const NATIVE_APP_BUNDLE_NAME: &str = "APW.app";
const NATIVE_APP_EXECUTABLE_NAME: &str = "APW";
const NATIVE_APP_SOCKET_NAME: &str = "broker.sock";
const NATIVE_APP_STATUS_NAME: &str = "status.json";
const NATIVE_APP_CREDENTIALS_NAME: &str = "credentials.json";
const NATIVE_APP_BROKER_LOG_NAME: &str = "broker.log";
const NATIVE_APP_RUNTIME_DIR_MODE: u32 = 0o700;
const NATIVE_APP_FILE_MODE: u32 = 0o600;
const MAX_BROKER_BYTES: usize = MAX_MESSAGE_BYTES;
const MAX_BROKER_LOG_BYTES: u64 = 10 * 1024 * 1024;
const SOCKET_TIMEOUT_MS: u64 = 3_000;
const CONNECT_RETRIES: usize = 10;
const CONNECT_RETRY_DELAY_MS: u64 = 200;

/// Environment variable that opts the bootstrap flow into materializing a
/// plaintext demo credentials file. Without it, `apw app install` /
/// `apw doctor` never write `credentials.json`. See issue #14.
pub const APW_DEMO_ENV: &str = "APW_DEMO";

fn demo_mode_enabled() -> bool {
    matches!(env::var(APW_DEMO_ENV).as_deref(), Ok("1"))
}

/// Default per-invocation timeout for an external fallback provider exec.
/// Overridable via `APW_FALLBACK_TIMEOUT_MS`. See issue #3.
const DEFAULT_FALLBACK_TIMEOUT_MS: u64 = 15_000;

/// Default per-process invocation cap for external fallback providers.
/// Overridable via `APW_FALLBACK_INVOCATION_LIMIT`. See issue #3.
const DEFAULT_FALLBACK_INVOCATION_LIMIT: usize = 5;

static FALLBACK_INVOCATIONS: AtomicUsize = AtomicUsize::new(0);

fn fallback_timeout() -> Duration {
    let ms = env::var("APW_FALLBACK_TIMEOUT_MS")
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .unwrap_or(DEFAULT_FALLBACK_TIMEOUT_MS);
    Duration::from_millis(ms)
}

fn fallback_invocation_limit() -> usize {
    env::var("APW_FALLBACK_INVOCATION_LIMIT")
        .ok()
        .and_then(|raw| raw.parse::<usize>().ok())
        .unwrap_or(DEFAULT_FALLBACK_INVOCATION_LIMIT)
}

fn record_fallback_invocation() -> Result<()> {
    let limit = fallback_invocation_limit();
    let count = FALLBACK_INVOCATIONS.fetch_add(1, Ordering::SeqCst) + 1;
    if count > limit {
        return Err(APWError::new(
            Status::GenericError,
            format!(
                "External fallback provider invoked {count} times this session, exceeding the configured limit of {limit}. Restart `apw` to reset, or raise APW_FALLBACK_INVOCATION_LIMIT."
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
fn reset_fallback_invocations_for_tests() {
    FALLBACK_INVOCATIONS.store(0, Ordering::SeqCst);
}

/// Validate an external fallback-provider path before exec. The returned
/// `PathBuf` is the canonicalized path safe to hand to `Command::new`. The
/// validation is best-effort against TOCTOU — the file is re-stat'd via the
/// canonical path immediately before exec, but a fully race-free path would
/// require `fexecve` against a previously opened fd. See issue #1.
fn validate_provider_path(provider_label: &str, raw: &str) -> Result<PathBuf> {
    if raw.starts_with('~') {
        return Err(APWError::new(
            Status::InvalidConfig,
            format!(
                "Fallback provider `{provider_label}` path must not start with `~`; expand it to an absolute filesystem path."
            ),
        ));
    }

    let candidate = PathBuf::from(raw);
    if !candidate.is_absolute() {
        return Err(APWError::new(
            Status::InvalidConfig,
            format!("Fallback provider `{provider_label}` must use an absolute executable path."),
        ));
    }

    let canonical = fs::canonicalize(&candidate).map_err(|error| {
        APWError::new(
            Status::InvalidConfig,
            format!(
                "Fallback provider `{provider_label}` path {} is not reachable: {error}",
                candidate.display()
            ),
        )
    })?;

    let metadata = fs::metadata(&canonical).map_err(|error| {
        APWError::new(
            Status::InvalidConfig,
            format!(
                "Fallback provider `{provider_label}` path {} cannot be stat'd: {error}",
                canonical.display()
            ),
        )
    })?;

    if !metadata.file_type().is_file() {
        return Err(APWError::new(
            Status::InvalidConfig,
            format!(
                "Fallback provider `{provider_label}` path {} must be a regular file.",
                canonical.display()
            ),
        ));
    }

    let mode = metadata.permissions().mode();
    if mode & 0o002 != 0 {
        return Err(APWError::new(
            Status::InvalidConfig,
            format!(
                "Fallback provider `{provider_label}` path {} is world-writable (mode {:o}); refuse to exec.",
                canonical.display(),
                mode & 0o777
            ),
        ));
    }
    if mode & 0o111 == 0 {
        return Err(APWError::new(
            Status::InvalidConfig,
            format!(
                "Fallback provider `{provider_label}` path {} has no execute bit (mode {:o}).",
                canonical.display(),
                mode & 0o777
            ),
        ));
    }

    let euid = unsafe { libc::geteuid() };
    if metadata.uid() != euid {
        return Err(APWError::new(
            Status::InvalidConfig,
            format!(
                "Fallback provider `{provider_label}` path {} is owned by uid {} but the current process runs as uid {}; refuse to exec.",
                canonical.display(),
                metadata.uid(),
                euid
            ),
        ));
    }

    Ok(canonical)
}

/// Run a `Command` with a wall-clock timeout, killing the child if it does
/// not terminate in time. See issue #3.
fn run_provider_command(provider_label: &str, mut command: Command) -> Result<Output> {
    record_fallback_invocation()?;

    let timeout = fallback_timeout();
    let child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            APWError::new(
                Status::ProcessNotRunning,
                format!("Failed to spawn `{provider_label}`: {error}"),
            )
        })?;

    let pid = child.id() as i32;

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });

    match rx.recv_timeout(timeout) {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(error)) => Err(APWError::new(
            Status::ProcessNotRunning,
            format!("`{provider_label}` exec failed: {error}"),
        )),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            unsafe {
                libc::kill(pid, libc::SIGKILL);
            }
            logging::warn(
                "native-app",
                format!(
                    "killed `{provider_label}` after {} ms; provider exceeded APW_FALLBACK_TIMEOUT_MS.",
                    timeout.as_millis()
                ),
            );
            Err(APWError::new(
                Status::CommunicationTimeout,
                format!(
                    "External fallback provider `{provider_label}` did not respond within {} ms.",
                    timeout.as_millis()
                ),
            ))
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(APWError::new(
            Status::GenericError,
            format!("`{provider_label}` exec channel closed unexpectedly."),
        )),
    }
}

fn home_dir() -> PathBuf {
    match env::var("HOME").or_else(|_| env::var("USERPROFILE")) {
        Ok(dir) => PathBuf::from(dir),
        Err(_) => {
            logging::warn(
                "native-app",
                "HOME is not set; runtime files will be written to the current directory",
            );
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

pub fn native_app_broker_log_path() -> PathBuf {
    native_app_runtime_dir().join(NATIVE_APP_BROKER_LOG_NAME)
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

fn rotate_broker_log_if_needed(path: &Path) -> Result<()> {
    let metadata = match fs::metadata(path) {
        Ok(value) => value,
        Err(_) => return Ok(()),
    };

    if metadata.len() < MAX_BROKER_LOG_BYTES {
        return Ok(());
    }

    let rotated = path.with_extension("log.1");
    if rotated.exists() {
        fs::remove_file(&rotated).map_err(|error| {
            APWError::new(
                Status::ProcessNotRunning,
                format!(
                    "Failed to remove rotated broker log {}: {error}",
                    rotated.display()
                ),
            )
        })?;
    }

    fs::rename(path, &rotated).map_err(|error| {
        APWError::new(
            Status::ProcessNotRunning,
            format!("Failed to rotate broker log {}: {error}", path.display()),
        )
    })?;
    logging::warn(
        "native-app",
        format!("rotated broker log to {}", rotated.display()),
    );
    Ok(())
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
    if !demo_mode_enabled() {
        return Ok(());
    }
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
    logging::warn(
        "native-app",
        format!(
            "APW_DEMO=1 set; wrote placeholder credentials to {}. Disable APW_DEMO before shipping.",
            path.display()
        ),
    );
    Ok(())
}

fn socket_running() -> bool {
    let socket_path = native_app_socket_path();
    if !socket_path.exists() {
        return false;
    }
    if !socket_path_safe_to_connect(&socket_path) {
        return false;
    }
    UnixStream::connect(socket_path).is_ok()
}

fn socket_path_safe_to_connect(socket_path: &Path) -> bool {
    let metadata = match fs::symlink_metadata(socket_path) {
        Ok(value) => value,
        Err(_) => return false,
    };

    if !metadata.file_type().is_socket() {
        return false;
    }

    metadata.permissions().mode() & 0o777 == NATIVE_APP_FILE_MODE
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
    if !socket_path_safe_to_connect(&socket_path) {
        logging::warn(
            "native-app",
            format!(
                "broker socket at {} failed security checks, falling back to direct execution",
                socket_path.display()
            ),
        );
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
    logging::warn(
        "native-app",
        format!(
            "broker socket unavailable, falling back to {}",
            executable.display()
        ),
    );
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
        "brokerLogPath": native_app_broker_log_path(),
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
                "Run `apw login https://example.com` to exercise the bootstrap credential flow.",
                format!("Inspect broker logs at {}.", native_app_broker_log_path().display())
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

    let broker_log = native_app_broker_log_path();
    rotate_broker_log_if_needed(&broker_log)?;
    let stdout = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&broker_log)
        .map_err(|error| {
            APWError::new(
                Status::ProcessNotRunning,
                format!("Failed to open native app broker log: {error}"),
            )
        })?;
    set_permissions(&broker_log, NATIVE_APP_FILE_MODE)?;
    let stderr = stdout.try_clone().map_err(|error| {
        APWError::new(
            Status::ProcessNotRunning,
            format!("Failed to clone native app broker log handle: {error}"),
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
        "brokerLog": broker_log,
    }))
}

pub fn native_app_login(url: &str) -> Result<Value> {
    match native_app_request("login", url) {
        Ok(payload) => Ok(payload),
        Err(error) if matches!(error.code, Status::NoResults | Status::ProcessNotRunning) => {
            if let Some(payload) = external_provider_login(url)? {
                return Ok(payload);
            }
            Err(error)
        }
        Err(error) => Err(error),
    }
}

pub fn native_app_fill(url: &str) -> Result<Value> {
    native_app_request("fill", url)
}

fn native_app_request(intent: &str, url: &str) -> Result<Value> {
    let payload = send_request(intent, json!({ "url": url, "intent": intent }))?;
    Ok(payload)
}

fn external_provider_login(url: &str) -> Result<Option<Value>> {
    let config = read_config_file_or_empty();
    let Some(provider) = config.fallback_provider else {
        return Ok(None);
    };
    let Some(raw_path) = config.fallback_provider_path.as_deref() else {
        return Err(APWError::new(
            Status::InvalidConfig,
            format!(
                "Fallback provider `{}` requires an absolute `fallbackProviderPath`.",
                provider.as_str()
            ),
        ));
    };
    let provider_path = validate_provider_path(provider.as_str(), raw_path)?;

    let host = url::Url::parse(url)
        .map_err(|_| APWError::new(Status::InvalidParam, "Invalid URL for external fallback."))?
        .host_str()
        .map(str::to_string)
        .ok_or_else(|| APWError::new(Status::InvalidParam, "Invalid URL for external fallback."))?;

    let payload = match provider {
        ExternalFallbackProvider::OnePassword => {
            load_1password_credential(&provider_path, &host, url)?
        }
        ExternalFallbackProvider::Bitwarden => {
            load_bitwarden_credential(&provider_path, &host, url)?
        }
    };
    Ok(Some(payload))
}

fn load_1password_credential(path: &Path, host: &str, raw_url: &str) -> Result<Value> {
    let mut list_command = Command::new(path);
    list_command
        .arg("item")
        .arg("list")
        .arg("--categories")
        .arg("LOGIN")
        .arg("--format")
        .arg("json");
    let list_output = run_provider_command("1password", list_command)?;
    if !list_output.status.success() {
        return Err(APWError::new(
            Status::NoResults,
            format!(
                "1Password CLI did not return a credential for {host}: {}",
                String::from_utf8_lossy(&list_output.stderr).trim()
            ),
        ));
    }

    let items: Value = serde_json::from_slice(&list_output.stdout).map_err(|error| {
        APWError::new(
            Status::ProtoInvalidResponse,
            format!("1Password CLI returned invalid JSON: {error}"),
        )
    })?;
    let item_id = items
        .as_array()
        .and_then(|values| {
            values
                .iter()
                .find(|item| one_password_item_matches_url(item, host, raw_url))
                .or_else(|| {
                    values
                        .iter()
                        .find(|item| one_password_item_matches_title(item, host, raw_url))
                })
        })
        .and_then(|item| item.get("id"))
        .and_then(Value::as_str)
        .ok_or_else(|| {
            APWError::new(
                Status::NoResults,
                format!("1Password CLI returned no credential for {host}."),
            )
        })?;

    let mut get_command = Command::new(path);
    get_command
        .arg("item")
        .arg("get")
        .arg(item_id)
        .arg("--format")
        .arg("json");
    let output = run_provider_command("1password", get_command)?;
    if !output.status.success() {
        return Err(APWError::new(
            Status::NoResults,
            format!(
                "1Password CLI did not return a credential for {host}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }

    let item: Value = serde_json::from_slice(&output.stdout).map_err(|error| {
        APWError::new(
            Status::ProtoInvalidResponse,
            format!("1Password CLI returned invalid JSON: {error}"),
        )
    })?;
    let fields = item
        .get("fields")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            APWError::new(
                Status::ProtoInvalidResponse,
                "1Password item is missing fields.",
            )
        })?;
    let username = fields
        .iter()
        .find(|field| {
            field.get("id").and_then(Value::as_str) == Some("username")
                || field.get("label").and_then(Value::as_str) == Some("username")
                || field.get("purpose").and_then(Value::as_str) == Some("USERNAME")
        })
        .and_then(|field| field.get("value"))
        .and_then(Value::as_str)
        .ok_or_else(|| {
            APWError::new(
                Status::ProtoInvalidResponse,
                "1Password item is missing a username.",
            )
        })?;
    let password = fields
        .iter()
        .find(|field| {
            field.get("id").and_then(Value::as_str) == Some("password")
                || field.get("label").and_then(Value::as_str) == Some("password")
                || field.get("purpose").and_then(Value::as_str) == Some("PASSWORD")
        })
        .and_then(|field| field.get("value"))
        .and_then(Value::as_str)
        .ok_or_else(|| {
            APWError::new(
                Status::ProtoInvalidResponse,
                "1Password item is missing a password.",
            )
        })?;
    let resolved_url = item
        .get("urls")
        .and_then(Value::as_array)
        .and_then(|urls| urls.first())
        .and_then(|entry| entry.get("href"))
        .and_then(Value::as_str)
        .unwrap_or(raw_url);

    Ok(external_cli_payload(
        ExternalFallbackProvider::OnePassword,
        host,
        resolved_url,
        username,
        password,
    ))
}

fn load_bitwarden_credential(path: &Path, host: &str, raw_url: &str) -> Result<Value> {
    let mut command = Command::new(path);
    command.arg("list").arg("items").arg("--search").arg(host);
    let output = run_provider_command("bitwarden", command)?;
    if !output.status.success() {
        return Err(APWError::new(
            Status::NoResults,
            format!(
                "Bitwarden CLI did not return a credential for {host}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }

    let items: Value = serde_json::from_slice(&output.stdout).map_err(|error| {
        APWError::new(
            Status::ProtoInvalidResponse,
            format!("Bitwarden CLI returned invalid JSON: {error}"),
        )
    })?;
    let item = items
        .as_array()
        .and_then(|values| {
            values
                .iter()
                .find(|item| bitwarden_item_matches_target(item, host, raw_url))
        })
        .ok_or_else(|| {
            APWError::new(
                Status::NoResults,
                format!("Bitwarden CLI returned no credential for {host}."),
            )
        })?;
    let login = item
        .get("login")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            APWError::new(
                Status::ProtoInvalidResponse,
                "Bitwarden item is missing login data.",
            )
        })?;
    let username = login
        .get("username")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            APWError::new(
                Status::ProtoInvalidResponse,
                "Bitwarden item is missing a username.",
            )
        })?;
    let password = login
        .get("password")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            APWError::new(
                Status::ProtoInvalidResponse,
                "Bitwarden item is missing a password.",
            )
        })?;
    let resolved_url = login
        .get("uris")
        .and_then(Value::as_array)
        .and_then(|uris| uris.first())
        .and_then(|entry| entry.get("uri"))
        .and_then(Value::as_str)
        .unwrap_or(raw_url);

    Ok(external_cli_payload(
        ExternalFallbackProvider::Bitwarden,
        host,
        resolved_url,
        username,
        password,
    ))
}

fn one_password_item_matches_url(item: &Value, host: &str, raw_url: &str) -> bool {
    item.get("urls")
        .and_then(Value::as_array)
        .map(|urls| {
            urls.iter()
                .filter_map(|entry| entry.get("href"))
                .filter_map(Value::as_str)
                .any(|uri| uri_matches_target(uri, host, raw_url))
        })
        .unwrap_or(false)
}

fn one_password_item_matches_title(item: &Value, host: &str, raw_url: &str) -> bool {
    item.get("title")
        .and_then(Value::as_str)
        .map(|title| title.eq_ignore_ascii_case(host) || title.eq_ignore_ascii_case(raw_url))
        .unwrap_or(false)
}

fn bitwarden_item_matches_target(item: &Value, host: &str, raw_url: &str) -> bool {
    item.get("login")
        .and_then(Value::as_object)
        .and_then(|login| login.get("uris"))
        .and_then(Value::as_array)
        .map(|uris| {
            uris.iter()
                .filter_map(|entry| entry.get("uri"))
                .filter_map(Value::as_str)
                .any(|uri| uri_matches_target(uri, host, raw_url))
        })
        .unwrap_or(false)
}

fn uri_matches_target(uri: &str, host: &str, raw_url: &str) -> bool {
    uri.eq_ignore_ascii_case(raw_url)
        || host_from_uri_like(uri)
            .map(|candidate| candidate.eq_ignore_ascii_case(host))
            .unwrap_or(false)
}

fn host_from_uri_like(uri: &str) -> Option<String> {
    url::Url::parse(uri)
        .ok()
        .and_then(|value| value.host_str().map(str::to_string))
        .or_else(|| {
            url::Url::parse(&format!("https://{uri}"))
                .ok()
                .and_then(|value| value.host_str().map(str::to_string))
        })
}

fn external_cli_payload(
    provider: ExternalFallbackProvider,
    host: &str,
    url: &str,
    username: &str,
    password: &str,
) -> Value {
    json!({
        "status": "approved",
        "url": url,
        "domain": host,
        "username": username,
        "password": password,
        "transport": "external_cli",
        "userMediated": false,
        "source": provider.as_str(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::APWConfigV1;
    use serial_test::serial;
    use std::os::unix::net::UnixListener;
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

    fn with_demo_env<F, R>(run: F) -> R
    where
        F: FnOnce() -> R,
    {
        let previous = env::var(APW_DEMO_ENV).ok();
        env::set_var(APW_DEMO_ENV, "1");
        let result = run();
        if let Some(value) = previous {
            env::set_var(APW_DEMO_ENV, value);
        } else {
            env::remove_var(APW_DEMO_ENV);
        }
        result
    }

    #[test]
    #[serial]
    fn doctor_does_not_create_credentials_by_default() {
        with_temp_home(|| {
            env::remove_var(APW_DEMO_ENV);
            let payload = native_app_doctor().unwrap();
            assert_eq!(
                payload["frameworks"]["authenticationServicesLinked"],
                json!(true)
            );
            assert!(
                !native_app_credentials_path().exists(),
                "credentials.json must not be materialized without APW_DEMO=1"
            );
        });
    }

    #[test]
    #[serial]
    fn doctor_creates_default_credentials_when_demo_env_set() {
        with_temp_home(|| {
            with_demo_env(|| {
                let payload = native_app_doctor().unwrap();
                assert_eq!(
                    payload["frameworks"]["authenticationServicesLinked"],
                    json!(true)
                );
                assert!(native_app_credentials_path().exists());
            });
        });
    }

    #[test]
    #[serial]
    fn status_reports_uninstalled_bundle_by_default() {
        with_temp_home(|| {
            let payload = native_app_status();
            assert_eq!(payload["installed"], json!(false));
            assert_eq!(payload["service"]["running"], json!(false));
            assert!(payload["brokerLogPath"]
                .as_str()
                .unwrap()
                .ends_with("broker.log"));
        });
    }

    #[test]
    #[serial]
    fn rotates_broker_log_when_it_exceeds_limit() {
        with_temp_home(|| {
            ensure_runtime_dir().unwrap();
            let path = native_app_broker_log_path();
            fs::write(&path, vec![b'x'; MAX_BROKER_LOG_BYTES as usize]).unwrap();

            rotate_broker_log_if_needed(&path).unwrap();

            assert!(!path.exists());
            assert!(path.with_extension("log.1").exists());
        });
    }

    #[test]
    #[serial]
    fn fill_request_includes_fill_intent() {
        with_temp_home(|| {
            let bundle_dir = native_app_bundle_install_path();
            fs::create_dir_all(bundle_dir.parent().unwrap()).unwrap();
            fs::create_dir_all(bundle_dir.join("Contents").join("MacOS")).unwrap();

            let executable = native_app_executable_in_bundle(&bundle_dir);
            fs::write(
                &executable,
                r#"#!/usr/bin/env python3
import json
import sys
payload = json.loads(sys.argv[3])
print(json.dumps({"ok": True, "code": 0, "payload": payload}))
"#,
            )
            .unwrap();
            let mut permissions = fs::metadata(&executable).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&executable, permissions).unwrap();

            let payload = native_app_fill("https://example.com").unwrap();
            assert_eq!(payload["intent"], "fill");
            assert_eq!(payload["url"], "https://example.com");
        });
    }

    #[test]
    #[serial]
    fn login_can_fallback_to_1password_cli() {
        with_temp_home(|| {
            reset_fallback_invocations_for_tests();
            let provider_dir = TempDir::new().unwrap();
            let provider_path = provider_dir.path().join("op");
            fs::write(
                &provider_path,
                r#"#!/usr/bin/env python3
import json
import sys

if sys.argv[1:] == ["item", "list", "--categories", "LOGIN", "--format", "json"]:
    print(json.dumps([
      {
        "id": "item-wrong",
        "title": "vault.example.com",
        "urls": [{"href": "https://elsewhere.example.com"}]
      },
      {
        "id": "item-correct",
        "title": "Work Vault",
        "urls": [{"href": "https://vault.example.com"}]
      }
    ]))
elif sys.argv[1:] == ["item", "get", "item-correct", "--format", "json"]:
    print(json.dumps({
      "fields": [
        {"id": "username", "value": "alice@example.com"},
        {"id": "password", "value": "secret-1password"}
      ],
      "urls": [{"href": "https://vault.example.com"}]
    }))
else:
    raise SystemExit(1)
"#,
            )
            .unwrap();
            let mut permissions = fs::metadata(&provider_path).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&provider_path, permissions).unwrap();

            let config_root = home_dir().join(".apw");
            fs::create_dir_all(&config_root).unwrap();
            let config = APWConfigV1 {
                username: "demo".to_string(),
                shared_key: "demo-shared-key".to_string(),
                fallback_provider: Some(ExternalFallbackProvider::OnePassword),
                fallback_provider_path: Some(provider_path.display().to_string()),
                ..APWConfigV1::default()
            };
            fs::write(
                config_root.join("config.json"),
                serde_json::to_vec_pretty(&config).unwrap(),
            )
            .unwrap();

            let payload = native_app_login("https://vault.example.com").unwrap();
            assert_eq!(payload["source"], "1password");
            assert_eq!(payload["transport"], "external_cli");
            assert_eq!(payload["username"], "alice@example.com");
        });
    }

    #[test]
    #[serial]
    fn login_bitwarden_fallback_matches_uri_before_selecting_item() {
        with_temp_home(|| {
            reset_fallback_invocations_for_tests();
            let provider_dir = TempDir::new().unwrap();
            let provider_path = provider_dir.path().join("bw");
            fs::write(
                &provider_path,
                r#"#!/usr/bin/env python3
import json
import sys

if sys.argv[1:] == ["list", "items", "--search", "vault.example.com"]:
    print(json.dumps([
      {
        "name": "vault.example.com note only",
        "login": {
          "username": "wrong@example.com",
          "password": "wrong-secret",
          "uris": [{"uri": "https://elsewhere.example.com"}]
        }
      },
      {
        "name": "Work Vault",
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
            .unwrap();
            let mut permissions = fs::metadata(&provider_path).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&provider_path, permissions).unwrap();

            let config_root = home_dir().join(".apw");
            fs::create_dir_all(&config_root).unwrap();
            let config = APWConfigV1 {
                username: "demo".to_string(),
                shared_key: "demo-shared-key".to_string(),
                fallback_provider: Some(ExternalFallbackProvider::Bitwarden),
                fallback_provider_path: Some(provider_path.display().to_string()),
                ..APWConfigV1::default()
            };
            fs::write(
                config_root.join("config.json"),
                serde_json::to_vec_pretty(&config).unwrap(),
            )
            .unwrap();

            let payload = native_app_login("https://vault.example.com").unwrap();
            assert_eq!(payload["source"], "bitwarden");
            assert_eq!(payload["transport"], "external_cli");
            assert_eq!(payload["username"], "alice@example.com");
            assert_eq!(payload["url"], "https://vault.example.com/login");
        });
    }

    #[test]
    #[serial]
    fn login_rejects_relative_external_provider_paths() {
        with_temp_home(|| {
            let config_root = home_dir().join(".apw");
            fs::create_dir_all(&config_root).unwrap();
            let config = APWConfigV1 {
                username: "demo".to_string(),
                shared_key: "demo-shared-key".to_string(),
                fallback_provider: Some(ExternalFallbackProvider::Bitwarden),
                fallback_provider_path: Some("bw".to_string()),
                ..APWConfigV1::default()
            };
            fs::write(
                config_root.join("config.json"),
                serde_json::to_vec_pretty(&config).unwrap(),
            )
            .unwrap();

            let error = native_app_login("https://vault.example.com").unwrap_err();
            assert_eq!(error.code, Status::InvalidConfig);
            assert!(error.message.contains("absolute executable path"));
        });
    }

    fn write_provider_config(provider_path: &Path) {
        let config_root = home_dir().join(".apw");
        fs::create_dir_all(&config_root).unwrap();
        let config = APWConfigV1 {
            username: "demo".to_string(),
            shared_key: "demo-shared-key".to_string(),
            fallback_provider: Some(ExternalFallbackProvider::Bitwarden),
            fallback_provider_path: Some(provider_path.display().to_string()),
            ..APWConfigV1::default()
        };
        fs::write(
            config_root.join("config.json"),
            serde_json::to_vec_pretty(&config).unwrap(),
        )
        .unwrap();
    }

    #[test]
    #[serial]
    fn login_rejects_tilde_prefixed_provider_paths() {
        with_temp_home(|| {
            let config_root = home_dir().join(".apw");
            fs::create_dir_all(&config_root).unwrap();
            let config = APWConfigV1 {
                username: "demo".to_string(),
                shared_key: "demo-shared-key".to_string(),
                fallback_provider: Some(ExternalFallbackProvider::Bitwarden),
                fallback_provider_path: Some("~/bin/bw".to_string()),
                ..APWConfigV1::default()
            };
            fs::write(
                config_root.join("config.json"),
                serde_json::to_vec_pretty(&config).unwrap(),
            )
            .unwrap();

            let error = native_app_login("https://vault.example.com").unwrap_err();
            assert_eq!(error.code, Status::InvalidConfig);
            assert!(error.message.contains("must not start with `~`"));
        });
    }

    #[test]
    #[serial]
    fn login_rejects_world_writable_provider_path() {
        with_temp_home(|| {
            reset_fallback_invocations_for_tests();
            let provider_dir = TempDir::new().unwrap();
            let provider_path = provider_dir.path().join("bw");
            fs::write(&provider_path, "#!/bin/sh\necho ignored\n").unwrap();
            fs::set_permissions(&provider_path, fs::Permissions::from_mode(0o777)).unwrap();
            write_provider_config(&provider_path);

            let error = native_app_login("https://vault.example.com").unwrap_err();
            assert_eq!(error.code, Status::InvalidConfig);
            assert!(
                error.message.contains("world-writable"),
                "expected world-writable rejection, got: {}",
                error.message
            );
        });
    }

    #[test]
    #[serial]
    fn login_rejects_non_executable_provider_path() {
        with_temp_home(|| {
            reset_fallback_invocations_for_tests();
            let provider_dir = TempDir::new().unwrap();
            let provider_path = provider_dir.path().join("bw");
            fs::write(&provider_path, "#!/bin/sh\necho ignored\n").unwrap();
            fs::set_permissions(&provider_path, fs::Permissions::from_mode(0o644)).unwrap();
            write_provider_config(&provider_path);

            let error = native_app_login("https://vault.example.com").unwrap_err();
            assert_eq!(error.code, Status::InvalidConfig);
            assert!(
                error.message.contains("execute bit"),
                "expected execute-bit rejection, got: {}",
                error.message
            );
        });
    }

    #[test]
    #[serial]
    fn login_rejects_unreachable_provider_path() {
        with_temp_home(|| {
            let config_root = home_dir().join(".apw");
            fs::create_dir_all(&config_root).unwrap();
            let config = APWConfigV1 {
                username: "demo".to_string(),
                shared_key: "demo-shared-key".to_string(),
                fallback_provider: Some(ExternalFallbackProvider::Bitwarden),
                fallback_provider_path: Some("/no/such/path/bw".to_string()),
                ..APWConfigV1::default()
            };
            fs::write(
                config_root.join("config.json"),
                serde_json::to_vec_pretty(&config).unwrap(),
            )
            .unwrap();

            let error = native_app_login("https://vault.example.com").unwrap_err();
            assert_eq!(error.code, Status::InvalidConfig);
            assert!(error.message.contains("not reachable"));
        });
    }

    #[test]
    #[serial]
    fn login_times_out_when_provider_hangs() {
        with_temp_home(|| {
            reset_fallback_invocations_for_tests();
            env::set_var("APW_FALLBACK_TIMEOUT_MS", "200");

            let provider_dir = TempDir::new().unwrap();
            let provider_path = provider_dir.path().join("bw");
            fs::write(&provider_path, "#!/bin/sh\nsleep 30\n").unwrap();
            fs::set_permissions(&provider_path, fs::Permissions::from_mode(0o755)).unwrap();
            write_provider_config(&provider_path);

            let started = std::time::Instant::now();
            let error = native_app_login("https://vault.example.com").unwrap_err();
            let elapsed = started.elapsed();

            env::remove_var("APW_FALLBACK_TIMEOUT_MS");

            assert_eq!(error.code, Status::CommunicationTimeout);
            assert!(
                elapsed < Duration::from_secs(5),
                "fallback exec must abort near the 200ms timeout, took {elapsed:?}"
            );
        });
    }

    #[test]
    #[serial]
    fn login_enforces_session_invocation_limit() {
        with_temp_home(|| {
            reset_fallback_invocations_for_tests();
            env::set_var("APW_FALLBACK_INVOCATION_LIMIT", "2");

            let provider_dir = TempDir::new().unwrap();
            let provider_path = provider_dir.path().join("bw");
            fs::write(
                &provider_path,
                "#!/usr/bin/env python3\nimport json\nprint(json.dumps([]))\n",
            )
            .unwrap();
            fs::set_permissions(&provider_path, fs::Permissions::from_mode(0o755)).unwrap();
            write_provider_config(&provider_path);

            // First two invocations are allowed (each call returns NoResults
            // because the script returns an empty array — that's fine; we're
            // only asserting the limit, not the result).
            let _ = native_app_login("https://vault.example.com");
            let _ = native_app_login("https://vault.example.com");

            // Third should be refused by the rate limiter.
            let error = native_app_login("https://vault.example.com").unwrap_err();
            env::remove_var("APW_FALLBACK_INVOCATION_LIMIT");

            assert!(
                error.message.contains("exceeding the configured limit"),
                "expected rate-limit error, got: {}",
                error.message
            );
        });
    }

    #[test]
    #[serial]
    fn invalid_socket_permissions_fall_back_to_direct_exec() {
        with_temp_home(|| {
            ensure_runtime_dir().unwrap();
            let socket_path = native_app_socket_path();
            let listener = UnixListener::bind(&socket_path).unwrap();
            fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o666)).unwrap();

            let bundle_dir = native_app_bundle_install_path();
            fs::create_dir_all(bundle_dir.parent().unwrap()).unwrap();
            fs::create_dir_all(bundle_dir.join("Contents").join("MacOS")).unwrap();

            let executable = native_app_executable_in_bundle(&bundle_dir);
            fs::write(
                &executable,
                r##"#!/usr/bin/env python3
import json
import sys

payload = json.loads(sys.argv[3])
print(json.dumps({
  "ok": True,
  "code": 0,
  "payload": {
    "status": "approved",
    "url": payload["url"],
    "domain": "example.com",
    "username": "demo@example.com",
    "password": "fallback-secret",
    "transport": "direct_exec",
    "intent": sys.argv[2],
    "userMediated": True
  }
}))
"##,
            )
            .unwrap();
            let mut permissions = fs::metadata(&executable).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&executable, permissions).unwrap();

            let payload = native_app_login("https://example.com").unwrap();
            assert_eq!(payload["transport"], "direct_exec");

            drop(listener);
        });
    }

    #[test]
    #[serial]
    fn stale_socket_file_falls_back_to_direct_exec() {
        with_temp_home(|| {
            ensure_runtime_dir().unwrap();
            let socket_path = native_app_socket_path();
            fs::write(&socket_path, b"stale").unwrap();
            fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600)).unwrap();

            let bundle_dir = native_app_bundle_install_path();
            fs::create_dir_all(bundle_dir.parent().unwrap()).unwrap();
            fs::create_dir_all(bundle_dir.join("Contents").join("MacOS")).unwrap();

            let executable = native_app_executable_in_bundle(&bundle_dir);
            fs::write(
                &executable,
                r##"#!/usr/bin/env python3
import json
import sys

payload = json.loads(sys.argv[3])
print(json.dumps({
  "ok": True,
  "code": 0,
  "payload": {
    "status": "approved",
    "url": payload["url"],
    "domain": "example.com",
    "username": "demo@example.com",
    "password": "fallback-secret",
    "transport": "direct_exec",
    "intent": sys.argv[2],
    "userMediated": True
  }
}))
"##,
            )
            .unwrap();
            let mut permissions = fs::metadata(&executable).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&executable, permissions).unwrap();

            let payload = native_app_login("https://example.com").unwrap();
            assert_eq!(payload["transport"], "direct_exec");
        });
    }
}
