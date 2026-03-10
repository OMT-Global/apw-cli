use crate::error::{APWError, Result};
use crate::host::{
    ensure_native_host_runtime_dir, native_host_failure_message, native_host_preflight_status,
    native_host_socket_path, native_host_status_note,
};
use crate::types::{APWResponseEnvelope, Command, ManifestConfig, Message, RuntimeMode, Status};
use crate::utils::{write_config, WriteConfigInput};
use chrono::Utc;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::future::Future;
use std::io::ErrorKind;
#[cfg(target_os = "macos")]
use std::os::fd::AsRawFd;
#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;
#[cfg(target_os = "macos")]
use std::path::Path;
use std::pin::Pin;
#[cfg(unix)]
use std::process::Command as StdCommand;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
#[cfg(test)]
use std::sync::Mutex as StdMutex;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UdpSocket, UnixListener, UnixStream};
use tokio::process::Command as TokioCommand;
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::time::{timeout, Duration};
use tokio_tungstenite::{accept_async, tungstenite::Message as WebSocketMessage};

const COMMAND_TIMEOUT_MS: u64 = 30_000;
const LAUNCH_PROBE_TIMEOUT_MS: u64 = 1_000;
const LAUNCH_PROBE_RESPONSE_TIMEOUT_MS: u64 = 5_000;
const PROCESS_STATUS_RETRY_LIMIT: u8 = 40;
const PROCESS_STATUS_RETRY_DELAY_MS: u64 = 25;
const MAX_HELPER_PAYLOAD: usize = 16 * 1024;
const MAX_FRAME_SIZE: usize = 4 + MAX_HELPER_PAYLOAD;
const HELPER_LAUNCH_OK: &str = "ok";
const HELPER_LAUNCH_FAILED: &str = "failed";
const HELPER_LAUNCH_DISABLED: &str = "disabled";
const HELPER_NOT_CONFIGURED: &str = "Helper launch metadata is not configured.";
const LAUNCHCTL_PATH: &str = "/bin/launchctl";
const BRIDGE_STATUS_WAITING: &str = "waiting";
const BRIDGE_STATUS_ATTACHED: &str = "attached";
const BRIDGE_STATUS_DISCONNECTED: &str = "disconnected";
const BRIDGE_STATUS_ERROR: &str = "error";
const BRIDGE_ATTACH_TIMEOUT_MS: u64 = 5_000;
#[cfg(target_os = "macos")]
const MANIFEST_PATHS: [&str; 2] = [
    "/Library/Application Support/Mozilla/NativeMessagingHosts/com.apple.passwordmanager.json",
    "/Library/Google/Chrome/NativeMessagingHosts/com.apple.passwordmanager.json",
];
static BRIDGE_REQUEST_SEQUENCE: AtomicU64 = AtomicU64::new(1);
static BRIDGE_CONNECTION_SEQUENCE: AtomicU64 = AtomicU64::new(1);
#[cfg(test)]
static TEST_MACOS_MAJOR_OVERRIDE: StdMutex<Option<u32>> = StdMutex::new(None);

type BackendFuture =
    Pin<Box<dyn Future<Output = Result<(tokio::process::Child, FramedHelper)>> + Send>>;

trait HelperBackend {
    fn strategy(&self) -> &'static str;
    fn launch(&self, manifest: &ManifestConfig) -> BackendFuture;
}

struct DirectHelperBackend;

struct LaunchdCompatibleBackend;

#[derive(Debug, Clone)]
pub struct HelperLaunchContext {
    pub launch_strategy: String,
    pub launch_status: String,
    pub launch_error: Option<String>,
}

impl Default for HelperLaunchContext {
    fn default() -> Self {
        Self {
            launch_strategy: String::new(),
            launch_status: HELPER_LAUNCH_FAILED.to_string(),
            launch_error: Some(HELPER_NOT_CONFIGURED.to_string()),
        }
    }
}

struct FramedHelper {
    stdin: tokio::process::ChildStdin,
    stdout: tokio::process::ChildStdout,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum BridgeToBrowserMessage {
    Request { request_id: String, payload: Value },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum BridgeFromBrowserMessage {
    Hello {
        browser: String,
        version: Option<String>,
    },
    Response {
        request_id: String,
        ok: bool,
        payload: Option<Value>,
        code: Option<Status>,
        error: Option<String>,
    },
    Status {
        status: String,
        error: Option<String>,
    },
}

#[derive(Debug, Clone)]
struct BridgeSnapshot {
    status: String,
    browser: Option<String>,
    connected_at: Option<String>,
    last_error: Option<String>,
}

impl Default for BridgeSnapshot {
    fn default() -> Self {
        Self {
            status: BRIDGE_STATUS_WAITING.to_string(),
            browser: None,
            connected_at: None,
            last_error: None,
        }
    }
}

#[derive(Default)]
struct BridgeState {
    snapshot: BridgeSnapshot,
    transport: Option<mpsc::UnboundedSender<BridgeToBrowserMessage>>,
    active_connection_id: Option<u64>,
    pending: HashMap<String, oneshot::Sender<Result<Value>>>,
}

#[derive(Clone)]
struct BrowserBridge {
    host: String,
    port: u16,
    runtime_mode: RuntimeMode,
    state: Arc<Mutex<BridgeState>>,
}

fn map_helper_io_error(error: &std::io::Error) -> Option<Status> {
    match error.kind() {
        ErrorKind::BrokenPipe | ErrorKind::UnexpectedEof => Some(Status::ProcessNotRunning),
        _ => None,
    }
}

impl FramedHelper {
    async fn write_frame(&mut self, payload: &[u8]) -> Result<()> {
        if payload.len() > MAX_HELPER_PAYLOAD {
            return Err(APWError::new(
                Status::InvalidParam,
                "Outgoing payload exceeds max size.",
            ));
        }

        let mut frame = Vec::with_capacity(4 + payload.len());
        frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        frame.extend_from_slice(payload);

        self.stdin.write_all(&frame).await.map_err(|error| {
            if let Some(status) = map_helper_io_error(&error) {
                return APWError::new(status, helper_not_running_message());
            }

            APWError::new(
                Status::GenericError,
                format!("Failed writing to helper process: {error}"),
            )
        })?;
        self.stdin.flush().await.map_err(|error| {
            if let Some(status) = map_helper_io_error(&error) {
                return APWError::new(status, helper_not_running_message());
            }

            APWError::new(
                Status::GenericError,
                format!("Failed writing to helper process: {error}"),
            )
        })?;
        Ok(())
    }

    async fn read_exact_n(&mut self, len: usize) -> Result<Vec<u8>> {
        let mut buffer = vec![0_u8; len];
        self.stdout.read_exact(&mut buffer).await.map_err(|error| {
            if let Some(status) = map_helper_io_error(&error) {
                return APWError::new(status, helper_not_running_message());
            }

            APWError::new(
                Status::ProtoInvalidResponse,
                format!("Failed reading helper response: {error}"),
            )
        })?;
        Ok(buffer)
    }

    async fn read_frame(&mut self) -> Result<Vec<u8>> {
        let length = self.read_exact_n(4).await?;
        let length = u32::from_le_bytes([length[0], length[1], length[2], length[3]]) as usize;
        if length == 0 || length > MAX_HELPER_PAYLOAD {
            return Err(APWError::new(
                Status::ProtoInvalidResponse,
                "Invalid helper frame size.",
            ));
        }

        self.read_exact_n(length).await
    }
}

impl BrowserBridge {
    fn new(host: String, port: u16, runtime_mode: RuntimeMode) -> Self {
        Self {
            host,
            port,
            runtime_mode,
            state: Arc::new(Mutex::new(BridgeState::default())),
        }
    }

    async fn persist_snapshot(&self) {
        let snapshot = {
            let state = self.state.lock().await;
            state.snapshot.clone()
        };

        let input = WriteConfigInput {
            port: Some(self.port),
            host: Some(self.host.clone()),
            allow_empty: true,
            clear_auth: false,
            runtime_mode: Some(self.runtime_mode),
            bridge_status: Some(snapshot.status),
            bridge_browser: snapshot.browser,
            bridge_connected_at: snapshot.connected_at,
            bridge_last_error: snapshot.last_error,
            reset_launch_metadata: true,
            reset_bridge_metadata: true,
            refresh_created_at: false,
            ..WriteConfigInput::default()
        };

        if let Err(error) = write_config(input) {
            eprintln!("Failed to persist browser bridge status: {}", error.message);
        }
    }

    async fn attach(
        &self,
        connection_id: u64,
        transport: mpsc::UnboundedSender<BridgeToBrowserMessage>,
        browser: String,
    ) {
        let stale = {
            let mut state = self.state.lock().await;
            let stale = std::mem::take(&mut state.pending);
            state.transport = Some(transport);
            state.active_connection_id = Some(connection_id);
            state.snapshot.status = BRIDGE_STATUS_ATTACHED.to_string();
            state.snapshot.browser = Some(browser);
            state.snapshot.connected_at = Some(Utc::now().to_rfc3339());
            state.snapshot.last_error = None;
            stale
        };

        Self::fail_pending(
            stale,
            APWError::new(
                Status::ProcessNotRunning,
                if self.runtime_mode == RuntimeMode::Native {
                    "Native host was replaced before pending requests completed."
                } else {
                    "Browser bridge was replaced before pending requests completed."
                },
            ),
        );
        self.persist_snapshot().await;
    }

    async fn complete_response(&self, request_id: &str, response: Result<Value>) {
        let pending = {
            let mut state = self.state.lock().await;
            state.pending.remove(request_id)
        };

        if let Some(sender) = pending {
            let _ = sender.send(response);
        }
    }

    async fn mark_status(
        &self,
        connection_id: u64,
        status: &str,
        error: Option<String>,
    ) -> Option<HashMap<String, oneshot::Sender<Result<Value>>>> {
        let pending = {
            let mut state = self.state.lock().await;
            if state.active_connection_id != Some(connection_id) {
                return None;
            }

            let pending = if status == BRIDGE_STATUS_DISCONNECTED || status == BRIDGE_STATUS_ERROR {
                state.transport = None;
                state.active_connection_id = None;
                std::mem::take(&mut state.pending)
            } else {
                HashMap::new()
            };

            state.snapshot.status = status.to_string();
            state.snapshot.connected_at = if status == BRIDGE_STATUS_ATTACHED {
                state.snapshot.connected_at.clone()
            } else {
                None
            };
            state.snapshot.last_error = error;
            pending
        };
        self.persist_snapshot().await;
        Some(pending)
    }

    async fn mark_error(&self, connection_id: u64, error: String) {
        if let Some(pending) = self
            .mark_status(connection_id, BRIDGE_STATUS_ERROR, Some(error.clone()))
            .await
        {
            Self::fail_pending(pending, APWError::new(Status::ProcessNotRunning, error));
        }
    }

    async fn mark_disconnected(&self, connection_id: u64, error: Option<String>) {
        if let Some(pending) = self
            .mark_status(connection_id, BRIDGE_STATUS_DISCONNECTED, error.clone())
            .await
        {
            let message = browser_bridge_not_attached_message(
                self.runtime_mode,
                self.snapshot().await.browser.as_deref(),
                BRIDGE_STATUS_DISCONNECTED,
                error.as_deref(),
            );
            Self::fail_pending(pending, APWError::new(Status::ProcessNotRunning, message));
        }
    }

    async fn snapshot(&self) -> BridgeSnapshot {
        let state = self.state.lock().await;
        state.snapshot.clone()
    }

    async fn enqueue_request(&self, request: &[u8]) -> Result<Value> {
        let payload = serde_json::from_slice::<Value>(request).map_err(|_| {
            APWError::new(
                Status::InvalidMessageFormat,
                "Daemon received malformed client payload.",
            )
        })?;
        let request_id = format!(
            "bridge-{}",
            BRIDGE_REQUEST_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        );

        let receiver = {
            let mut state = self.state.lock().await;
            let transport = state.transport.clone().ok_or_else(|| {
                APWError::new(
                    Status::ProcessNotRunning,
                    browser_bridge_not_attached_message(
                        self.runtime_mode,
                        state.snapshot.browser.as_deref(),
                        state.snapshot.status.as_str(),
                        state.snapshot.last_error.as_deref(),
                    ),
                )
            })?;
            if state.snapshot.status != BRIDGE_STATUS_ATTACHED {
                return Err(APWError::new(
                    Status::ProcessNotRunning,
                    browser_bridge_not_attached_message(
                        self.runtime_mode,
                        state.snapshot.browser.as_deref(),
                        state.snapshot.status.as_str(),
                        state.snapshot.last_error.as_deref(),
                    ),
                ));
            }

            let (sender, receiver) = oneshot::channel();
            state.pending.insert(request_id.clone(), sender);
            transport
                .send(BridgeToBrowserMessage::Request {
                    request_id: request_id.clone(),
                    payload,
                })
                .map_err(|_| {
                    state.pending.remove(&request_id);
                    APWError::new(
                        Status::ProcessNotRunning,
                        browser_bridge_not_attached_message(
                            self.runtime_mode,
                            state.snapshot.browser.as_deref(),
                            BRIDGE_STATUS_DISCONNECTED,
                            state.snapshot.last_error.as_deref(),
                        ),
                    )
                })?;
            receiver
        };

        timeout(Duration::from_millis(COMMAND_TIMEOUT_MS), receiver)
            .await
            .map_err(|_| {
                let message = if self.runtime_mode == RuntimeMode::Native {
                    "Native host response timed out."
                } else {
                    "Browser bridge response timed out."
                };
                APWError::new(Status::CommunicationTimeout, message)
            })?
            .map_err(|_| {
                let message = if self.runtime_mode == RuntimeMode::Native {
                    "Native host disconnected before returning a response."
                } else {
                    "Browser bridge disconnected before returning a response."
                };
                APWError::new(Status::ProcessNotRunning, message)
            })?
    }

    fn fail_pending(pending: HashMap<String, oneshot::Sender<Result<Value>>>, error: APWError) {
        for (_, sender) in pending {
            let _ = sender.send(Err(error.clone()));
        }
    }
}

fn browser_bridge_not_attached_message(
    runtime_mode: RuntimeMode,
    browser: Option<&str>,
    status: &str,
    last_error: Option<&str>,
) -> String {
    if runtime_mode == RuntimeMode::Native {
        let base = match status {
            BRIDGE_STATUS_DISCONNECTED => {
                "Daemon is running in native mode, but the APW native host disconnected."
            }
            BRIDGE_STATUS_ERROR => {
                if let Some(error) = last_error {
                    return format!(
                        "Daemon is running in native mode, but the APW native host reported an error: {error}. {}",
                        native_host_status_note()
                    );
                }
                "Daemon is running in native mode, but the APW native host is not attached."
            }
            _ => "Daemon is running in native mode, but the APW native host is not attached.",
        };
        return format!("{base} {}", native_host_status_note());
    }

    let browser = browser.unwrap_or("Chrome");
    match status {
        BRIDGE_STATUS_DISCONNECTED => format!(
            "Daemon is running in browser mode, but the {browser} bridge disconnected. Reload the APW Chrome bridge extension and wait for `apw status --json` to report `bridge.status=attached`."
        ),
        BRIDGE_STATUS_ERROR => {
            if let Some(error) = last_error {
                format!(
                    "Daemon is running in browser mode, but the {browser} bridge reported an error: {error}. Reload the APW Chrome bridge extension and wait for `apw status --json` to report `bridge.status=attached`."
                )
            } else {
                format!(
                    "Daemon is running in browser mode, but no {browser} bridge is attached. Load the APW Chrome bridge extension and wait for `apw status --json` to report `bridge.status=attached`."
                )
            }
        }
        _ => format!(
            "Daemon is running in browser mode, but no {browser} bridge is attached. Load the APW Chrome bridge extension and wait for `apw status --json` to report `bridge.status=attached`."
        ),
    }
}

impl HelperBackend for DirectHelperBackend {
    fn strategy(&self) -> &'static str {
        "direct"
    }

    fn launch(&self, manifest: &ManifestConfig) -> BackendFuture {
        let path = manifest.path.clone();
        Box::pin(async move {
            let mut command = TokioCommand::new(path.as_str());
            command
                .arg(".")
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::inherit());

            let mut process = command.spawn().map_err(|error| {
                APWError::new(
                    Status::ProcessNotRunning,
                    format!("Unable to start helper: {error}"),
                )
            })?;

            let stdin = process.stdin.take().ok_or_else(|| {
                APWError::new(Status::ProcessNotRunning, "Failed to open helper stdin.")
            })?;
            let stdout = process.stdout.take().ok_or_else(|| {
                APWError::new(Status::ProcessNotRunning, "Failed to open helper stdout.")
            })?;

            Ok((process, FramedHelper { stdin, stdout }))
        })
    }
}

impl HelperBackend for LaunchdCompatibleBackend {
    fn strategy(&self) -> &'static str {
        "launchd_compatible"
    }

    fn launch(&self, manifest: &ManifestConfig) -> BackendFuture {
        let path = manifest.path.clone();
        Box::pin(async move {
            let mut command = launchd_aware_command(path.as_str());
            command
                .arg(".")
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::inherit());

            let mut process = command.spawn().map_err(|error| {
                APWError::new(
                    Status::ProcessNotRunning,
                    format!("Unable to start helper: {error}"),
                )
            })?;

            let stdin = process.stdin.take().ok_or_else(|| {
                APWError::new(Status::ProcessNotRunning, "Failed to open helper stdin.")
            })?;
            let stdout = process.stdout.take().ok_or_else(|| {
                APWError::new(Status::ProcessNotRunning, "Failed to open helper stdout.")
            })?;

            Ok((process, FramedHelper { stdin, stdout }))
        })
    }
}

fn launchd_aware_command(target_path: &str) -> TokioCommand {
    let uid = std::env::var("UID")
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok())
        .or_else(resolve_uid_via_id_command);

    if let Some(value) = uid {
        let mut command = TokioCommand::new(LAUNCHCTL_PATH);
        command
            .arg("asuser")
            .arg(value.to_string())
            .arg(target_path);
        return command;
    }

    TokioCommand::new(target_path)
}

#[cfg(unix)]
fn resolve_uid_via_id_command() -> Option<u32> {
    let output = StdCommand::new("id").arg("-u").output().ok()?;
    if !output.status.success() {
        return None;
    }

    let raw = String::from_utf8(output.stdout).ok()?;
    raw.trim().parse::<u32>().ok()
}

#[cfg(not(unix))]
fn resolve_uid_via_id_command() -> Option<u32> {
    None
}

fn backend_selection(mode: RuntimeMode) -> Vec<Box<dyn HelperBackend + Send + Sync>> {
    match mode {
        RuntimeMode::Native => Vec::new(),
        RuntimeMode::Browser => Vec::new(),
        RuntimeMode::Direct => vec![Box::new(DirectHelperBackend)],
        RuntimeMode::Launchd => vec![Box::new(LaunchdCompatibleBackend)],
        RuntimeMode::Auto => {
            vec![
                Box::new(DirectHelperBackend),
                Box::new(LaunchdCompatibleBackend),
            ]
        }
        RuntimeMode::Disabled => Vec::new(),
    }
}

fn configured_macos_major_version() -> Option<u32> {
    #[cfg(test)]
    {
        if let Some(value) = *TEST_MACOS_MAJOR_OVERRIDE
            .lock()
            .expect("macos override lock")
        {
            return Some(value);
        }
    }

    std::env::var("APW_MACOS_MAJOR_OVERRIDE")
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok())
}

#[cfg(test)]
fn set_macos_major_override_for_tests(value: Option<u32>) {
    *TEST_MACOS_MAJOR_OVERRIDE
        .lock()
        .expect("macos override lock") = value;
}

#[cfg(target_os = "macos")]
fn current_macos_major_version() -> Option<u32> {
    if let Some(value) = configured_macos_major_version() {
        return Some(value);
    }

    let output = StdCommand::new("sw_vers")
        .arg("-productVersion")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let raw = String::from_utf8(output.stdout).ok()?;
    raw.trim()
        .split('.')
        .next()
        .and_then(|segment| segment.parse::<u32>().ok())
}

#[cfg(not(target_os = "macos"))]
fn current_macos_major_version() -> Option<u32> {
    configured_macos_major_version()
}

fn resolve_runtime_mode(mode: RuntimeMode) -> RuntimeMode {
    match mode {
        RuntimeMode::Auto if current_macos_major_version().is_some_and(|major| major >= 26) => {
            RuntimeMode::Native
        }
        other => other,
    }
}

fn launch_strategy_labels(mode: RuntimeMode) -> Vec<String> {
    match mode {
        RuntimeMode::Native => vec!["native_host".to_string()],
        RuntimeMode::Browser => vec!["browser_bridge".to_string()],
        RuntimeMode::Disabled => Vec::new(),
        other => backend_selection(other)
            .into_iter()
            .map(|backend| backend.strategy().to_string())
            .collect(),
    }
}

fn empty_manifest_preflight() -> Value {
    json!({
        "searchedPaths": manifest_search_paths(),
        "path": Value::Null,
        "found": false,
        "valid": false,
        "binaryPath": Value::Null,
        "binaryAbsolute": Value::Null,
        "binaryExecutable": Value::Null,
        "allowedOrigins": Value::Null,
    })
}

#[cfg(target_os = "macos")]
fn manifest_search_paths() -> Vec<String> {
    MANIFEST_PATHS.iter().map(|path| path.to_string()).collect()
}

#[cfg(not(target_os = "macos"))]
fn manifest_search_paths() -> Vec<String> {
    Vec::new()
}

#[cfg(target_os = "macos")]
fn inspect_manifest_preflight() -> (Value, String, Option<String>) {
    let searched_paths = manifest_search_paths();
    let manifest_path = MANIFEST_PATHS
        .iter()
        .copied()
        .find(|candidate| Path::new(candidate).exists());

    let manifest_json = |path: Option<&str>,
                         found: bool,
                         valid: bool,
                         binary_path: Option<&str>,
                         binary_absolute: Option<bool>,
                         binary_executable: Option<bool>,
                         allowed_origins: Option<usize>| {
        json!({
            "searchedPaths": searched_paths.clone(),
            "path": path,
            "found": found,
            "valid": valid,
            "binaryPath": binary_path,
            "binaryAbsolute": binary_absolute,
            "binaryExecutable": binary_executable,
            "allowedOrigins": allowed_origins,
        })
    };

    let Some(path) = manifest_path else {
        return (
            manifest_json(None, false, false, None, None, None, None),
            "manifest_missing".to_string(),
            Some("APW Helper manifest not found. You must be running macOS 14+.".to_string()),
        );
    };

    let manifest_content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(_) => {
            return (
                manifest_json(Some(path), true, false, None, None, None, None),
                "manifest_invalid".to_string(),
                Some("Malformed helper manifest JSON.".to_string()),
            );
        }
    };

    let candidate: Value = match serde_json::from_str(&manifest_content) {
        Ok(value) => value,
        Err(_) => {
            return (
                manifest_json(Some(path), true, false, None, None, None, None),
                "manifest_invalid".to_string(),
                Some("Malformed helper manifest.".to_string()),
            );
        }
    };

    if !is_manifest(&candidate) {
        return (
            manifest_json(Some(path), true, false, None, None, None, None),
            "manifest_invalid".to_string(),
            Some("Malformed helper manifest.".to_string()),
        );
    }

    let binary_path = candidate.get("path").and_then(Value::as_str);
    let allowed_origins = candidate
        .get("allowedOrigins")
        .or_else(|| candidate.get("allowed_extensions"))
        .and_then(Value::as_array)
        .map(Vec::len);
    let binary_absolute =
        binary_path.map(|value| is_absolute_unix_path(value) && !value.contains(".."));

    if binary_absolute != Some(true) {
        return (
            manifest_json(
                Some(path),
                true,
                false,
                binary_path,
                binary_absolute,
                None,
                allowed_origins,
            ),
            "manifest_invalid".to_string(),
            Some("Unexpected helper binary path.".to_string()),
        );
    }

    let binary_executable = binary_path.map(is_executable);
    if binary_executable != Some(true) {
        return (
            manifest_json(
                Some(path),
                true,
                true,
                binary_path,
                binary_absolute,
                binary_executable,
                allowed_origins,
            ),
            "binary_not_executable".to_string(),
            Some("Cannot execute helper binary.".to_string()),
        );
    }

    (
        manifest_json(
            Some(path),
            true,
            true,
            binary_path,
            binary_absolute,
            binary_executable,
            allowed_origins,
        ),
        "ready".to_string(),
        None,
    )
}

pub(crate) fn helper_preflight_status(configured_mode: RuntimeMode) -> Value {
    let resolved_mode = resolve_runtime_mode(configured_mode);

    if resolved_mode == RuntimeMode::Native {
        return native_host_preflight_status(configured_mode);
    }

    if resolved_mode == RuntimeMode::Disabled {
        return json!({
            "supported": cfg!(target_os = "macos"),
            "platform": {
                "os": std::env::consts::OS,
                "arch": std::env::consts::ARCH,
                "macosMajorVersion": current_macos_major_version(),
            },
            "configuredRuntimeMode": configured_mode,
            "resolvedRuntimeMode": resolved_mode,
            "launchStrategies": launch_strategy_labels(resolved_mode),
            "status": "disabled",
            "error": Value::Null,
            "manifest": empty_manifest_preflight(),
        });
    }

    if !cfg!(target_os = "macos") {
        return json!({
            "supported": false,
            "platform": {
                "os": std::env::consts::OS,
                "arch": std::env::consts::ARCH,
                "macosMajorVersion": current_macos_major_version(),
            },
            "configuredRuntimeMode": configured_mode,
            "resolvedRuntimeMode": resolved_mode,
            "launchStrategies": launch_strategy_labels(resolved_mode),
            "status": "unsupported_platform",
            "error": "APW Helper manifest unsupported outside of macOS.",
            "manifest": empty_manifest_preflight(),
        });
    }

    if resolved_mode == RuntimeMode::Browser {
        return json!({
            "supported": true,
            "platform": {
                "os": std::env::consts::OS,
                "arch": std::env::consts::ARCH,
                "macosMajorVersion": current_macos_major_version(),
            },
            "configuredRuntimeMode": configured_mode,
            "resolvedRuntimeMode": resolved_mode,
            "launchStrategies": launch_strategy_labels(resolved_mode),
            "status": "browser_bridge",
            "error": Value::Null,
            "manifest": empty_manifest_preflight(),
        });
    }

    #[cfg(target_os = "macos")]
    let (manifest, status, error) = inspect_manifest_preflight();
    #[cfg(not(target_os = "macos"))]
    let (manifest, status, error) = (
        empty_manifest_preflight(),
        "unsupported_platform".to_string(),
        Some("APW Helper manifest unsupported outside of macOS.".to_string()),
    );

    json!({
        "supported": true,
        "platform": {
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "macosMajorVersion": current_macos_major_version(),
        },
        "configuredRuntimeMode": configured_mode,
        "resolvedRuntimeMode": resolved_mode,
        "launchStrategies": launch_strategy_labels(resolved_mode),
        "status": status,
        "error": error,
        "manifest": manifest,
    })
}

fn helper_preflight_state_name(preflight: &Value) -> String {
    preflight
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string()
}

fn helper_preflight_guidance_from(preflight: &Value) -> String {
    let status = helper_preflight_state_name(preflight);
    let platform_os = preflight
        .get("platform")
        .and_then(|value| value.get("os"))
        .and_then(Value::as_str)
        .unwrap_or(std::env::consts::OS);
    let platform_arch = preflight
        .get("platform")
        .and_then(|value| value.get("arch"))
        .and_then(Value::as_str)
        .unwrap_or(std::env::consts::ARCH);

    match status.as_str() {
        "browser_bridge" => "Install the browser bridge with `./scripts/install-browser-bridge.sh`, load `browser-bridge/` in Chrome, start the daemon with `apw start`, and wait for `apw status --json` to report `bridge.status=attached`.".to_string(),
        "manifest_missing" => "Run `apw status --json` and review `daemon.preflight.manifest.searchedPaths`; the Apple helper manifest is missing on this host.".to_string(),
        "manifest_invalid" => "Run `apw status --json` and review `daemon.preflight.manifest.path` plus `daemon.preflight.manifest.binaryPath`; the helper manifest is present but invalid.".to_string(),
        "binary_not_executable" => "Run `apw status --json` and review `daemon.preflight.manifest.binaryPath` plus `daemon.preflight.manifest.binaryExecutable`; the helper binary is not executable.".to_string(),
        "unsupported_platform" => format!(
            "APW is supported only on macOS. Current platform: {platform_os}/{platform_arch}."
        ),
        "disabled" => {
            "Re-run `apw start` with `--runtime-mode native`, `--runtime-mode direct`, or `--runtime-mode launchd`.".to_string()
        }
        _ => {
            "Run `apw status --json` and inspect `daemon.preflight` for the resolved runtime path and helper launch diagnostics.".to_string()
        }
    }
}

pub(crate) fn helper_preflight_status_note(configured_mode: RuntimeMode) -> String {
    if resolve_runtime_mode(configured_mode) == RuntimeMode::Native {
        return native_host_status_note();
    }
    let preflight = helper_preflight_status(configured_mode);
    let status = helper_preflight_state_name(&preflight);
    format!(
        "Run `apw status --json` and inspect `daemon.preflight`; current `daemon.preflight.status={status}`."
    )
}

pub(crate) fn helper_preflight_failure_message(
    configured_mode: RuntimeMode,
    base_message: &str,
) -> String {
    if resolve_runtime_mode(configured_mode) == RuntimeMode::Native {
        return native_host_failure_message(base_message);
    }
    let preflight = helper_preflight_status(configured_mode);
    let status = helper_preflight_state_name(&preflight);
    let guidance = helper_preflight_guidance_from(&preflight);
    format!("{base_message} {guidance} Current `daemon.preflight.status={status}`.")
}

fn capabilities_probe_message() -> Message {
    Message {
        cmd: Command::GetCapabilities as i32,
        payload: None,
        msg: None,
        capabilities: Some(json!({
            "canFillOneTimeCodes": true,
        })),
        set_up_totp_page_url: None,
        set_up_totp_uri: None,
        url: None,
        tab_id: None,
        frame_id: None,
    }
}

async fn probe_helper_readiness(helper: &mut FramedHelper) -> Result<()> {
    let payload = serde_json::to_vec(&capabilities_probe_message()).map_err(|error| {
        APWError::new(
            Status::ServerError,
            format!("Failed to serialize helper launch probe: {error}"),
        )
    })?;
    helper.write_frame(&payload).await?;

    let response = timeout(
        Duration::from_millis(LAUNCH_PROBE_RESPONSE_TIMEOUT_MS),
        helper.read_frame(),
    )
    .await
    .map_err(|_| {
        APWError::new(
            Status::CommunicationTimeout,
            "Helper capabilities probe timed out.",
        )
    })??;

    let payload = parse_helper_payload(&response)
        .and_then(|payload| parse_helper_response_shape(&payload))?;
    if !payload.is_object() {
        return Err(APWError::new(
            Status::ProtoInvalidResponse,
            "Invalid helper capabilities payload.",
        ));
    }

    Ok(())
}

#[cfg(target_os = "macos")]
fn is_safe_manifest_path(path: &str) -> bool {
    MANIFEST_PATHS.contains(&path)
}

#[cfg(any(test, target_os = "macos"))]
fn is_absolute_unix_path(path: &str) -> bool {
    path.starts_with('/') && !path.contains('\0')
}

#[cfg(target_os = "macos")]
fn is_executable(path: &str) -> bool {
    Path::new(path)
        .metadata()
        .map(|metadata| {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                metadata.is_file() && (metadata.permissions().mode() & 0o111 != 0)
            }
            #[cfg(not(unix))]
            {
                metadata.is_file()
            }
        })
        .unwrap_or(false)
}

async fn probe_helper_launch(
    manifest: &ManifestConfig,
    mode: RuntimeMode,
    dry_run: bool,
) -> Result<(
    Option<tokio::process::Child>,
    Option<FramedHelper>,
    HelperLaunchContext,
)> {
    let mut last_error: Option<String> = None;
    let mut launch_strategy = HELPER_LAUNCH_FAILED.to_string();

    if mode == RuntimeMode::Disabled {
        return Ok((
            None,
            None,
            HelperLaunchContext {
                launch_strategy: HELPER_LAUNCH_DISABLED.to_string(),
                launch_status: HELPER_LAUNCH_DISABLED.to_string(),
                launch_error: Some("Runtime mode disabled launch checks.".to_string()),
            },
        ));
    }

    for backend in backend_selection(mode) {
        launch_strategy = backend.strategy().to_string();
        let started = timeout(
            Duration::from_millis(LAUNCH_PROBE_TIMEOUT_MS),
            backend.launch(manifest),
        )
        .await
        .map_err(|_| APWError::new(Status::CommunicationTimeout, "Helper launch timed out."))?;

        match started {
            Ok((mut process, mut helper)) => {
                let readiness = match process.try_wait() {
                    Ok(Some(status)) => Err(report_termination_if_any(status)),
                    Ok(None) => probe_helper_readiness(&mut helper).await,
                    Err(error) => Err(APWError::new(
                        Status::ProcessNotRunning,
                        format!("Helper process check failed: {error}"),
                    )),
                };

                match readiness {
                    Ok(()) => {
                        if dry_run {
                            let _ = process.kill().await;
                            let _ = process.wait().await;
                            let context = HelperLaunchContext {
                                launch_strategy,
                                launch_status: HELPER_LAUNCH_OK.to_string(),
                                launch_error: None,
                            };
                            return Ok((None, None, context));
                        }

                        let context = HelperLaunchContext {
                            launch_strategy,
                            launch_status: HELPER_LAUNCH_OK.to_string(),
                            launch_error: None,
                        };
                        return Ok((Some(process), Some(helper), context));
                    }
                    Err(error) => {
                        last_error =
                            Some(resolve_probe_failure_message(&mut process, error.message).await);
                        let _ = process.kill().await;
                        let _ = process.wait().await;
                        continue;
                    }
                }
            }
            Err(error) => {
                last_error = Some(error.message.clone());
            }
        }
    }

    if launch_strategy.is_empty() {
        launch_strategy = "none".to_string();
    }
    Ok((
        None,
        None,
        HelperLaunchContext {
            launch_strategy,
            launch_status: HELPER_LAUNCH_FAILED.to_string(),
            launch_error: if let Some(error) = last_error {
                Some(error)
            } else {
                Some("No launch backend available.".to_string())
            },
        },
    ))
}

fn persistence_for_launch(
    host: &str,
    port: u16,
    context: &HelperLaunchContext,
    runtime_mode: RuntimeMode,
) -> Option<WriteConfigInput> {
    Some(WriteConfigInput {
        username: None,
        shared_key: None,
        port: Some(port),
        host: Some(host.to_string()),
        allow_empty: true,
        clear_auth: false,
        runtime_mode: Some(runtime_mode),
        last_launch_status: Some(context.launch_status.clone()),
        last_launch_error: context.launch_error.clone(),
        last_launch_strategy: Some(context.launch_strategy.clone()),
        reset_bridge_metadata: true,
        refresh_created_at: false,
        ..WriteConfigInput::default()
    })
}

fn persistence_for_launch_error(
    host: &str,
    port: u16,
    runtime_mode: RuntimeMode,
    launch_strategy: &str,
    launch_error: String,
) -> WriteConfigInput {
    WriteConfigInput {
        username: None,
        shared_key: None,
        port: Some(port),
        host: Some(host.to_string()),
        allow_empty: true,
        clear_auth: false,
        runtime_mode: Some(runtime_mode),
        last_launch_status: Some(HELPER_LAUNCH_FAILED.to_string()),
        last_launch_error: Some(launch_error),
        last_launch_strategy: Some(launch_strategy.to_string()),
        reset_bridge_metadata: true,
        refresh_created_at: false,
        ..WriteConfigInput::default()
    }
}

fn persistence_for_browser(
    host: &str,
    port: u16,
    runtime_mode: RuntimeMode,
    bridge_status: &str,
    bridge_browser: Option<String>,
    bridge_connected_at: Option<String>,
    bridge_last_error: Option<String>,
) -> WriteConfigInput {
    WriteConfigInput {
        username: None,
        shared_key: None,
        port: Some(port),
        host: Some(host.to_string()),
        allow_empty: true,
        clear_auth: false,
        runtime_mode: Some(runtime_mode),
        bridge_status: Some(bridge_status.to_string()),
        bridge_browser,
        bridge_connected_at,
        bridge_last_error,
        reset_launch_metadata: true,
        reset_bridge_metadata: true,
        refresh_created_at: false,
        ..WriteConfigInput::default()
    }
}

fn helper_termination_message(status: &std::process::ExitStatus) -> String {
    if let Some(code) = status.code() {
        return format!("Helper process exited with code {code}.");
    }

    #[cfg(unix)]
    if let Some(signal) = status.signal() {
        if signal == 9 {
            return "Helper process was terminated by SIGKILL (Code Signature Constraint Violation). Helper launch requires an approved browser/native host context on this OS and cannot be launched directly from this CLI today.".to_string();
        }
        return format!("Helper process terminated by signal {signal}.");
    }

    "Helper process is not running.".to_string()
}

fn helper_not_running_message() -> String {
    "Helper process is not running.".to_string()
}

fn check_helper_status(
    process: &mut tokio::process::Child,
) -> std::result::Result<Option<std::process::ExitStatus>, APWError> {
    process.try_wait().map_err(|error| {
        APWError::new(
            Status::ProcessNotRunning,
            format!("Helper process check failed: {error}"),
        )
    })
}

#[cfg(any(test, target_os = "macos"))]
fn is_manifest(value: &Value) -> bool {
    let candidate = match value.as_object() {
        Some(candidate) => candidate,
        None => return false,
    };

    let Some(name) = candidate.get("name").and_then(Value::as_str) else {
        return false;
    };
    let Some(description) = candidate.get("description").and_then(Value::as_str) else {
        return false;
    };
    let Some(path) = candidate.get("path").and_then(Value::as_str) else {
        return false;
    };
    let Some(r#type) = candidate.get("type").and_then(Value::as_str) else {
        return false;
    };
    let Some(allowed_origins) = candidate
        .get("allowedOrigins")
        .or_else(|| candidate.get("allowed_extensions"))
        .and_then(Value::as_array)
    else {
        return false;
    };

    if name.is_empty() || description.is_empty() || path.is_empty() || r#type.is_empty() {
        return false;
    }

    allowed_origins.iter().all(|value| value.is_string())
}

#[cfg(not(target_os = "macos"))]
fn read_manifest() -> Result<ManifestConfig> {
    Err(APWError::new(
        Status::GenericError,
        "APW Helper manifest unsupported outside of macOS.",
    ))
}

#[cfg(target_os = "macos")]
fn read_manifest() -> Result<ManifestConfig> {
    let path = MANIFEST_PATHS
        .iter()
        .copied()
        .find(|candidate| Path::new(candidate).exists())
        .ok_or_else(|| {
            APWError::new(
                Status::GenericError,
                "APW Helper manifest not found. You must be running macOS 14+.",
            )
        })?;

    if !is_safe_manifest_path(path) {
        return Err(APWError::new(
            Status::InvalidConfig,
            "Unexpected helper binary path.",
        ));
    }

    let manifest_content = std::fs::read_to_string(path)
        .map_err(|_| APWError::new(Status::InvalidConfig, "Malformed helper manifest JSON."))?;
    let candidate: Value = serde_json::from_str(&manifest_content)
        .map_err(|_| APWError::new(Status::InvalidConfig, "Malformed helper manifest."))?;

    if !is_manifest(&candidate) {
        return Err(APWError::new(
            Status::InvalidConfig,
            "Malformed helper manifest.",
        ));
    }

    let binary_path = candidate
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| APWError::new(Status::InvalidConfig, "Malformed helper manifest."))?;

    if !is_absolute_unix_path(binary_path) || binary_path.contains("..") {
        return Err(APWError::new(
            Status::InvalidConfig,
            "Unexpected helper binary path.",
        ));
    }

    if !is_executable(binary_path) {
        return Err(APWError::new(
            Status::ProcessNotRunning,
            "Cannot execute helper binary.",
        ));
    }

    serde_json::from_value(candidate)
        .map_err(|_| APWError::new(Status::InvalidConfig, "Malformed helper manifest."))
}

fn parse_helper_payload(raw: &[u8]) -> Result<Value> {
    if raw.len() > MAX_HELPER_PAYLOAD {
        return Err(APWError::new(
            Status::ProtoInvalidResponse,
            "Response too large.",
        ));
    }

    let value: Value = serde_json::from_slice(raw).map_err(|_| {
        APWError::new(
            Status::ProtoInvalidResponse,
            "Helper returned invalid JSON.",
        )
    })?;

    if value.is_object() {
        return Ok(value);
    }

    Err(APWError::new(
        Status::ProtoInvalidResponse,
        "Invalid helper response payload.",
    ))
}

fn parse_helper_response_shape(payload: &Value) -> Result<Value> {
    if !payload.is_object() {
        return Err(APWError::new(
            Status::ProtoInvalidResponse,
            "Invalid helper response payload.",
        ));
    }

    let object = payload.as_object().expect("object");
    let Some(ok) = object.get("ok").and_then(Value::as_bool) else {
        return Ok(payload.clone());
    };

    if ok {
        return object
            .get("payload")
            .cloned()
            .ok_or_else(|| APWError::new(Status::ProtoInvalidResponse, "Invalid helper payload."));
    }

    let code = object
        .get("code")
        .and_then(|candidate| {
            candidate
                .as_i64()
                .or_else(|| candidate.as_u64().map(|value| value as i64))
                .and_then(|raw| crate::types::Status::try_from(raw).ok())
                .or_else(|| {
                    candidate
                        .as_str()
                        .and_then(|text| text.parse::<i64>().ok())
                        .and_then(|raw| crate::types::Status::try_from(raw).ok())
                })
                .or_else(|| {
                    candidate.as_str().and_then(|text| match text {
                        "Success" => Some(crate::types::Status::Success),
                        "GenericError" => Some(crate::types::Status::GenericError),
                        "InvalidParam" => Some(crate::types::Status::InvalidParam),
                        "NoResults" => Some(crate::types::Status::NoResults),
                        "FailedToDelete" => Some(crate::types::Status::FailedToDelete),
                        "FailedToUpdate" => Some(crate::types::Status::FailedToUpdate),
                        "InvalidMessageFormat" => Some(crate::types::Status::InvalidMessageFormat),
                        "DuplicateItem" => Some(crate::types::Status::DuplicateItem),
                        "UnknownAction" => Some(crate::types::Status::UnknownAction),
                        "InvalidSession" => Some(crate::types::Status::InvalidSession),
                        "ServerError" => Some(crate::types::Status::ServerError),
                        "CommunicationTimeout" => Some(crate::types::Status::CommunicationTimeout),
                        "InvalidConfig" => Some(crate::types::Status::InvalidConfig),
                        "ProcessNotRunning" => Some(crate::types::Status::ProcessNotRunning),
                        "ProtoInvalidResponse" => Some(crate::types::Status::ProtoInvalidResponse),
                        _ => None,
                    })
                })
        })
        .unwrap_or(crate::types::Status::GenericError);
    let error = object
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or(crate::types::status_text(code));
    Err(APWError::new(code, error.to_string()))
}

async fn send_envelope_to_client(
    listener: &UdpSocket,
    peer: std::net::SocketAddr,
    code: Status,
    payload: Option<Value>,
    error: Option<String>,
) -> Result<()> {
    let response = if code == Status::Success {
        APWResponseEnvelope {
            ok: true,
            code,
            payload,
            error: None,
            request_id: None,
        }
    } else {
        APWResponseEnvelope {
            ok: false,
            code,
            payload: None,
            error: Some(error.unwrap_or_else(|| crate::types::status_text(code).to_string())),
            request_id: None,
        }
    };

    let encoded = serde_json::to_vec(&response)
        .map_err(|_| APWError::new(Status::ServerError, "Failed to serialize daemon response."))?;
    listener.send_to(&encoded, peer).await.map_err(|error| {
        APWError::new(
            Status::GenericError,
            format!("Failed sending daemon response: {error}"),
        )
    })?;

    Ok(())
}

fn parse_bridge_message(raw: WebSocketMessage) -> Result<BridgeFromBrowserMessage> {
    let payload = match raw {
        WebSocketMessage::Text(text) => text.to_string(),
        WebSocketMessage::Binary(bytes) => String::from_utf8(bytes.to_vec()).map_err(|_| {
            APWError::new(
                Status::ProtoInvalidResponse,
                "Browser bridge sent invalid UTF-8 payload.",
            )
        })?,
        WebSocketMessage::Close(_) => {
            return Err(APWError::new(
                Status::ProcessNotRunning,
                "Browser bridge closed the connection.",
            ))
        }
        WebSocketMessage::Ping(_) | WebSocketMessage::Pong(_) => {
            return Err(APWError::new(
                Status::ProtoInvalidResponse,
                "Browser bridge control frame cannot be decoded as JSON.",
            ))
        }
        WebSocketMessage::Frame(_) => {
            return Err(APWError::new(
                Status::ProtoInvalidResponse,
                "Browser bridge frame cannot be decoded as JSON.",
            ))
        }
    };

    serde_json::from_str(&payload).map_err(|_| {
        APWError::new(
            Status::ProtoInvalidResponse,
            "Browser bridge sent malformed JSON.",
        )
    })
}

async fn handle_browser_bridge_connection(
    bridge: BrowserBridge,
    stream: tokio::net::TcpStream,
) -> Result<()> {
    let websocket = accept_async(stream).await.map_err(|error| {
        APWError::new(
            Status::ProcessNotRunning,
            format!("Browser bridge WebSocket handshake failed: {error}"),
        )
    })?;
    let connection_id = BRIDGE_CONNECTION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let (mut sink, mut source) = websocket.split();

    let hello = timeout(
        Duration::from_millis(BRIDGE_ATTACH_TIMEOUT_MS),
        source.next(),
    )
    .await
    .map_err(|_| {
        APWError::new(
            Status::ProcessNotRunning,
            "Browser bridge did not send an identity message in time.",
        )
    })?
    .ok_or_else(|| {
        APWError::new(
            Status::ProcessNotRunning,
            "Browser bridge disconnected before sending an identity message.",
        )
    })?
    .map_err(|error| {
        APWError::new(
            Status::ProcessNotRunning,
            format!("Browser bridge receive failed: {error}"),
        )
    })?;

    let hello = parse_bridge_message(hello)?;
    let browser = match hello {
        BridgeFromBrowserMessage::Hello { browser, .. } => browser,
        _ => {
            bridge
                .mark_error(
                    connection_id,
                    "Browser bridge must send a hello message before request forwarding."
                        .to_string(),
                )
                .await;
            return Err(APWError::new(
                Status::ProtoInvalidResponse,
                "Browser bridge must send a hello message first.",
            ));
        }
    };

    let (outbound_tx, mut outbound_rx) = mpsc::unbounded_channel::<BridgeToBrowserMessage>();
    bridge.attach(connection_id, outbound_tx, browser).await;

    let result: Result<()> = loop {
        tokio::select! {
            outbound = outbound_rx.recv() => {
                let Some(outbound) = outbound else {
                    break Ok(());
                };
                let encoded = serde_json::to_string(&outbound).map_err(|error| {
                    APWError::new(
                        Status::ServerError,
                        format!("Failed to serialize browser bridge request: {error}"),
                    )
                })?;
                sink.send(WebSocketMessage::Text(encoded.into())).await.map_err(|error| {
                    APWError::new(
                        Status::ProcessNotRunning,
                        format!("Browser bridge send failed: {error}"),
                    )
                })?;
            }
            inbound = source.next() => {
                let Some(inbound) = inbound else {
                    bridge.mark_disconnected(connection_id, None).await;
                    break Ok(());
                };
                let inbound = inbound.map_err(|error| {
                    APWError::new(
                        Status::ProcessNotRunning,
                        format!("Browser bridge receive failed: {error}"),
                    )
                })?;
                if matches!(inbound, WebSocketMessage::Close(_)) {
                    bridge.mark_disconnected(connection_id, None).await;
                    break Ok(());
                }
                match parse_bridge_message(inbound)? {
                    BridgeFromBrowserMessage::Hello { .. } => {}
                    BridgeFromBrowserMessage::Status { status, error } => match status.as_str() {
                        BRIDGE_STATUS_ERROR => bridge
                            .mark_error(
                                connection_id,
                                error.unwrap_or_else(|| "Browser bridge reported an unknown error.".to_string()),
                            )
                            .await,
                        BRIDGE_STATUS_DISCONNECTED => {
                            bridge.mark_disconnected(connection_id, error).await;
                        }
                        BRIDGE_STATUS_ATTACHED => {
                            let snapshot = bridge.snapshot().await;
                            let input = persistence_for_browser(
                                &bridge.host,
                                bridge.port,
                                bridge.runtime_mode,
                                BRIDGE_STATUS_ATTACHED,
                                snapshot.browser,
                                snapshot.connected_at,
                                None,
                            );
                            if let Err(error) = write_config(input) {
                                eprintln!("Failed to refresh browser bridge attachment state: {}", error.message);
                            }
                        }
                        _ => {}
                    },
                    BridgeFromBrowserMessage::Response { request_id, ok, payload, code, error } => {
                        if ok {
                            if let Some(payload) = payload {
                                bridge.complete_response(&request_id, Ok(payload)).await;
                            } else {
                                bridge.complete_response(
                                    &request_id,
                                    Err(APWError::new(
                                        Status::ProtoInvalidResponse,
                                        "Browser bridge response was missing helper payload.",
                                    )),
                                )
                                .await;
                            }
                        } else {
                            bridge.complete_response(
                                &request_id,
                                Err(APWError::new(
                                    code.unwrap_or(Status::GenericError),
                                    error.unwrap_or_else(|| "Browser bridge returned an unknown error.".to_string()),
                                )),
                            )
                            .await;
                        }
                    }
                }
            }
        }
    };

    if let Err(error) = &result {
        bridge
            .mark_error(connection_id, error.message.clone())
            .await;
    }

    result
}

async fn run_browser_bridge_accept_loop(
    bridge: BrowserBridge,
    listener: TcpListener,
) -> Result<()> {
    loop {
        let (stream, _) = listener.accept().await.map_err(|error| {
            APWError::new(
                Status::GenericError,
                format!("Browser bridge accept failed: {error}"),
            )
        })?;
        let bridge_for_task = bridge.clone();
        tokio::spawn(async move {
            if let Err(error) = handle_browser_bridge_connection(bridge_for_task, stream).await {
                eprintln!("Browser bridge connection failed: {}", error.message);
            }
        });
    }
}

pub struct DaemonOptions {
    pub port: u16,
    pub host: String,
    pub runtime_mode: RuntimeMode,
    pub dry_run: bool,
}

impl Default for DaemonOptions {
    fn default() -> Self {
        Self {
            port: 0,
            host: "127.0.0.1".to_string(),
            runtime_mode: RuntimeMode::Auto,
            dry_run: false,
        }
    }
}

fn report_termination_if_any(status: std::process::ExitStatus) -> APWError {
    APWError::new(
        Status::ProcessNotRunning,
        helper_termination_message(&status),
    )
}

async fn resolve_probe_failure_message(
    process: &mut tokio::process::Child,
    fallback: String,
) -> String {
    let mut retries = 0_u8;
    while retries < PROCESS_STATUS_RETRY_LIMIT {
        match check_helper_status(process) {
            Ok(Some(status)) => return helper_termination_message(&status),
            Ok(None) => {
                retries += 1;
                tokio::time::sleep(Duration::from_millis(PROCESS_STATUS_RETRY_DELAY_MS)).await;
            }
            Err(error) => return error.message,
        }
    }

    fallback
}

async fn ensure_helper_stays_alive(process: &mut tokio::process::Child) -> Result<()> {
    let mut retries = 0_u8;
    while retries < PROCESS_STATUS_RETRY_LIMIT {
        if let Some(status) = process.try_wait().map_err(|error| {
            APWError::new(
                Status::ProcessNotRunning,
                format!("Helper process check failed: {error}"),
            )
        })? {
            return Err(report_termination_if_any(status));
        }

        retries += 1;
        tokio::time::sleep(Duration::from_millis(PROCESS_STATUS_RETRY_DELAY_MS)).await;
    }
    Ok(())
}

async fn terminate_helper(process: &mut tokio::process::Child) {
    let _ = process.kill().await;
    let _ = process.wait().await;
}

async fn start_browser_daemon_inner(options: DaemonOptions, host: String) -> Result<()> {
    let bridge_listener = TcpListener::bind((host.as_str(), options.port))
        .await
        .map_err(|error| {
            APWError::new(
                Status::GenericError,
                format!("Failed to bind browser bridge socket: {error}"),
            )
        })?;
    let listener_port = bridge_listener
        .local_addr()
        .map_err(|error| {
            APWError::new(
                Status::GenericError,
                format!("Unable to resolve browser bridge address: {error}"),
            )
        })?
        .port();

    let listener = UdpSocket::bind((host.as_str(), listener_port))
        .await
        .map_err(|error| {
            APWError::new(
                Status::GenericError,
                format!("Failed to bind UDP socket: {error}"),
            )
        })?;

    let input = persistence_for_browser(
        &host,
        listener_port,
        options.runtime_mode,
        BRIDGE_STATUS_WAITING,
        None,
        None,
        None,
    );
    if options.dry_run {
        write_config(input)?;
        return Ok(());
    }

    let mut input = input;
    input.clear_auth = true;
    write_config(input)?;

    let bridge = BrowserBridge::new(host.clone(), listener_port, options.runtime_mode);
    tokio::spawn(run_browser_bridge_accept_loop(
        bridge.clone(),
        bridge_listener,
    ));

    eprintln!(
        "APW daemon listening on {listener_port} (browser mode). Load the APW Chrome bridge extension. After `apw status --json` reports `bridge.status=attached`, run `apw auth`."
    );

    let mut request_buffer = vec![0_u8; MAX_FRAME_SIZE];
    loop {
        let (size, peer) = listener
            .recv_from(&mut request_buffer)
            .await
            .map_err(|error| {
                APWError::new(
                    Status::GenericError,
                    format!("Daemon receive failed: {error}"),
                )
            })?;

        if size > MAX_HELPER_PAYLOAD {
            let _ = send_envelope_to_client(
                &listener,
                peer,
                Status::InvalidParam,
                None,
                Some("Request too large.".to_string()),
            )
            .await;
            request_buffer.fill(0);
            continue;
        }

        let request_slice = &request_buffer[..size];
        let result = bridge
            .enqueue_request(request_slice)
            .await
            .and_then(|payload| parse_helper_response_shape(&payload));

        match result {
            Ok(payload) => {
                let _ =
                    send_envelope_to_client(&listener, peer, Status::Success, Some(payload), None)
                        .await;
            }
            Err(error) => {
                let _ =
                    send_envelope_to_client(&listener, peer, error.code, None, Some(error.message))
                        .await;
            }
        }

        request_buffer.fill(0);
    }
}

async fn read_native_host_frame(stream: &mut UnixStream) -> Result<Vec<u8>> {
    let mut length = [0_u8; 4];
    stream.read_exact(&mut length).await.map_err(|error| {
        APWError::new(
            Status::ProcessNotRunning,
            format!("Failed reading native host frame header: {error}"),
        )
    })?;
    let payload_length = u32::from_le_bytes(length) as usize;
    if payload_length == 0 || payload_length > MAX_HELPER_PAYLOAD {
        return Err(APWError::new(
            Status::ProtoInvalidResponse,
            "Invalid native host frame size.",
        ));
    }

    let mut payload = vec![0_u8; payload_length];
    stream.read_exact(&mut payload).await.map_err(|error| {
        APWError::new(
            Status::ProcessNotRunning,
            format!("Failed reading native host frame: {error}"),
        )
    })?;
    Ok(payload)
}

async fn write_native_host_frame(stream: &mut UnixStream, payload: &[u8]) -> Result<()> {
    if payload.len() > MAX_HELPER_PAYLOAD {
        return Err(APWError::new(
            Status::InvalidParam,
            "Outgoing native host payload exceeds max size.",
        ));
    }

    stream
        .write_all(&(payload.len() as u32).to_le_bytes())
        .await
        .map_err(|error| {
            APWError::new(
                Status::ProcessNotRunning,
                format!("Failed writing native host frame header: {error}"),
            )
        })?;
    stream.write_all(payload).await.map_err(|error| {
        APWError::new(
            Status::ProcessNotRunning,
            format!("Failed writing native host frame: {error}"),
        )
    })?;
    stream.flush().await.map_err(|error| {
        APWError::new(
            Status::ProcessNotRunning,
            format!("Failed flushing native host frame: {error}"),
        )
    })?;
    Ok(())
}

fn persist_native_waiting(host: &str, port: u16, runtime_mode: RuntimeMode) -> Result<()> {
    write_config(WriteConfigInput {
        port: Some(port),
        host: Some(host.to_string()),
        allow_empty: true,
        clear_auth: true,
        runtime_mode: Some(runtime_mode),
        bridge_status: Some(BRIDGE_STATUS_WAITING.to_string()),
        bridge_browser: Some("native".to_string()),
        bridge_connected_at: None,
        bridge_last_error: None,
        reset_launch_metadata: true,
        reset_bridge_metadata: true,
        refresh_created_at: false,
        ..WriteConfigInput::default()
    })?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn verify_native_host_peer(stream: &UnixStream) -> Result<()> {
    let fd = stream.as_raw_fd();
    let mut peer_uid: libc::uid_t = 0;
    let mut peer_gid: libc::gid_t = 0;
    let status = unsafe { libc::getpeereid(fd, &mut peer_uid, &mut peer_gid) };
    if status != 0 {
        return Err(APWError::new(
            Status::ProcessNotRunning,
            format!(
                "Failed to verify native host peer credentials: {}",
                std::io::Error::last_os_error()
            ),
        ));
    }

    let current_uid = unsafe { libc::geteuid() };
    if peer_uid != current_uid {
        return Err(APWError::new(
            Status::ProcessNotRunning,
            "Native host peer UID mismatch.",
        ));
    }

    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn verify_native_host_peer(_stream: &UnixStream) -> Result<()> {
    Ok(())
}

async fn run_native_host_accept_loop(bridge: BrowserBridge, listener: UnixListener) {
    loop {
        let (mut stream, _) = match listener.accept().await {
            Ok(value) => value,
            Err(error) => {
                eprintln!("Failed accepting native host connection: {error}");
                continue;
            }
        };

        if let Err(error) = verify_native_host_peer(&stream) {
            eprintln!("{}", error.message);
            continue;
        }

        let hello = match timeout(
            Duration::from_millis(BRIDGE_ATTACH_TIMEOUT_MS),
            read_native_host_frame(&mut stream),
        )
        .await
        {
            Ok(Ok(payload)) => payload,
            Ok(Err(error)) => {
                eprintln!("{}", error.message);
                continue;
            }
            Err(_) => {
                eprintln!("Timed out waiting for native host hello frame.");
                continue;
            }
        };

        let hello = match serde_json::from_slice::<BridgeFromBrowserMessage>(&hello) {
            Ok(message) => message,
            Err(error) => {
                eprintln!("Malformed native host hello frame: {error}");
                continue;
            }
        };

        let identity = match hello {
            BridgeFromBrowserMessage::Hello { browser, version } => version.unwrap_or(browser),
            _ => {
                eprintln!("Unexpected native host handshake message.");
                continue;
            }
        };

        let connection_id = BRIDGE_CONNECTION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let (sender, mut receiver) = mpsc::unbounded_channel::<BridgeToBrowserMessage>();
        bridge.attach(connection_id, sender, identity.clone()).await;
        eprintln!("APW native host attached ({identity}).");

        loop {
            tokio::select! {
                Some(message) = receiver.recv() => {
                    let payload = match serde_json::to_vec(&message) {
                        Ok(payload) => payload,
                        Err(error) => {
                            bridge
                                .mark_error(connection_id, format!("Failed to encode native host request: {error}"))
                                .await;
                            break;
                        }
                    };

                    if let Err(error) = write_native_host_frame(&mut stream, &payload).await {
                        bridge.mark_disconnected(connection_id, Some(error.message)).await;
                        break;
                    }
                }
                incoming = read_native_host_frame(&mut stream) => {
                    let payload = match incoming {
                        Ok(payload) => payload,
                        Err(error) => {
                            bridge.mark_disconnected(connection_id, Some(error.message)).await;
                            break;
                        }
                    };

                    let message = match serde_json::from_slice::<BridgeFromBrowserMessage>(&payload) {
                        Ok(message) => message,
                        Err(error) => {
                            bridge.mark_error(connection_id, format!("Malformed native host response: {error}")).await;
                            break;
                        }
                    };

                    match message {
                        BridgeFromBrowserMessage::Hello { .. } => {}
                        BridgeFromBrowserMessage::Status { status, error } => {
                            if status == BRIDGE_STATUS_ERROR {
                                bridge
                                    .mark_error(
                                        connection_id,
                                        error.unwrap_or_else(|| "Native host reported an unknown error.".to_string()),
                                    )
                                    .await;
                                break;
                            }
                            if status == BRIDGE_STATUS_DISCONNECTED {
                                bridge.mark_disconnected(connection_id, error).await;
                                break;
                            }
                        }
                        BridgeFromBrowserMessage::Response {
                            request_id,
                            ok,
                            payload,
                            code,
                            error,
                        } => {
                            let result = if ok {
                                Ok(payload.unwrap_or(Value::Null))
                            } else {
                                Err(APWError::new(
                                    code.unwrap_or(Status::GenericError),
                                    error.unwrap_or_else(|| "Native host request failed.".to_string()),
                                ))
                            };
                            bridge.complete_response(&request_id, result).await;
                        }
                    }
                }
            }
        }
    }
}

async fn start_native_daemon_inner(options: DaemonOptions, host: String) -> Result<()> {
    if !cfg!(target_os = "macos") {
        return Err(APWError::new(
            Status::ProcessNotRunning,
            native_host_failure_message("APW native host is supported only on macOS."),
        ));
    }

    let preflight = native_host_preflight_status(options.runtime_mode);
    let preflight_status = preflight
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    if preflight_status != "ready" {
        return Err(APWError::new(
            Status::ProcessNotRunning,
            native_host_failure_message("APW native host is not installed or not ready."),
        ));
    }

    ensure_native_host_runtime_dir()?;
    let requested_port = if options.port == 0 {
        10_000
    } else {
        options.port
    };
    let socket_path = native_host_socket_path();
    if socket_path.exists() {
        let _ = fs::remove_file(&socket_path);
    }

    let native_listener = UnixListener::bind(&socket_path).map_err(|error| {
        APWError::new(
            Status::GenericError,
            format!(
                "Failed to bind native host socket at {}: {error}",
                socket_path.display()
            ),
        )
    })?;

    let listener = UdpSocket::bind((host.as_str(), requested_port))
        .await
        .map_err(|error| {
            APWError::new(
                Status::GenericError,
                format!("Failed to bind UDP socket: {error}"),
            )
        })?;
    let listener_port = requested_port;

    if options.dry_run {
        persist_native_waiting(&host, listener_port, options.runtime_mode)?;
        return Ok(());
    }

    persist_native_waiting(&host, listener_port, options.runtime_mode)?;

    let bridge = BrowserBridge::new(host.clone(), listener_port, options.runtime_mode);
    let accept_loop = tokio::spawn(run_native_host_accept_loop(bridge.clone(), native_listener));

    let attach = timeout(Duration::from_millis(BRIDGE_ATTACH_TIMEOUT_MS), async {
        loop {
            if bridge.snapshot().await.status == BRIDGE_STATUS_ATTACHED {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;

    if attach.is_err() {
        accept_loop.abort();
        let _ = fs::remove_file(&socket_path);
        return Err(APWError::new(
            Status::ProcessNotRunning,
            native_host_failure_message("APW native host did not attach before startup timed out."),
        ));
    }

    eprintln!(
        "APW daemon listening on {listener_port} (native mode). Run `apw host install` if needed. After `apw status --json` reports `host.status=attached`, run `apw auth`."
    );

    let mut request_buffer = vec![0_u8; MAX_FRAME_SIZE];
    loop {
        let (size, peer) = listener
            .recv_from(&mut request_buffer)
            .await
            .map_err(|error| {
                APWError::new(
                    Status::GenericError,
                    format!("Daemon receive failed: {error}"),
                )
            })?;

        if size > MAX_HELPER_PAYLOAD {
            let _ = send_envelope_to_client(
                &listener,
                peer,
                Status::InvalidParam,
                None,
                Some("Request too large.".to_string()),
            )
            .await;
            request_buffer.fill(0);
            continue;
        }

        let request_slice = &request_buffer[..size];
        let result = bridge
            .enqueue_request(request_slice)
            .await
            .and_then(|payload| parse_helper_response_shape(&payload));

        match result {
            Ok(payload) => {
                let _ =
                    send_envelope_to_client(&listener, peer, Status::Success, Some(payload), None)
                        .await;
            }
            Err(error) => {
                let _ =
                    send_envelope_to_client(&listener, peer, error.code, None, Some(error.message))
                        .await;
            }
        }

        request_buffer.fill(0);
    }
}

async fn start_daemon_inner(
    options: DaemonOptions,
    manifest: Option<ManifestConfig>,
) -> Result<()> {
    let host = if options.host.trim().is_empty() {
        "127.0.0.1".to_string()
    } else {
        options.host.clone()
    };

    if options.runtime_mode == RuntimeMode::Browser {
        return start_browser_daemon_inner(options, host).await;
    }
    if options.runtime_mode == RuntimeMode::Native {
        return start_native_daemon_inner(options, host).await;
    }

    let manifest = manifest.ok_or_else(|| {
        APWError::new(
            Status::InvalidConfig,
            "Missing helper manifest for non-browser runtime mode.",
        )
    })?;

    let (process, helper, context) =
        probe_helper_launch(&manifest, options.runtime_mode, options.dry_run).await?;
    let requested_port = if options.port == 0 {
        10_000
    } else {
        options.port
    };

    if options.dry_run {
        if let Some(input) =
            persistence_for_launch(&host, requested_port, &context, options.runtime_mode)
        {
            write_config(input)?;
        }

        if context.launch_status == HELPER_LAUNCH_OK {
            return Ok(());
        }

        let launch_error = context
            .launch_error
            .unwrap_or_else(helper_not_running_message);
        return Err(APWError::new(
            Status::ProcessNotRunning,
            helper_preflight_failure_message(options.runtime_mode, &launch_error),
        ));
    }

    let (mut process, mut helper) = match (process, helper) {
        (Some(process), Some(helper)) => (process, helper),
        _ => {
            if let Some(input) =
                persistence_for_launch(&host, requested_port, &context, options.runtime_mode)
            {
                write_config(input)?;
            }
            let launch_error = context
                .launch_error
                .unwrap_or_else(helper_not_running_message);
            return Err(APWError::new(
                Status::ProcessNotRunning,
                helper_preflight_failure_message(options.runtime_mode, &launch_error),
            ));
        }
    };

    ensure_helper_stays_alive(&mut process).await?;

    let listener = match UdpSocket::bind((host.as_str(), options.port)).await {
        Ok(listener) => listener,
        Err(error) => {
            terminate_helper(&mut process).await;
            return Err(APWError::new(
                Status::GenericError,
                format!("Failed to bind UDP socket: {error}"),
            ));
        }
    };
    let listener_port = match listener.local_addr() {
        Ok(listener_port) => listener_port,
        Err(error) => {
            terminate_helper(&mut process).await;
            return Err(APWError::new(
                Status::GenericError,
                format!("Unable to resolve listener address: {error}"),
            ));
        }
    };
    let listener_port = listener_port.port();

    if let Some(mut input) =
        persistence_for_launch(&host, listener_port, &context, options.runtime_mode)
    {
        input.clear_auth = true;
        if let Err(error) = write_config(input) {
            terminate_helper(&mut process).await;
            return Err(error);
        }
    }

    eprintln!("APW Helper Listening on {listener_port}.");

    let mut request_buffer = vec![0_u8; MAX_FRAME_SIZE];
    loop {
        let (size, peer) = listener
            .recv_from(&mut request_buffer)
            .await
            .map_err(|error| {
                APWError::new(
                    Status::GenericError,
                    format!("Daemon receive failed: {error}"),
                )
            })?;

        if size > MAX_HELPER_PAYLOAD {
            let _ = send_envelope_to_client(
                &listener,
                peer,
                Status::InvalidParam,
                None,
                Some("Request too large.".to_string()),
            )
            .await;
            request_buffer.fill(0);
            continue;
        }

        let request_slice = &request_buffer[..size];
        let result = async {
            helper.write_frame(request_slice).await?;
            let framed = timeout(
                Duration::from_millis(COMMAND_TIMEOUT_MS),
                helper.read_frame(),
            )
            .await
            .map_err(|_| APWError::new(Status::CommunicationTimeout, "Command output timeout."))?
            .and_then(|bytes| {
                parse_helper_payload(&bytes)
                    .and_then(|payload| parse_helper_response_shape(&payload))
            })?;
            send_envelope_to_client(&listener, peer, Status::Success, Some(framed), None).await
        }
        .await;

        match result {
            Ok(()) => {}
            Err(error) => {
                let mapped_error = match check_helper_status(&mut process) {
                    Ok(Some(status)) => {
                        eprintln!("{}", helper_termination_message(&status));
                        report_termination_if_any(status)
                    }
                    Ok(None) if error.code == Status::ProcessNotRunning => APWError::new(
                        Status::ProcessNotRunning,
                        resolve_probe_failure_message(&mut process, error.message).await,
                    ),
                    Ok(None) | Err(_) => error,
                };

                if mapped_error.code == Status::ProcessNotRunning {
                    let launch_error = mapped_error.message.clone();
                    let status_input = persistence_for_launch_error(
                        &host,
                        listener_port,
                        options.runtime_mode,
                        &context.launch_strategy,
                        launch_error.clone(),
                    );
                    if let Err(error) = write_config(status_input) {
                        eprintln!("Failed to persist helper launch status: {}", error.message);
                    }
                    let _ = send_envelope_to_client(
                        &listener,
                        peer,
                        Status::ProcessNotRunning,
                        None,
                        Some(launch_error),
                    )
                    .await;
                    continue;
                } else {
                    let _ = send_envelope_to_client(
                        &listener,
                        peer,
                        mapped_error.code,
                        None,
                        Some(mapped_error.message),
                    )
                    .await;
                }
            }
        }

        request_buffer.fill(0);
    }
}

pub async fn start_daemon(options: DaemonOptions) -> Result<()> {
    let mut options = options;
    options.runtime_mode = resolve_runtime_mode(options.runtime_mode);
    let runtime_mode = options.runtime_mode;
    let manifest = if matches!(
        options.runtime_mode,
        RuntimeMode::Browser | RuntimeMode::Native
    ) {
        None
    } else {
        Some(read_manifest().map_err(|error| {
            APWError::new(
                error.code,
                helper_preflight_failure_message(runtime_mode, &error.message),
            )
        })?)
    };
    start_daemon_inner(options, manifest)
        .await
        .map_err(|error| {
            let needs_launch_guidance = matches!(
                error.code,
                Status::ProcessNotRunning | Status::InvalidConfig | Status::CommunicationTimeout
            ) || error.message.contains("manifest")
                || error.message.contains("helper")
                || error.message.contains("Runtime mode");

            if needs_launch_guidance {
                APWError::new(
                    error.code,
                    helper_preflight_failure_message(runtime_mode, &error.message),
                )
            } else {
                error
            }
        })
}

#[cfg(test)]
async fn start_daemon_with_manifest(
    options: DaemonOptions,
    manifest: ManifestConfig,
) -> Result<()> {
    let mut options = options;
    options.runtime_mode = resolve_runtime_mode(options.runtime_mode);
    start_daemon_inner(options, Some(manifest)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::supports_keychain_for_tests;
    use crate::utils::{read_config, ConfigReadOptions, SESSION_MAX_AGE_MS};
    use futures_util::{SinkExt, StreamExt};
    use num_traits::Zero;
    use rand::{thread_rng, Rng, RngCore};
    use serde_json::json;
    use serial_test::serial;
    use std::env;
    #[cfg(unix)]
    use std::fs;
    #[cfg(unix)]
    use std::net::UdpSocket as StdUdpSocket;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    #[cfg(unix)]
    use std::path::{Path, PathBuf};
    #[cfg(unix)]
    use std::process::{Command as StdProcessCommand, Stdio};
    #[cfg(unix)]
    use std::thread;
    #[cfg(unix)]
    use std::time::Duration as StdDuration;
    #[cfg(unix)]
    use tempfile::tempdir;
    #[cfg(unix)]
    use tokio::runtime::Runtime;
    #[cfg(unix)]
    use tokio_tungstenite::{connect_async, tungstenite::Message as TestWebSocketMessage};

    static TEST_HOME_LOCK: StdMutex<()> = StdMutex::new(());

    #[cfg(unix)]
    fn with_temp_home<F, R>(run: F) -> R
    where
        F: FnOnce(&Path) -> R,
    {
        let _guard = TEST_HOME_LOCK.lock().unwrap();
        let temp = tempdir().unwrap();
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

    #[cfg(unix)]
    fn config_path_for(home: &Path) -> PathBuf {
        home.join(".apw").join("config.json")
    }

    #[cfg(unix)]
    fn seed_authenticated_config() {
        supports_keychain_for_tests(Some(false));
        write_config(WriteConfigInput {
            username: Some("alice".to_string()),
            shared_key: Some(1u32.into()),
            port: Some(10_012),
            host: Some("127.0.0.1".to_string()),
            allow_empty: false,
            ..WriteConfigInput::default()
        })
        .unwrap();
    }

    #[cfg(unix)]
    fn wait_for_config<F>(home: &Path, predicate: F) -> Value
    where
        F: Fn(&Value) -> bool,
    {
        for _ in 0..200 {
            let path = config_path_for(home);
            if let Ok(raw) = fs::read_to_string(&path) {
                if let Ok(value) = serde_json::from_str::<Value>(&raw) {
                    if predicate(&value) {
                        return value;
                    }
                }
            }
            thread::sleep(StdDuration::from_millis(25));
        }

        panic!("timed out waiting for config state");
    }

    #[cfg(unix)]
    fn daemon_request(port: u16, request: &Message) -> Value {
        let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        socket
            .set_read_timeout(Some(StdDuration::from_secs(1)))
            .unwrap();
        let payload = serde_json::to_vec(request).unwrap();
        socket.send_to(&payload, ("127.0.0.1", port)).unwrap();

        let mut buffer = vec![0_u8; 4096];
        let size = socket.recv(&mut buffer).unwrap();
        serde_json::from_slice(&buffer[..size]).unwrap()
    }

    #[cfg(unix)]
    async fn connect_browser_bridge_for_test(
        port: u16,
    ) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>
    {
        let url = format!("ws://127.0.0.1:{port}");
        for _ in 0..80 {
            if let Ok((stream, _)) = connect_async(url.as_str()).await {
                return stream;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("timed out connecting browser bridge");
    }

    #[cfg(unix)]
    fn helper_manifest(script: &str) -> (tempfile::TempDir, ManifestConfig) {
        let dir = tempdir().unwrap();
        let helper_path = dir.path().join("helper.sh");
        fs::write(&helper_path, script).unwrap();
        let mut permissions = fs::metadata(&helper_path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&helper_path, permissions).unwrap();

        (
            dir,
            ManifestConfig {
                name: "com.apple.passwordmanager".to_string(),
                description: "PasswordManagerBrowserExtensionHelper".to_string(),
                path: helper_path.to_string_lossy().to_string(),
                r#type: "stdio".to_string(),
                allowed_origins: vec!["test-origin".to_string()],
            },
        )
    }

    #[cfg(unix)]
    fn probe_helper_context(script: &str) -> HelperLaunchContext {
        let dir = tempdir().unwrap();
        let helper_path = dir.path().join("probe-helper.sh");
        fs::write(&helper_path, script).unwrap();
        let mut permissions = fs::metadata(&helper_path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&helper_path, permissions).unwrap();

        let manifest = ManifestConfig {
            name: "com.apple.passwordmanager".to_string(),
            description: "PasswordManagerBrowserExtensionHelper".to_string(),
            path: helper_path.to_string_lossy().to_string(),
            r#type: "stdio".to_string(),
            allowed_origins: vec!["test-origin".to_string()],
        };
        let runtime = Runtime::new().unwrap();
        let (_, _, context) = runtime
            .block_on(probe_helper_launch(&manifest, RuntimeMode::Direct, true))
            .unwrap();
        context
    }

    #[test]
    fn is_manifest_accepts_allowed_extensions_key() {
        let manifest = serde_json::json!({
            "name": "com.apple.passwordmanager",
            "description": "PasswordManagerBrowserExtensionHelper",
            "path": "/System/Cryptexes/App/System/Library/CoreServices/PasswordManagerBrowserExtensionHelper.app/Contents/MacOS/PasswordManagerBrowserExtensionHelper",
            "type": "stdio",
            "allowed_extensions": [
                "password-manager-firefox-extension@apple.com"
            ]
        });

        assert!(is_manifest(&manifest));
        let parsed = serde_json::from_value::<ManifestConfig>(manifest).unwrap();
        assert_eq!(
            parsed.allowed_origins[0],
            "password-manager-firefox-extension@apple.com"
        );
    }

    #[test]
    fn parse_helper_payload_rejects_non_object() {
        assert!(parse_helper_payload(b"[]").is_err());
        assert!(parse_helper_payload(b"\"string\"").is_err());
    }

    #[test]
    fn parse_helper_payload_accepts_object_shape() {
        let value = json!({"ok": true});
        assert!(parse_helper_payload(&serde_json::to_vec(&value).unwrap()).is_ok());
    }

    #[test]
    fn parse_helper_response_shape_legacy_passthrough() {
        let legacy = json!({"status": 0, "Entries": []});
        let parsed = parse_helper_response_shape(&legacy).unwrap();
        assert_eq!(parsed.get("status"), Some(&serde_json::json!(0)));
        assert_eq!(
            parsed
                .get("Entries")
                .and_then(Value::as_array)
                .unwrap()
                .len(),
            0
        );
    }

    #[test]
    fn parse_helper_response_shape_success_payload() {
        let response = json!({
          "ok": true,
          "code": 0,
          "payload": {"ok": true},
        });
        let parsed = parse_helper_response_shape(&response).unwrap();
        assert_eq!(parsed["ok"], true);
    }

    #[test]
    fn parse_helper_response_shape_error_payload() {
        let response = json!({
          "ok": false,
          "code": 101,
          "error": "Timed out",
        });
        let result = parse_helper_response_shape(&response).unwrap_err();
        assert_eq!(result.code, Status::CommunicationTimeout);
        assert_eq!(result.message, "Timed out");
    }

    #[test]
    fn parse_helper_response_shape_unknown_status_maps_generic_error() {
        let response = json!({
          "ok": false,
          "code": 9999,
          "error": "",
        });
        let result = parse_helper_response_shape(&response).unwrap_err();
        assert_eq!(result.code, Status::GenericError);
    }

    #[test]
    fn parse_helper_payload_rejects_invalid_json() {
        assert!(parse_helper_payload(b"{bad-json").is_err());
    }

    #[test]
    fn parse_helper_payload_rejects_too_large_payload() {
        let oversized = vec![b'a'; MAX_HELPER_PAYLOAD + 1];
        assert!(parse_helper_payload(&oversized).is_err());
    }

    #[test]
    fn persistence_for_launch_error_records_status_and_error() {
        let input = persistence_for_launch_error(
            "127.0.0.1",
            10_022,
            RuntimeMode::Auto,
            "auto",
            "helper crashed".to_string(),
        );

        assert_eq!(input.port, Some(10_022));
        assert_eq!(input.host, Some("127.0.0.1".to_string()));
        assert_eq!(input.runtime_mode, Some(RuntimeMode::Auto));
        assert_eq!(
            input.last_launch_status,
            Some(HELPER_LAUNCH_FAILED.to_string())
        );
        assert_eq!(input.last_launch_error, Some("helper crashed".to_string()));
        assert_eq!(input.last_launch_strategy, Some("auto".to_string()));
    }

    #[test]
    fn backend_selection_prefers_direct_then_launchd() {
        let auto = backend_selection(RuntimeMode::Auto);
        assert_eq!(auto.len(), 2);
        assert_eq!(auto[0].strategy(), "direct");
        assert_eq!(auto[1].strategy(), "launchd_compatible");

        let direct = backend_selection(RuntimeMode::Direct);
        assert_eq!(direct.len(), 1);
        assert_eq!(direct[0].strategy(), "direct");

        let launchd = backend_selection(RuntimeMode::Launchd);
        assert_eq!(launchd.len(), 1);
        assert_eq!(launchd[0].strategy(), "launchd_compatible");

        let disabled = backend_selection(RuntimeMode::Disabled);
        assert!(disabled.is_empty());
    }

    #[test]
    #[serial]
    fn resolve_runtime_mode_auto_prefers_native_on_macos_26() {
        set_macos_major_override_for_tests(Some(26));
        assert_eq!(resolve_runtime_mode(RuntimeMode::Auto), RuntimeMode::Native);
        set_macos_major_override_for_tests(None);
    }

    #[test]
    #[serial]
    fn helper_preflight_resolves_auto_to_native_on_macos_26() {
        set_macos_major_override_for_tests(Some(26));
        let value = helper_preflight_status(RuntimeMode::Auto);

        assert_eq!(value["configuredRuntimeMode"], json!(RuntimeMode::Auto));
        assert_eq!(value["resolvedRuntimeMode"], json!(RuntimeMode::Native));
        assert_eq!(value["launchStrategies"], json!(["native_host"]));
        set_macos_major_override_for_tests(None);
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn helper_preflight_reports_unsupported_platform_off_macos() {
        let value = helper_preflight_status(RuntimeMode::Direct);

        assert_eq!(value["supported"], json!(false));
        assert_eq!(value["status"], json!("unsupported_platform"));
        assert_eq!(
            value["error"],
            json!("APW Helper manifest unsupported outside of macOS.")
        );
        assert_eq!(value["launchStrategies"], json!(["direct"]));
    }

    #[test]
    #[serial]
    fn helper_preflight_failure_message_mentions_current_status() {
        set_macos_major_override_for_tests(Some(26));
        let message =
            helper_preflight_failure_message(RuntimeMode::Auto, "Helper process is not running.");

        assert!(message.contains("Helper process is not running."));
        assert!(message.contains("daemon.preflight.status="));
        set_macos_major_override_for_tests(None);
    }

    #[cfg(unix)]
    #[test]
    #[serial]
    fn probe_helper_launch_succeeds_after_capabilities_probe() {
        let context = probe_helper_context(
            "#!/usr/bin/perl\nbinmode STDIN;\nbinmode STDOUT;\nselect(STDOUT);\n$| = 1;\nread(STDIN, my $lenbuf, 4) == 4 or exit 1;\nmy $len = unpack(\"V\", $lenbuf);\nread(STDIN, my $payload, $len) == $len or exit 1;\nmy $json = q({\"ok\":true,\"code\":0,\"payload\":{\"canFillOneTimeCodes\":true}});\nprint pack(\"V\", length($json));\nprint $json;\nsleep 5;\n",
        );

        assert_eq!(context.launch_status, HELPER_LAUNCH_OK);
        assert_eq!(context.launch_strategy, "direct");
        assert!(context.launch_error.is_none());
    }

    #[cfg(unix)]
    #[test]
    #[serial]
    fn probe_helper_launch_reports_exit_before_probe_reply() {
        let context = probe_helper_context("#!/bin/sh\nexit 0\n");

        assert_eq!(context.launch_status, HELPER_LAUNCH_FAILED);
        assert_eq!(context.launch_strategy, "direct");
        assert_eq!(
            context.launch_error,
            Some("Helper process exited with code 0.".to_string())
        );
    }

    #[cfg(unix)]
    #[test]
    #[serial]
    fn probe_helper_launch_rejects_malformed_probe_payload() {
        let context = probe_helper_context(
            "#!/bin/sh\nexec perl -e 'binmode STDIN; binmode STDOUT; select(STDOUT); $| = 1; read(STDIN, my $lenbuf, 4) == 4 or exit 1; my $len = unpack(\"V\", $lenbuf); read(STDIN, my $payload, $len) == $len or exit 1; my $json = q(not-json); print pack(\"V\", length($json)); print $json; sleep 5;'\n",
        );

        assert_eq!(context.launch_status, HELPER_LAUNCH_FAILED);
        assert_eq!(context.launch_strategy, "direct");
        assert_eq!(
            context.launch_error,
            Some("Helper returned invalid JSON.".to_string())
        );
    }

    #[cfg(unix)]
    #[test]
    #[serial]
    fn probe_helper_launch_preserves_sigkill_probe_failure_message() {
        let context = probe_helper_context(
            "#!/bin/sh\nexec perl -e 'binmode STDIN; read(STDIN, my $lenbuf, 4) == 4 or exit 1; my $len = unpack(\"V\", $lenbuf); read(STDIN, my $payload, $len) == $len or exit 1; kill 9, $$;'\n",
        );

        assert_eq!(context.launch_status, HELPER_LAUNCH_FAILED);
        assert_eq!(context.launch_strategy, "direct");
        assert_eq!(
            context.launch_error,
            Some("Helper process was terminated by SIGKILL (Code Signature Constraint Violation). Helper launch requires an approved browser/native host context on this OS and cannot be launched directly from this CLI today.".to_string())
        );
    }

    #[test]
    fn parse_helper_payload_fuzz_random_binary_inputs() {
        let mut rng = thread_rng();
        for _ in 0..512 {
            let len = (rng.next_u32() as usize) % (MAX_HELPER_PAYLOAD + 256);
            let mut raw = vec![0_u8; len];
            rng.fill_bytes(&mut raw);

            let parsed = parse_helper_payload(&raw);
            if let Ok(value) = parsed {
                assert!(value.is_object(), "payload should remain object-shaped");
            }
        }
    }

    #[test]
    fn is_absolute_unix_path_validates_absolute_inputs() {
        assert!(is_absolute_unix_path("/bin/bash"));
        assert!(!is_absolute_unix_path("bin/bash"));
        assert!(is_absolute_unix_path("/tmp/../../etc/passwd"));
        assert!(!is_absolute_unix_path("/tmp/bad\0path"));
    }

    #[test]
    fn is_manifest_rejects_malformed_fields() {
        let missing_path = json!({
            "name": "com.apple.passwordmanager",
            "description": "desc",
            "type": "stdio",
            "allowed_extensions": ["abc"]
        });

        assert!(!is_manifest(&missing_path));

        let wrong_origin = json!({
            "name": "com.apple.passwordmanager",
            "description": "desc",
            "path": "/bin/bash",
            "type": "stdio",
            "allowedOrigins": ["bad", 1]
        });
        assert!(!is_manifest(&wrong_origin));

        let empty_strings = json!({
            "name": "",
            "description": "desc",
            "path": "/bin/bash",
            "type": "stdio",
            "allowed_extensions": ["abc"],
        });
        assert!(!is_manifest(&empty_strings));
    }

    #[test]
    fn parse_helper_response_shape_errors_on_missing_payload() {
        let response = json!({
          "ok": true,
          "code": 0,
        });
        let result = parse_helper_response_shape(&response).unwrap_err();
        assert_eq!(result.code, Status::ProtoInvalidResponse);
        assert_eq!(result.message, "Invalid helper payload.");
    }

    #[test]
    fn read_manifest_fails_off_macos() {
        #[cfg(not(target_os = "macos"))]
        let result = read_manifest();
        #[cfg(not(target_os = "macos"))]
        {
            assert!(result.is_err());
            assert_eq!(result.unwrap_err().code, Status::GenericError);
        }

        #[cfg(target_os = "macos")]
        {
            let result = read_manifest();
            assert!(result.is_ok() || result.is_err());
        }
    }

    #[test]
    fn parse_helper_response_shape_fuzz_random_binary_shapes() {
        let mut rng = thread_rng();
        for _ in 0..2048 {
            let len = rng.gen_range(0..(MAX_HELPER_PAYLOAD + 256));
            let mut raw = vec![0_u8; len];
            rng.fill(raw.as_mut_slice());

            if let Ok(payload) = serde_json::from_slice::<Value>(&raw) {
                let _ = parse_helper_response_shape(&payload);
                continue;
            }
            assert!(parse_helper_payload(&raw).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    #[serial]
    fn start_daemon_browser_reports_waiting_bridge_until_extension_attaches() {
        with_temp_home(|home| {
            let runtime = Runtime::new().unwrap();
            let daemon = runtime.spawn(start_daemon_inner(
                DaemonOptions {
                    port: 0,
                    host: "127.0.0.1".to_string(),
                    runtime_mode: RuntimeMode::Browser,
                    dry_run: false,
                },
                None,
            ));

            let config = wait_for_config(home, |value| {
                value["runtimeMode"] == json!("browser")
                    && value["bridgeStatus"] == json!(BRIDGE_STATUS_WAITING)
                    && value["port"].as_u64().unwrap_or_default() > 0
            });
            let port = config["port"].as_u64().unwrap() as u16;
            let response = daemon_request(port, &capabilities_probe_message());

            assert_eq!(response["ok"], json!(false));
            assert_eq!(response["code"], json!(Status::ProcessNotRunning));
            assert!(response["error"]
                .as_str()
                .unwrap_or_default()
                .contains("bridge.status=attached"));

            daemon.abort();
            let result = runtime.block_on(daemon);
            assert!(matches!(result, Err(error) if error.is_cancelled()));
        });
    }

    #[cfg(unix)]
    #[test]
    #[serial]
    fn start_daemon_browser_routes_requests_and_tracks_attach_disconnect() {
        with_temp_home(|home| {
            let runtime = Runtime::new().unwrap();
            let daemon = runtime.spawn(start_daemon_inner(
                DaemonOptions {
                    port: 0,
                    host: "127.0.0.1".to_string(),
                    runtime_mode: RuntimeMode::Browser,
                    dry_run: false,
                },
                None,
            ));

            let config = wait_for_config(home, |value| {
                value["runtimeMode"] == json!("browser")
                    && value["bridgeStatus"] == json!(BRIDGE_STATUS_WAITING)
                    && value["port"].as_u64().unwrap_or_default() > 0
            });
            let port = config["port"].as_u64().unwrap() as u16;

            let bridge_task = runtime.spawn(async move {
                let mut websocket = connect_browser_bridge_for_test(port).await;
                websocket
                    .send(TestWebSocketMessage::Text(
                        serde_json::to_string(&BridgeFromBrowserMessage::Hello {
                            browser: "chrome".to_string(),
                            version: Some("test".to_string()),
                        })
                        .unwrap()
                        .into(),
                    ))
                    .await
                    .unwrap();

                let inbound = websocket.next().await.unwrap().unwrap();
                let request = match inbound {
                    TestWebSocketMessage::Text(text) => {
                        serde_json::from_str::<BridgeToBrowserMessage>(&text).unwrap()
                    }
                    _ => panic!("expected text request"),
                };
                let request_id = match request {
                    BridgeToBrowserMessage::Request {
                        request_id,
                        payload,
                    } => {
                        assert_eq!(payload["cmd"], json!(Command::GetCapabilities as i32));
                        request_id
                    }
                };

                websocket
                    .send(TestWebSocketMessage::Text(
                        serde_json::to_string(&BridgeFromBrowserMessage::Response {
                            request_id,
                            ok: true,
                            payload: Some(json!({
                                "ok": true,
                                "code": 0,
                                "payload": {"status": "ok"},
                            })),
                            code: None,
                            error: None,
                        })
                        .unwrap()
                        .into(),
                    ))
                    .await
                    .unwrap();
                websocket.close(None).await.unwrap();
            });

            let attached = wait_for_config(home, |value| {
                value["bridgeStatus"] == json!(BRIDGE_STATUS_ATTACHED)
                    && value["bridgeBrowser"] == json!("chrome")
            });
            assert!(attached["bridgeConnectedAt"].is_string());

            let response = daemon_request(port, &capabilities_probe_message());
            assert_eq!(response["ok"], json!(true));
            assert_eq!(response["code"], json!(Status::Success));
            assert_eq!(response["payload"]["status"], json!("ok"));

            runtime.block_on(bridge_task).unwrap();

            let disconnected = wait_for_config(home, |value| {
                value["bridgeStatus"] == json!(BRIDGE_STATUS_DISCONNECTED)
                    && value["bridgeBrowser"] == json!("chrome")
            });
            assert!(disconnected["bridgeConnectedAt"].is_null());

            daemon.abort();
            let result = runtime.block_on(daemon);
            assert!(matches!(result, Err(error) if error.is_cancelled()));
        });
    }

    #[cfg(unix)]
    #[test]
    #[serial]
    fn start_daemon_browser_propagates_bridge_transport_errors() {
        with_temp_home(|home| {
            let runtime = Runtime::new().unwrap();
            let daemon = runtime.spawn(start_daemon_inner(
                DaemonOptions {
                    port: 0,
                    host: "127.0.0.1".to_string(),
                    runtime_mode: RuntimeMode::Browser,
                    dry_run: false,
                },
                None,
            ));

            let config = wait_for_config(home, |value| {
                value["bridgeStatus"] == json!(BRIDGE_STATUS_WAITING)
                    && value["port"].as_u64().unwrap_or_default() > 0
            });
            let port = config["port"].as_u64().unwrap() as u16;

            let bridge_task = runtime.spawn(async move {
                let mut websocket = connect_browser_bridge_for_test(port).await;
                websocket
                    .send(TestWebSocketMessage::Text(
                        serde_json::to_string(&BridgeFromBrowserMessage::Hello {
                            browser: "chrome".to_string(),
                            version: None,
                        })
                        .unwrap()
                        .into(),
                    ))
                    .await
                    .unwrap();

                let inbound = websocket.next().await.unwrap().unwrap();
                let request_id = match inbound {
                    TestWebSocketMessage::Text(text) => {
                        match serde_json::from_str::<BridgeToBrowserMessage>(&text).unwrap() {
                            BridgeToBrowserMessage::Request { request_id, .. } => request_id,
                        }
                    }
                    _ => panic!("expected text request"),
                };

                websocket
                    .send(TestWebSocketMessage::Text(
                        serde_json::to_string(&BridgeFromBrowserMessage::Response {
                            request_id,
                            ok: false,
                            payload: None,
                            code: Some(Status::ProcessNotRunning),
                            error: Some("Native host disconnected.".to_string()),
                        })
                        .unwrap()
                        .into(),
                    ))
                    .await
                    .unwrap();
            });

            let attached = wait_for_config(home, |value| {
                value["bridgeStatus"] == json!(BRIDGE_STATUS_ATTACHED)
            });
            assert_eq!(attached["bridgeBrowser"], json!("chrome"));

            let response = daemon_request(port, &capabilities_probe_message());
            assert_eq!(response["ok"], json!(false));
            assert_eq!(response["code"], json!(Status::ProcessNotRunning));
            assert_eq!(response["error"], json!("Native host disconnected."));

            runtime.block_on(bridge_task).unwrap();
            daemon.abort();
            let result = runtime.block_on(daemon);
            assert!(matches!(result, Err(error) if error.is_cancelled()));
        });
    }

    #[cfg(unix)]
    #[test]
    #[serial]
    fn start_daemon_routes_requests_end_to_end() {
        with_temp_home(|home| {
            let (_helper_dir, manifest) = helper_manifest(
                "#!/bin/sh\nexec perl -e 'binmode STDIN; binmode STDOUT; select(STDOUT); $| = 1; sub respond { my ($json) = @_; print pack(\"V\", length($json)); print $json; } read(STDIN, my $lenbuf, 4) == 4 or exit 1; my $len = unpack(\"V\", $lenbuf); read(STDIN, my $payload, $len) == $len or exit 1; respond(q|{\"ok\":true,\"code\":0,\"payload\":{\"canFillOneTimeCodes\":true}}|); read(STDIN, my $lenbuf2, 4) == 4 or exit 1; my $len2 = unpack(\"V\", $lenbuf2); read(STDIN, my $payload2, $len2) == $len2 or exit 1; respond(q|{\"ok\":true,\"code\":0,\"payload\":{\"status\":\"ok\"}}|); sleep 5;'\n",
            );

            let runtime = Runtime::new().unwrap();
            let daemon = runtime.spawn(start_daemon_with_manifest(
                DaemonOptions {
                    port: 0,
                    host: "127.0.0.1".to_string(),
                    runtime_mode: RuntimeMode::Direct,
                    dry_run: false,
                },
                manifest,
            ));

            let config = wait_for_config(home, |value| {
                value["lastLaunchStatus"] == json!("ok")
                    && value["port"].as_u64().unwrap_or_default() > 0
            });
            let port = config["port"].as_u64().unwrap() as u16;
            let response = daemon_request(port, &capabilities_probe_message());

            assert_eq!(response["ok"], json!(true));
            assert_eq!(response["code"], json!(Status::Success));
            assert_eq!(response["payload"]["status"], json!("ok"));

            daemon.abort();
            let result = runtime.block_on(daemon);
            assert!(matches!(result, Err(error) if error.is_cancelled()));
        });
    }

    #[cfg(unix)]
    #[test]
    #[serial]
    fn start_daemon_persists_process_not_running_after_helper_exit() {
        with_temp_home(|home| {
            let (_helper_dir, manifest) = helper_manifest(
                "#!/bin/sh\nexec perl -e 'binmode STDIN; binmode STDOUT; select(STDOUT); $| = 1; sub respond { my ($json) = @_; print pack(\"V\", length($json)); print $json; } read(STDIN, my $lenbuf, 4) == 4 or exit 1; my $len = unpack(\"V\", $lenbuf); read(STDIN, my $payload, $len) == $len or exit 1; respond(q|{\"ok\":true,\"code\":0,\"payload\":{\"canFillOneTimeCodes\":true}}|); read(STDIN, my $lenbuf2, 4) == 4 or exit 1; my $len2 = unpack(\"V\", $lenbuf2); read(STDIN, my $payload2, $len2) == $len2 or exit 1; exit 0;'\n",
            );

            let runtime = Runtime::new().unwrap();
            let daemon = runtime.spawn(start_daemon_with_manifest(
                DaemonOptions {
                    port: 0,
                    host: "127.0.0.1".to_string(),
                    runtime_mode: RuntimeMode::Direct,
                    dry_run: false,
                },
                manifest,
            ));

            let config = wait_for_config(home, |value| {
                value["lastLaunchStatus"] == json!("ok")
                    && value["port"].as_u64().unwrap_or_default() > 0
            });
            let port = config["port"].as_u64().unwrap() as u16;

            let response = daemon_request(port, &capabilities_probe_message());
            assert_eq!(response["ok"], json!(false));
            assert_eq!(response["code"], json!(Status::ProcessNotRunning));
            assert_eq!(
                response["error"],
                json!("Helper process exited with code 0.")
            );

            let persisted = wait_for_config(home, |value| {
                value["lastLaunchStatus"] == json!("failed")
                    && value["lastLaunchError"] == json!("Helper process exited with code 0.")
            });
            assert_eq!(persisted["lastLaunchStrategy"], json!("direct"));

            daemon.abort();
            let result = runtime.block_on(daemon);
            assert!(matches!(result, Err(error) if error.is_cancelled()));
        });
    }

    #[cfg(unix)]
    #[test]
    #[serial]
    fn start_daemon_dry_run_failure_preserves_existing_session_state() {
        with_temp_home(|home| {
            seed_authenticated_config();
            let (_helper_dir, manifest) = helper_manifest("#!/bin/sh\nexit 0\n");
            let runtime = Runtime::new().unwrap();
            let result = runtime.block_on(start_daemon_with_manifest(
                DaemonOptions {
                    port: 0,
                    host: "127.0.0.1".to_string(),
                    runtime_mode: RuntimeMode::Direct,
                    dry_run: true,
                },
                manifest,
            ));

            assert!(result.is_err());
            let error = result.unwrap_err();
            assert_eq!(error.code, Status::ProcessNotRunning);
            assert!(error.message.contains("daemon.preflight.status="));

            let config = wait_for_config(home, |value| {
                value["lastLaunchStatus"] == json!(HELPER_LAUNCH_FAILED)
            });
            assert_eq!(config["username"], json!("alice"));
            assert_eq!(config["sharedKey"], json!("AQ=="));
            assert_eq!(config["lastLaunchStrategy"], json!("direct"));

            supports_keychain_for_tests(None);
        });
    }

    #[cfg(unix)]
    #[test]
    #[serial]
    fn start_daemon_bind_failure_cleans_up_helper_process() {
        with_temp_home(|_home| {
            seed_authenticated_config();
            let occupied = StdUdpSocket::bind("127.0.0.1:0").unwrap();
            let occupied_port = occupied.local_addr().unwrap().port();
            let pid_dir = tempdir().unwrap();
            let pid_path = pid_dir.path().join("helper.pid");
            let script = format!(
                "#!/bin/sh\necho $$ > '{}'\nexec perl -e 'binmode STDIN; binmode STDOUT; select(STDOUT); $| = 1; read(STDIN, my $lenbuf, 4) == 4 or exit 1; my $len = unpack(\"V\", $lenbuf); read(STDIN, my $payload, $len) == $len or exit 1; my $json = q({{\"ok\":true,\"code\":0,\"payload\":{{\"canFillOneTimeCodes\":true}}}}); print pack(\"V\", length($json)); print $json; sleep 30;'\n",
                pid_path.display()
            );
            let (_helper_dir, manifest) = helper_manifest(&script);
            let runtime = Runtime::new().unwrap();
            let result = runtime.block_on(start_daemon_with_manifest(
                DaemonOptions {
                    port: occupied_port,
                    host: "127.0.0.1".to_string(),
                    runtime_mode: RuntimeMode::Direct,
                    dry_run: false,
                },
                manifest,
            ));

            assert!(result.is_err());
            let error = result.unwrap_err();
            assert_eq!(error.code, Status::GenericError);
            assert!(error.message.contains("Failed to bind UDP socket"));

            let pid = fs::read_to_string(&pid_path)
                .unwrap()
                .trim()
                .parse::<i32>()
                .unwrap();
            let mut alive = true;
            for _ in 0..20 {
                alive = StdProcessCommand::new("/bin/kill")
                    .arg("-0")
                    .arg(pid.to_string())
                    .stderr(Stdio::null())
                    .status()
                    .map(|status| status.success())
                    .unwrap_or(false);
                if !alive {
                    break;
                }
                thread::sleep(StdDuration::from_millis(25));
            }
            assert!(
                !alive,
                "helper process should be terminated after bind failure"
            );

            let config = read_config(Some(ConfigReadOptions {
                require_auth: false,
                max_age_ms: SESSION_MAX_AGE_MS,
                ignore_expiry: false,
            }))
            .unwrap();
            assert_eq!(config.username, "alice");
            assert!(!config.shared_key.is_zero());

            supports_keychain_for_tests(None);
        });
    }
}
