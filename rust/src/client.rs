use crate::daemon::{
    helper_preflight_failure_message, helper_preflight_status, helper_preflight_status_note,
};
use crate::error::{APWError, Result};
use crate::host::{native_host_failure_message, native_host_status_note};
use crate::native_app::native_app_status;
use crate::srp::{
    base64_decode_numeric, build_client_key_exchange, build_client_verification_message,
    is_valid_pake_message, parse_pake_message_type, SRPSession, SessionValues,
};
use crate::types::{
    APWResponseEnvelope, APWRuntimeConfig, Action, Command, MSGTypes, Message, Payload,
    RuntimeMode, SecretSessionVersion, Status, BRIDGE_STATUS_ATTACHED, BRIDGE_STATUS_DISCONNECTED,
    BRIDGE_STATUS_ERROR, BRIDGE_STATUS_WAITING, DEFAULT_HOST, DEFAULT_PORT, MAX_MESSAGE_BYTES,
    PAKE_FIELD_B, PAKE_FIELD_ERR_CODE, PAKE_FIELD_HAMK, PAKE_FIELD_MSG, PAKE_FIELD_PROTO,
    PAKE_FIELD_S, PAKE_FIELD_TID, SMSG,
};
use crate::utils::{
    clear_config, read_config, to_base64, write_config, ConfigReadOptions, WriteConfigInput,
    SESSION_MAX_AGE_MS,
};
use base64::{engine::general_purpose, Engine as _};
use chrono::Utc;
use num_bigint::BigUint;
use num_traits::Zero;
use rand::RngCore;
use serde_json::{json, Value};
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::thread;
use std::time::{Duration, Instant};

const BROWSER_NAME: &str = "Chrome";
const DEFAULT_TIMEOUT_MS: u64 = 5_000;
const DEFAULT_RETRIES: u8 = 0;
const DEFAULT_RETRY_DELAY_MS: u64 = 250;
const LAUNCH_STATUS_OK: &str = "ok";
const LAUNCH_STATUS_FAILED: &str = "failed";
const LAUNCH_STATUS_DISABLED: &str = "disabled";
const LAUNCH_NOT_RUNNING_MESSAGE: &str = "Helper process is not running.";
const UNAUTHENTICATED_DAEMON_MESSAGE: &str =
    "Daemon is running but not authenticated. Run `apw auth`.";

#[derive(Clone, Copy)]
pub struct ClientSendOpts {
    pub timeout_ms: u64,
    pub retries: u8,
}

impl Default for ClientSendOpts {
    fn default() -> Self {
        Self {
            timeout_ms: DEFAULT_TIMEOUT_MS,
            retries: DEFAULT_RETRIES,
        }
    }
}

fn parse_legacy_payload(payload: &Value) -> Option<Value> {
    let status = payload.get("STATUS")?;
    if !status.is_i64() && !status.is_u64() {
        return None;
    }
    payload.get("Entries")?.as_array()?;

    Some(payload.clone())
}

fn canonical_daemon_host(host: &str) -> &str {
    match host.trim() {
        "" | "0.0.0.0" => DEFAULT_HOST,
        "::" | "[::]" => "::1",
        other => other,
    }
}

fn resolve_daemon_target(host: &str, port: u16) -> Result<SocketAddr> {
    let canonical_host = canonical_daemon_host(host);
    let addrs = (canonical_host, port).to_socket_addrs().map_err(|error| {
        APWError::new(
            Status::GenericError,
            format!("Invalid daemon address {canonical_host}:{port}: {error}"),
        )
    })?;

    let mut fallback = None;
    for addr in addrs {
        if addr.is_ipv4() {
            return Ok(addr);
        }
        if fallback.is_none() {
            fallback = Some(addr);
        }
    }

    fallback.ok_or_else(|| {
        APWError::new(
            Status::GenericError,
            format!("Invalid daemon address {canonical_host}:{port}."),
        )
    })
}

fn local_bind_addr_for_target(target: &SocketAddr) -> &'static str {
    match target {
        SocketAddr::V4(_) => "0.0.0.0:0",
        SocketAddr::V6(_) => "[::]:0",
    }
}

fn parse_response_envelope(payload: &Value) -> Result<Value> {
    if !payload.is_object() {
        return Err(APWError::new(
            Status::ProtoInvalidResponse,
            "Invalid helper response payload type.",
        ));
    }

    if payload.get("ok").is_some() {
        let object = payload.as_object().ok_or_else(|| {
            APWError::new(
                Status::ProtoInvalidResponse,
                "Invalid helper response payload.",
            )
        })?;

        let ok = object.get("ok").and_then(Value::as_bool).ok_or_else(|| {
            APWError::new(Status::ProtoInvalidResponse, "Malformed helper envelope.")
        })?;

        if ok {
            return object.get("payload").cloned().ok_or_else(|| {
                APWError::new(Status::ProtoInvalidResponse, "Missing helper payload.")
            });
        }

        let code = object
            .get("code")
            .and_then(parse_status_code)
            .unwrap_or(Status::GenericError);

        let message = object
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or(crate::types::status_text(code));

        return Err(APWError::new(code, message.to_string()));
    }

    parse_legacy_payload(payload)
        .ok_or_else(|| APWError::new(Status::ProtoInvalidResponse, "Malformed helper envelope."))
}

fn launch_error_from_config(config: &APWRuntimeConfig) -> Option<APWError> {
    if config.runtime_mode == RuntimeMode::Native {
        return native_host_error_from_config(config);
    }
    if config.runtime_mode == RuntimeMode::Browser {
        return browser_bridge_error_from_config(config);
    }

    match config.last_launch_status.as_deref() {
        None | Some(LAUNCH_STATUS_OK) => None,
        Some(LAUNCH_STATUS_DISABLED) => Some(APWError::new(
            Status::ProcessNotRunning,
            helper_preflight_failure_message(config.runtime_mode, "Helper launch is disabled."),
        )),
        Some(LAUNCH_STATUS_FAILED) | Some(_) => Some(APWError::new(
            Status::ProcessNotRunning,
            helper_preflight_failure_message(
                config.runtime_mode,
                config
                    .last_launch_error
                    .clone()
                    .unwrap_or_else(|| LAUNCH_NOT_RUNNING_MESSAGE.to_string())
                    .as_str(),
            ),
        )),
    }
}

fn native_host_remediation(config: &APWRuntimeConfig, default_status: &str) -> String {
    let host_status = config
        .bridge_status
        .as_deref()
        .unwrap_or(default_status)
        .to_string();

    match host_status.as_str() {
        BRIDGE_STATUS_DISCONNECTED => format!(
            "Daemon is running in native mode, but the APW native host disconnected. {}",
            native_host_status_note()
        ),
        BRIDGE_STATUS_ERROR => {
            if let Some(error) = config.bridge_last_error.as_deref() {
                format!(
                    "Daemon is running in native mode, but the APW native host reported an error: {error}. {}",
                    native_host_status_note()
                )
            } else {
                native_host_failure_message(
                    "Daemon is running in native mode, but the APW native host is not attached.",
                )
            }
        }
        _ => native_host_failure_message(
            "Daemon is running in native mode, but the APW native host is not attached.",
        ),
    }
}

fn native_host_error_from_config(config: &APWRuntimeConfig) -> Option<APWError> {
    match config.bridge_status.as_deref() {
        Some(BRIDGE_STATUS_ATTACHED) => None,
        Some(BRIDGE_STATUS_ERROR) => Some(APWError::new(
            Status::ProcessNotRunning,
            native_host_remediation(config, BRIDGE_STATUS_ERROR),
        )),
        Some(BRIDGE_STATUS_DISCONNECTED) => Some(APWError::new(
            Status::ProcessNotRunning,
            native_host_remediation(config, BRIDGE_STATUS_DISCONNECTED),
        )),
        Some(BRIDGE_STATUS_WAITING) | None | Some(_) => Some(APWError::new(
            Status::ProcessNotRunning,
            native_host_remediation(config, BRIDGE_STATUS_WAITING),
        )),
    }
}

fn browser_bridge_remediation(config: &APWRuntimeConfig, default_status: &str) -> String {
    let browser = config
        .bridge_browser
        .clone()
        .unwrap_or_else(|| "Chrome".to_string());
    let bridge_status = config
        .bridge_status
        .as_deref()
        .unwrap_or(default_status)
        .to_string();
    let base = format!(
        "Daemon is running in browser mode, but no {browser} bridge is attached. Load the APW Chrome bridge extension and wait for `apw status --json` to report `bridge.status=attached`."
    );

    match bridge_status.as_str() {
        BRIDGE_STATUS_DISCONNECTED => format!(
            "Daemon is running in browser mode, but the {browser} bridge disconnected. Reload the APW Chrome bridge extension and wait for `apw status --json` to report `bridge.status=attached`. {}",
            helper_preflight_status_note(config.runtime_mode)
        ),
        BRIDGE_STATUS_ERROR => {
            if let Some(error) = config.bridge_last_error.as_deref() {
                format!(
                    "Daemon is running in browser mode, but the {browser} bridge reported an error: {error}. Reload the APW Chrome bridge extension and wait for `apw status --json` to report `bridge.status=attached`. {}",
                    helper_preflight_status_note(config.runtime_mode)
                )
            } else {
                format!("{base} {}", helper_preflight_status_note(config.runtime_mode))
            }
        }
        _ => format!("{base} {}", helper_preflight_status_note(config.runtime_mode)),
    }
}

fn browser_bridge_error_from_config(config: &APWRuntimeConfig) -> Option<APWError> {
    match config.bridge_status.as_deref() {
        Some(BRIDGE_STATUS_ATTACHED) => None,
        Some(BRIDGE_STATUS_ERROR) => Some(APWError::new(
            Status::ProcessNotRunning,
            browser_bridge_remediation(config, BRIDGE_STATUS_ERROR),
        )),
        Some(BRIDGE_STATUS_DISCONNECTED) => Some(APWError::new(
            Status::ProcessNotRunning,
            browser_bridge_remediation(config, BRIDGE_STATUS_DISCONNECTED),
        )),
        Some(BRIDGE_STATUS_WAITING) | None | Some(_) => Some(APWError::new(
            Status::ProcessNotRunning,
            browser_bridge_remediation(config, BRIDGE_STATUS_WAITING),
        )),
    }
}

fn helper_launch_profile() -> Result<APWRuntimeConfig> {
    let config = read_config(Some(ConfigReadOptions {
        require_auth: false,
        max_age_ms: SESSION_MAX_AGE_MS,
        ignore_expiry: false,
    }))?;

    if let Some(error) = launch_error_from_config(&config) {
        Err(error)
    } else {
        Ok(config)
    }
}

fn parse_status_code(value: &Value) -> Option<Status> {
    value
        .as_i64()
        .or_else(|| value.as_u64().map(|candidate| candidate as i64))
        .and_then(|code| Status::try_from(code).ok())
        .or_else(|| {
            value.as_str().and_then(|text| {
                text.parse::<i64>()
                    .ok()
                    .and_then(|code| Status::try_from(code).ok())
                    .or(match text {
                        "Success" => Some(Status::Success),
                        "GenericError" => Some(Status::GenericError),
                        "InvalidParam" => Some(Status::InvalidParam),
                        "NoResults" => Some(Status::NoResults),
                        "FailedToDelete" => Some(Status::FailedToDelete),
                        "FailedToUpdate" => Some(Status::FailedToUpdate),
                        "InvalidMessageFormat" => Some(Status::InvalidMessageFormat),
                        "DuplicateItem" => Some(Status::DuplicateItem),
                        "UnknownAction" => Some(Status::UnknownAction),
                        "InvalidSession" => Some(Status::InvalidSession),
                        "ServerError" => Some(Status::ServerError),
                        "CommunicationTimeout" => Some(Status::CommunicationTimeout),
                        "InvalidConfig" => Some(Status::InvalidConfig),
                        "ProcessNotRunning" => Some(Status::ProcessNotRunning),
                        "ProtoInvalidResponse" => Some(Status::ProtoInvalidResponse),
                        _ => None,
                    })
            })
        })
}

fn parse_json_payload(value: &Value, field: &str) -> Result<String> {
    let candidate = value.get("PAKE").and_then(Value::as_str).ok_or_else(|| {
        APWError::new(
            Status::ProtoInvalidResponse,
            format!("Invalid {field} payload."),
        )
    })?;

    Ok(candidate.to_string())
}

fn parse_pake_message_code(value: &Value) -> Result<i64> {
    if value.is_null() {
        return Ok(0);
    }

    parse_pake_message_type(value).ok_or_else(|| {
        APWError::new(
            Status::ProtoInvalidResponse,
            "Malformed PAKE numeric field.",
        )
    })
}

fn parse_pake_type(value: &Value, field: &str) -> Result<i64> {
    parse_pake_message_type(value).ok_or_else(|| {
        APWError::new(
            Status::ProtoInvalidResponse,
            format!("Invalid {field} message type."),
        )
    })
}

fn parse_smsg(payload: Value) -> Result<SMSG> {
    if let Some(message) = payload.get("SMSG") {
        if message.get(PAKE_FIELD_TID).is_some() && message.get("SDATA").is_some() {
            return serde_json::from_value(message.clone())
                .map_err(|_| APWError::new(Status::ProtoInvalidResponse, "Invalid SMSG payload."));
        }
        return Err(APWError::new(
            Status::ProtoInvalidResponse,
            "Malformed SMSG field.",
        ));
    }

    serde_json::from_value(payload).map_err(|_| {
        APWError::new(
            Status::ProtoInvalidResponse,
            "Invalid helper response payload.",
        )
    })
}

pub struct ApplePasswordManager {
    pub session: SRPSession,
    remote_host: String,
    remote_port: u16,
    challenge_timestamp: Instant,
}

pub struct APWMessages;

impl APWMessages {
    #[allow(dead_code)]
    pub fn get_capabilities() -> Message {
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

    pub fn request_challenge(session: &SRPSession) -> Result<Message> {
        let payload = serde_json::to_vec(&build_client_key_exchange(session)).map_err(|error| {
            APWError::new(
                Status::ServerError,
                format!("Failed to encode challenge: {error}"),
            )
        })?;

        Ok(Message {
            cmd: Command::Handshake as i32,
            payload: None,
            msg: Some(json!({
              "QID": "m0",
              "HSTBRSR": BROWSER_NAME,
              "PAKE": to_base64(&payload),
            })),
            capabilities: Some(json!({
              "canFillOneTimeCodes": true,
            })),
            set_up_totp_page_url: None,
            set_up_totp_uri: None,
            url: None,
            tab_id: None,
            frame_id: None,
        })
    }

    pub fn get_login_names_for_url(session: &SRPSession, url: &str) -> Result<Message> {
        let encrypted = session.encrypt(&json!({
          "ACT": Action::GhostSearch,
          "URL": url,
        }))?;
        let payload = serde_json::to_string(&json!({
          "QID": "CmdGetLoginNames4URL",
          "SMSG": {
            "TID": session.username,
            "SDATA": session.serialize(&encrypted, true),
          },
        }))
        .map_err(|error| {
            APWError::new(
                Status::ServerError,
                format!("Failed to encode payload: {error}"),
            )
        })?;

        Ok(Message {
            cmd: Command::GetLoginNamesForUrl as i32,
            payload: Some(json!(payload)),
            msg: None,
            capabilities: None,
            set_up_totp_page_url: None,
            set_up_totp_uri: None,
            url: Some(url.to_string()),
            tab_id: Some(1),
            frame_id: Some(1),
        })
    }

    pub fn get_password_for_url(
        session: &SRPSession,
        url: &str,
        login_name: &str,
    ) -> Result<Message> {
        let encrypted = session.encrypt(&json!({
          "ACT": Action::Search,
          "URL": url,
          "USR": login_name,
        }))?;
        let payload = serde_json::to_string(&json!({
          "QID": "CmdGetPassword4LoginName",
          "SMSG": {
            "TID": session.username,
            "SDATA": session.serialize(&encrypted, true),
          },
        }))
        .map_err(|error| {
            APWError::new(
                Status::ServerError,
                format!("Failed to encode payload: {error}"),
            )
        })?;

        Ok(Message {
            cmd: Command::GetPasswordForLoginName as i32,
            payload: Some(json!(payload)),
            msg: None,
            capabilities: None,
            set_up_totp_page_url: None,
            set_up_totp_uri: None,
            url: Some(url.to_string()),
            tab_id: Some(0),
            frame_id: Some(0),
        })
    }

    pub fn get_otp_for_url(session: &SRPSession, url: &str) -> Result<Message> {
        let encrypted = session.encrypt(&json!({
          "ACT": Action::Search,
          "TYPE": "oneTimeCodes",
          "frameURLs": [url],
        }))?;
        let payload = serde_json::to_string(&json!({
          "QID": "CmdDidFillOneTimeCode",
          "SMSG": {
            "TID": session.username,
            "SDATA": session.serialize(&encrypted, true),
          },
        }))
        .map_err(|error| {
            APWError::new(
                Status::ServerError,
                format!("Failed to encode payload: {error}"),
            )
        })?;

        Ok(Message {
            cmd: Command::DidFillOneTimeCode as i32,
            payload: Some(json!(payload)),
            msg: None,
            capabilities: None,
            set_up_totp_page_url: None,
            set_up_totp_uri: None,
            url: Some(url.to_string()),
            tab_id: Some(0),
            frame_id: Some(0),
        })
    }

    pub fn list_otp_for_url(session: &SRPSession, url: &str) -> Result<Message> {
        let encrypted = session.encrypt(&json!({
          "ACT": Action::GhostSearch,
          "TYPE": "oneTimeCodes",
          "frameURLs": [url],
        }))?;
        let payload = serde_json::to_string(&json!({
          "QID": "CmdDidFillOneTimeCode",
          "SMSG": {
            "TID": session.username,
            "SDATA": session.serialize(&encrypted, true),
          },
        }))
        .map_err(|error| {
            APWError::new(
                Status::ServerError,
                format!("Failed to encode payload: {error}"),
            )
        })?;

        Ok(Message {
            cmd: Command::GetOneTimeCodes as i32,
            payload: Some(json!(payload)),
            msg: None,
            capabilities: None,
            set_up_totp_page_url: None,
            set_up_totp_uri: None,
            url: Some(url.to_string()),
            tab_id: Some(0),
            frame_id: Some(0),
        })
    }

    pub fn verify_challenge(session: &SRPSession, proof: &[u8]) -> Message {
        let frame = serde_json::to_vec(&build_client_verification_message(session, proof))
            .unwrap_or_default();
        Message {
            cmd: Command::Handshake as i32,
            payload: None,
            msg: Some(json!({
              "HSTBRSR": BROWSER_NAME,
              "QID": "m2",
              "PAKE": to_base64(&frame),
            })),
            capabilities: None,
            set_up_totp_page_url: None,
            set_up_totp_uri: None,
            url: None,
            tab_id: None,
            frame_id: None,
        }
    }
}

fn normalize_lookup_url(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("http://{trimmed}")
    }
}

impl ApplePasswordManager {
    pub fn new() -> Self {
        let config = read_config(Some(ConfigReadOptions {
            require_auth: false,
            max_age_ms: SESSION_MAX_AGE_MS,
            ignore_expiry: false,
        }))
        .unwrap_or_else(|_| APWRuntimeConfig {
            schema: 1,
            port: DEFAULT_PORT,
            host: DEFAULT_HOST.to_string(),
            username: String::new(),
            shared_key: BigUint::zero(),
            runtime_mode: RuntimeMode::Auto,
            last_launch_status: None,
            last_launch_error: None,
            last_launch_strategy: None,
            bridge_status: None,
            bridge_browser: None,
            bridge_connected_at: None,
            bridge_last_error: None,
            fallback_provider: None,
            fallback_provider_path: None,
            created_at: Utc::now().timestamp().to_string(),
        });

        let mut session = SRPSession::new(true);
        if !config.username.is_empty() && !config.shared_key.is_zero() {
            session.update_with_values(SessionValues {
                username: Some(config.username),
                shared_key: Some(config.shared_key),
                client_private_key: None,
                salt: None,
                server_public_key: None,
            });
        }

        Self {
            session,
            remote_host: config.host,
            remote_port: config.port,
            challenge_timestamp: Instant::now() - Duration::from_secs(10),
        }
    }

    fn send_message_once(&self, message: &Message, timeout_ms: u64) -> Result<Value> {
        let config = helper_launch_profile()?;
        let target = resolve_daemon_target(config.host.as_str(), config.port)?;
        let payload = serde_json::to_vec(message).map_err(|error| {
            APWError::new(
                Status::ServerError,
                format!("Failed to serialize message: {error}"),
            )
        })?;
        if payload.len() > MAX_MESSAGE_BYTES {
            return Err(APWError::new(
                Status::ProtoInvalidResponse,
                "Request payload too large.",
            ));
        }

        let listener = UdpSocket::bind(local_bind_addr_for_target(&target)).map_err(|error| {
            APWError::new(
                Status::GenericError,
                format!("Unable to create UDP socket: {error}"),
            )
        })?;
        listener
            .set_read_timeout(Some(Duration::from_millis(timeout_ms)))
            .map_err(|error| {
                APWError::new(
                    Status::GenericError,
                    format!("Unable to configure socket: {error}"),
                )
            })?;

        listener.send_to(&payload, target).map_err(|error| {
            APWError::new(
                Status::GenericError,
                format!("Failed to send message: {error}"),
            )
        })?;

        let mut response = vec![0_u8; MAX_MESSAGE_BYTES * 2];
        let size = listener.recv(&mut response).map_err(|error| {
            if matches!(
                error.kind(),
                std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
            ) {
                APWError::new(
                    Status::CommunicationTimeout,
                    "No response from helper process",
                )
            } else {
                APWError::new(
                    Status::GenericError,
                    format!("No response from helper process: {error}"),
                )
            }
        })?;

        if size > MAX_MESSAGE_BYTES {
            return Err(APWError::new(
                Status::ProtoInvalidResponse,
                "Response payload too large.",
            ));
        }

        let parsed: Value = serde_json::from_slice(&response[..size]).map_err(|_| {
            APWError::new(
                Status::ProtoInvalidResponse,
                "Invalid helper response JSON.",
            )
        })?;
        parse_response_envelope(&parsed)
    }

    pub fn send_message(&self, message: Message, opts: Option<ClientSendOpts>) -> Result<Value> {
        let options = opts.unwrap_or_default();
        let mut attempt: u8 = 0;

        loop {
            match self.send_message_once(&message, options.timeout_ms) {
                Ok(value) => return Ok(value),
                Err(error)
                    if error.code == Status::CommunicationTimeout && attempt < options.retries =>
                {
                    let jitter = if DEFAULT_RETRY_DELAY_MS > 0 {
                        (rand::thread_rng().next_u64()) % DEFAULT_RETRY_DELAY_MS
                    } else {
                        0
                    };
                    thread::sleep(Duration::from_millis(DEFAULT_RETRY_DELAY_MS + jitter));
                    attempt = attempt.saturating_add(1);
                    continue;
                }
                Err(error) => return Err(error),
            }
        }
    }

    pub fn ensure_authenticated(
        &mut self,
        opts: Option<ConfigReadOptions>,
    ) -> Result<APWRuntimeConfig> {
        let options = opts.unwrap_or(ConfigReadOptions {
            require_auth: true,
            max_age_ms: SESSION_MAX_AGE_MS,
            ignore_expiry: false,
        });
        let config = read_config(Some(ConfigReadOptions {
            require_auth: false,
            max_age_ms: options.max_age_ms,
            ignore_expiry: options.ignore_expiry,
        }))?;
        self.remote_host = config.host.clone();
        self.remote_port = config.port;

        if let Some(error) = launch_error_from_config(&config) {
            return Err(error);
        }

        if !config.username.is_empty() && !config.shared_key.is_zero() {
            self.session.update_with_values(SessionValues {
                username: Some(config.username.clone()),
                shared_key: Some(config.shared_key.clone()),
                client_private_key: None,
                salt: None,
                server_public_key: None,
            });
            return Ok(config);
        }

        Err(APWError::new(
            Status::InvalidSession,
            if config.last_launch_status.is_some()
                || config.bridge_status.is_some()
                || config.runtime_mode != RuntimeMode::Auto
            {
                UNAUTHENTICATED_DAEMON_MESSAGE
            } else {
                "No active session. Start the daemon with `apw start`, then run `apw auth`."
            },
        ))
    }

    pub fn request_challenge(&mut self) -> Result<()> {
        let now = Instant::now();
        if now.duration_since(self.challenge_timestamp).as_millis() < 5000
            && self.session.shared_key().is_some()
        {
            return Ok(());
        }
        self.challenge_timestamp = now;

        let request = APWMessages::request_challenge(&self.session)?;
        let payload = self.send_message(request, None)?;

        let encoded = parse_json_payload(&payload, "challenge").and_then(|value| {
            if value.is_empty() {
                Err(APWError::new(
                    Status::ServerError,
                    "Invalid challenge response.",
                ))
            } else {
                Ok(value)
            }
        })?;
        let decoded = general_purpose::STANDARD
            .decode(&encoded)
            .map_err(|_| APWError::new(Status::ServerError, "Invalid server challenge payload."))?;
        let raw_message = serde_json::from_slice(&decoded)
            .map_err(|_| APWError::new(Status::ServerError, "Invalid server challenge payload."))?;

        if !is_valid_pake_message(&raw_message) {
            return Err(APWError::new(
                Status::ServerError,
                "Invalid server challenge: malformed PAKE message",
            ));
        }

        if parse_pake_type(
            raw_message.get(PAKE_FIELD_MSG).unwrap_or(&Value::Null),
            "message",
        )? != MSGTypes::ServerKeyExchange as i64
        {
            return Err(APWError::new(
                Status::ServerError,
                "Invalid server challenge: unexpected message type",
            ));
        }

        let proto = raw_message.get(PAKE_FIELD_PROTO).unwrap_or(&Value::Null);
        if parse_pake_type(proto, "protocol")?
            != SecretSessionVersion::SrpWithRfcVerification as i64
        {
            return Err(APWError::new(
                Status::ServerError,
                "Invalid server challenge: unsupported protocol",
            ));
        }

        let err_code =
            parse_pake_message_code(raw_message.get(PAKE_FIELD_ERR_CODE).unwrap_or(&Value::Null))?;
        if err_code != 0 {
            return Err(APWError::new(
                Status::ServerError,
                format!("Invalid server challenge: error {err_code}"),
            ));
        }

        let tid = raw_message
            .get(PAKE_FIELD_TID)
            .and_then(Value::as_str)
            .unwrap_or_default();
        if tid != self.session.username {
            return Err(APWError::new(
                Status::InvalidSession,
                "Invalid server challenge: session mismatch.",
            ));
        }

        let server_public_key = raw_message
            .get(PAKE_FIELD_B)
            .and_then(Value::as_str)
            .ok_or_else(|| {
                APWError::new(
                    Status::ServerError,
                    "Invalid server challenge: missing server key",
                )
            })?;
        let salt = raw_message
            .get(PAKE_FIELD_S)
            .and_then(Value::as_str)
            .ok_or_else(|| {
                APWError::new(
                    Status::ServerError,
                    "Invalid server challenge: missing salt",
                )
            })?;

        self.session.set_server_public_key(
            base64_decode_numeric(server_public_key, true)?,
            base64_decode_numeric(salt, true)?,
        )?;

        Ok(())
    }

    pub fn verify_challenge(&mut self, pin: String) -> Result<()> {
        let shared_key = self.session.set_shared_key(&pin)?;
        let proof = self.session.compute_m()?;
        let response =
            self.send_message(APWMessages::verify_challenge(&self.session, &proof), None)?;
        let encoded = parse_json_payload(&response, "verification")?;
        let decoded = general_purpose::STANDARD.decode(&encoded).map_err(|_| {
            APWError::new(Status::ServerError, "Invalid server verification payload.")
        })?;
        let raw_message: Value = serde_json::from_slice(&decoded).map_err(|_| {
            APWError::new(Status::ServerError, "Invalid server verification payload.")
        })?;

        if !is_valid_pake_message(&raw_message) {
            return Err(APWError::new(
                Status::ServerError,
                "Invalid server verification.",
            ));
        }

        if parse_pake_type(
            raw_message.get(PAKE_FIELD_MSG).unwrap_or(&Value::Null),
            "message",
        )? != MSGTypes::ServerVerification as i64
        {
            return Err(APWError::new(
                Status::ServerError,
                "Invalid server verification type.",
            ));
        }

        let tid = raw_message
            .get(PAKE_FIELD_TID)
            .and_then(Value::as_str)
            .unwrap_or_default();
        if tid != self.session.username {
            return Err(APWError::new(
                Status::InvalidSession,
                "Invalid server response session.",
            ));
        }

        let err_code =
            parse_pake_message_code(raw_message.get(PAKE_FIELD_ERR_CODE).unwrap_or(&Value::Null))?;
        if err_code == 1 {
            return Err(APWError::new(Status::InvalidSession, "Incorrect PIN."));
        }
        if err_code != 0 {
            return Err(APWError::new(
                Status::ServerError,
                "Server verification failed.",
            ));
        }

        let hamk = raw_message
            .get(PAKE_FIELD_HAMK)
            .and_then(Value::as_str)
            .ok_or_else(|| {
                APWError::new(Status::ServerError, "Invalid verification: missing HAMK.")
            })?;
        let expected = self.session.deserialize(hamk)?;
        let computed = self.session.compute_hmac(&proof)?;

        if !self.session.verify_hamk(&expected, &computed) {
            return Err(APWError::new(
                Status::ServerError,
                "Invalid verification proof.",
            ));
        }

        write_config(WriteConfigInput {
            username: Some(self.session.username.clone()),
            shared_key: Some(shared_key),
            port: Some(self.remote_port),
            host: Some(self.remote_host.clone()),
            allow_empty: false,
            refresh_created_at: true,
            ..WriteConfigInput::default()
        })?;

        Ok(())
    }

    pub fn get_login_names_for_url(&mut self, url: &str) -> Result<Payload> {
        self.ensure_authenticated(None)?;
        let normalized = normalize_lookup_url(url);
        let msg = APWMessages::get_login_names_for_url(&self.session, &normalized)?;
        let payload = self.send_message(msg, None)?;
        let decrypted = self.decrypt_payload(payload)?;
        serde_json::from_value(decrypted).map_err(|_| {
            APWError::new(
                Status::ProtoInvalidResponse,
                "Invalid helper response payload.",
            )
        })
    }

    pub fn get_password_for_url(&mut self, url: &str, login_name: &str) -> Result<Payload> {
        self.ensure_authenticated(None)?;
        let normalized = normalize_lookup_url(url);
        let msg = APWMessages::get_password_for_url(&self.session, &normalized, login_name)?;
        let payload = self.send_message(msg, None)?;
        let decrypted = self.decrypt_payload(payload)?;
        serde_json::from_value(decrypted).map_err(|_| {
            APWError::new(
                Status::ProtoInvalidResponse,
                "Invalid helper response payload.",
            )
        })
    }

    pub fn get_otp_for_url(&mut self, url: &str) -> Result<Payload> {
        self.ensure_authenticated(None)?;
        let normalized = normalize_lookup_url(url);
        let msg = APWMessages::get_otp_for_url(&self.session, &normalized)?;
        let payload = self.send_message(msg, None)?;
        let decrypted = self.decrypt_payload(payload)?;
        serde_json::from_value(decrypted).map_err(|_| {
            APWError::new(
                Status::ProtoInvalidResponse,
                "Invalid helper response payload.",
            )
        })
    }

    pub fn list_otp_for_url(&mut self, url: &str) -> Result<Payload> {
        self.ensure_authenticated(None)?;
        let normalized = normalize_lookup_url(url);
        let msg = APWMessages::list_otp_for_url(&self.session, &normalized)?;
        let payload = self.send_message(msg, None)?;
        let decrypted = self.decrypt_payload(payload)?;
        serde_json::from_value(decrypted).map_err(|_| {
            APWError::new(
                Status::ProtoInvalidResponse,
                "Invalid helper response payload.",
            )
        })
    }

    fn decrypt_payload(&self, payload: Value) -> Result<Value> {
        let container = parse_smsg(payload)?;
        if container.TID != self.session.username {
            return Err(APWError::new(
                Status::InvalidSession,
                "Response destined to another session.",
            ));
        }

        let encrypted = self.session.deserialize(&container.SDATA)?;
        let data = self
            .session
            .decrypt(&encrypted)
            .map_err(|_| APWError::new(Status::ProtoInvalidResponse, "Invalid data payload."))?;

        serde_json::from_slice(&data)
            .map_err(|_| APWError::new(Status::ProtoInvalidResponse, "Invalid data payload."))
    }

    pub fn status(&self) -> Value {
        let config = read_config(Some(ConfigReadOptions {
            require_auth: false,
            max_age_ms: SESSION_MAX_AGE_MS,
            ignore_expiry: false,
        }))
        .unwrap_or_else(|_| APWRuntimeConfig {
            schema: 1,
            port: DEFAULT_PORT,
            host: DEFAULT_HOST.to_string(),
            username: String::new(),
            shared_key: BigUint::zero(),
            runtime_mode: RuntimeMode::Auto,
            last_launch_status: None,
            last_launch_error: None,
            last_launch_strategy: None,
            bridge_status: None,
            bridge_browser: None,
            bridge_connected_at: None,
            bridge_last_error: None,
            fallback_provider: None,
            fallback_provider_path: None,
            created_at: Utc::now().timestamp().to_string(),
        });

        let created_at = chrono::DateTime::parse_from_rfc3339(&config.created_at)
            .ok()
            .map(|value| value.with_timezone(&Utc));

        let expired = match created_at {
            Some(created) => {
                if created > Utc::now() {
                    true
                } else {
                    (Utc::now() - created).num_milliseconds() > SESSION_MAX_AGE_MS as i64
                }
            }
            None => true,
        };

        let preflight = helper_preflight_status(config.runtime_mode);
        let bundle_version = preflight
            .get("appBundle")
            .and_then(|value| value.get("version"))
            .cloned()
            .unwrap_or(Value::Null);
        let bridge_browser = if config.runtime_mode == RuntimeMode::Browser {
            config.bridge_browser.clone()
        } else {
            None
        };

        let app_status = native_app_status();

        json!({
          "releaseLine": {
            "target": "v2.0.0",
            "legacyParityRetained": true,
            "primaryContract": "credential_broker"
          },
          "app": app_status,
          "daemon": {
            "host": config.host,
            "port": config.port,
            "schema": config.schema,
            "runtimeMode": config.runtime_mode,
            "lastLaunchStatus": config.last_launch_status,
            "lastLaunchError": config.last_launch_error,
            "lastLaunchStrategy": config.last_launch_strategy,
            "preflight": preflight,
          },
          "host": {
            "status": config.bridge_status,
            "connectedAt": config.bridge_connected_at,
            "bundleVersion": bundle_version,
            "lastError": config.bridge_last_error,
          },
          "bridge": {
            "status": config.bridge_status,
            "browser": bridge_browser,
            "connectedAt": config.bridge_connected_at,
            "lastError": config.bridge_last_error,
          },
          "session": {
            "username": config.username,
            "createdAt": config.created_at,
            "expired": expired,
            "authenticated": (!config.username.is_empty() && !config.shared_key.is_zero() && !expired),
          },
        })
    }

    #[allow(dead_code)]
    pub fn status_envelope(&self) -> APWResponseEnvelope<Value> {
        APWResponseEnvelope {
            ok: true,
            code: Status::Success,
            payload: Some(self.status()),
            error: None,
            request_id: None,
        }
    }

    pub fn logout(&mut self) -> Result<()> {
        clear_config();
        self.session = SRPSession::new(true);
        self.challenge_timestamp = Instant::now() - Duration::from_secs(10);
        Ok(())
    }

    pub fn set_session_for_response(
        &mut self,
        username: String,
        client_private_key: BigUint,
        server_public_key: BigUint,
        salt: BigUint,
    ) {
        self.session.update_with_values(SessionValues {
            username: Some(username),
            shared_key: None,
            client_private_key: Some(client_private_key),
            salt: Some(salt),
            server_public_key: Some(server_public_key),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::supports_keychain_for_tests;
    use crate::types::{APWConfigV1, SecretSource};
    use crate::utils::ConfigReadOptions;
    use crate::utils::SESSION_MAX_AGE_MS;
    use rand::{thread_rng, RngCore};
    use serial_test::serial;
    use std::env;
    use std::fs;
    use std::net::UdpSocket;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;
    use tempfile::TempDir;

    static TEST_HOME_LOCK: Mutex<()> = Mutex::new(());

    fn with_temp_home<F, R>(run: F) -> R
    where
        F: FnOnce() -> R,
    {
        let _guard = TEST_HOME_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let temp = TempDir::new().unwrap();
        let previous_home = env::var("HOME").ok();

        env::set_var("HOME", temp.path());
        let output = run();

        if let Some(value) = previous_home {
            env::set_var("HOME", value);
        } else {
            env::remove_var("HOME");
        }

        output
    }

    fn config_root_path() -> std::path::PathBuf {
        std::path::Path::new(&env::var("HOME").expect("HOME should be set during test execution"))
            .join(".apw")
    }

    fn config_path() -> std::path::PathBuf {
        config_root_path().join("config.json")
    }

    fn write_failed_launch_config(last_launch_error: &str) {
        supports_keychain_for_tests(Some(false));
        write_config(WriteConfigInput {
            username: None,
            shared_key: None,
            port: Some(10_012),
            host: Some("127.0.0.1".to_string()),
            allow_empty: true,
            clear_auth: true,
            runtime_mode: Some(RuntimeMode::Auto),
            last_launch_status: Some(LAUNCH_STATUS_FAILED.to_string()),
            last_launch_error: Some(last_launch_error.to_string()),
            last_launch_strategy: Some("direct".to_string()),
            ..WriteConfigInput::default()
        })
        .unwrap();
    }

    fn spawn_fake_daemon<F>(handler: F) -> (u16, thread::JoinHandle<()>)
    where
        F: Fn(&[u8]) -> Vec<u8> + Send + 'static,
    {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = socket.local_addr().unwrap().port();

        let handle = thread::spawn(move || {
            let mut buffer = vec![0_u8; 16 * 1024];
            let (size, peer) = socket.recv_from(&mut buffer).unwrap();
            let response = handler(&buffer[..size]);
            let _ = socket.send_to(&response, peer);
        });

        (port, handle)
    }

    fn spawn_stateful_fake_daemon<F>(handler: F) -> (u16, thread::JoinHandle<()>)
    where
        F: Fn(&[u8], u32) -> Vec<u8> + Send + Sync + 'static,
    {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = socket.local_addr().unwrap().port();
        socket
            .set_read_timeout(Some(std::time::Duration::from_millis(3_000)))
            .unwrap();
        let handler = Arc::new(handler);
        let handle = thread::spawn(move || {
            let mut buffer = vec![0_u8; 16 * 1024];
            let mut step = 0_u32;
            loop {
                let (size, peer) = match socket.recv_from(&mut buffer) {
                    Ok(value) => value,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            || error.kind() == std::io::ErrorKind::TimedOut =>
                    {
                        break;
                    }
                    Err(_) => break,
                };
                let response = (handler)(&buffer[..size], step);
                let _ = socket.send_to(&response, peer);
                step = step.saturating_add(1);
            }
        });

        (port, handle)
    }

    #[test]
    #[serial]
    fn send_message_round_trips_capsule_response() {
        let (port, daemon) = spawn_fake_daemon(|_| {
            let response = APWResponseEnvelope {
                ok: true,
                code: Status::Success,
                payload: Some(serde_json::json!({"status":"ok"})),
                error: None,
                request_id: None,
            };
            serde_json::to_vec(&response).unwrap()
        });

        let result = with_temp_home(|| {
            write_config(WriteConfigInput {
                username: None,
                shared_key: None,
                port: Some(port),
                host: Some("127.0.0.1".to_string()),
                allow_empty: true,
                ..WriteConfigInput::default()
            })
            .unwrap();

            let manager = ApplePasswordManager::new();
            let payload = manager
                .send_message(
                    APWMessages::get_capabilities(),
                    Some(ClientSendOpts {
                        timeout_ms: 100,
                        retries: 0,
                    }),
                )
                .unwrap();

            payload["status"] == "ok"
        });
        assert!(result);
        daemon.join().unwrap();
    }

    #[test]
    #[serial]
    fn send_message_rejects_malformed_payload() {
        let (port, daemon) = spawn_fake_daemon(|_| b"not-json".to_vec());
        let result = with_temp_home(|| {
            write_config(WriteConfigInput {
                username: None,
                shared_key: None,
                port: Some(port),
                host: Some("127.0.0.1".to_string()),
                allow_empty: true,
                ..WriteConfigInput::default()
            })
            .unwrap();
            let manager = ApplePasswordManager::new();
            manager.send_message(
                APWMessages::get_capabilities(),
                Some(ClientSendOpts {
                    timeout_ms: 100,
                    retries: 0,
                }),
            )
        });
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.code, Status::ProtoInvalidResponse);
        daemon.join().unwrap();
    }

    #[test]
    #[serial]
    fn send_message_times_out_with_retries_and_retry_budget() {
        let running = Arc::new(AtomicBool::new(true));
        let signal = running.clone();
        let (port, daemon) = {
            let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
            let port = socket.local_addr().unwrap().port();
            let _ = socket.set_read_timeout(Some(Duration::from_millis(300)));
            let handle = thread::spawn(move || {
                let mut buffer = vec![0_u8; 16 * 1024];
                while signal.load(Ordering::Acquire) {
                    match socket.recv_from(&mut buffer) {
                        Ok(_) => {}
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => continue,
                        Err(_) => break,
                    }
                }
            });
            (port, handle)
        };

        let result = with_temp_home(|| {
            write_config(WriteConfigInput {
                username: None,
                shared_key: None,
                port: Some(port),
                host: Some("127.0.0.1".to_string()),
                allow_empty: true,
                ..WriteConfigInput::default()
            })
            .unwrap();

            let manager = ApplePasswordManager::new();
            manager.send_message(
                APWMessages::get_capabilities(),
                Some(ClientSendOpts {
                    timeout_ms: 10,
                    retries: 1,
                }),
            )
        });

        assert!(matches!(result, Err(error) if error.code == Status::CommunicationTimeout));
        running.store(false, Ordering::Release);
        daemon.join().unwrap();
    }

    #[test]
    #[serial]
    fn send_message_accepts_legacy_payload() {
        let (port, daemon) = spawn_fake_daemon(|_| {
            serde_json::to_vec(&json!({
                "STATUS": Status::Success as i64,
                "Entries": [],
            }))
            .unwrap()
        });

        let result = with_temp_home(|| {
            write_config(WriteConfigInput {
                username: None,
                shared_key: None,
                port: Some(port),
                host: Some("127.0.0.1".to_string()),
                allow_empty: true,
                ..WriteConfigInput::default()
            })
            .unwrap();

            let manager = ApplePasswordManager::new();
            manager
                .send_message(
                    APWMessages::get_capabilities(),
                    Some(ClientSendOpts {
                        timeout_ms: 100,
                        retries: 0,
                    }),
                )
                .unwrap()
        });

        assert_eq!(result["STATUS"], json!(0));
        daemon.join().unwrap();
    }

    #[test]
    fn resolve_daemon_target_prefers_ipv4_loopback_for_localhost() {
        let target = resolve_daemon_target("localhost", 10_000).unwrap();
        assert!(target.is_ipv4());
    }

    #[test]
    #[serial]
    fn send_message_accepts_legacy_non_success_payload() {
        let (port, daemon) = spawn_fake_daemon(|_| {
            serde_json::to_vec(&json!({
                "STATUS": Status::NoResults as i64,
                "Entries": [],
            }))
            .unwrap()
        });

        let result = with_temp_home(|| {
            write_config(WriteConfigInput {
                username: None,
                shared_key: None,
                port: Some(port),
                host: Some("127.0.0.1".to_string()),
                allow_empty: true,
                ..WriteConfigInput::default()
            })
            .unwrap();

            let manager = ApplePasswordManager::new();
            manager
                .send_message(
                    APWMessages::get_capabilities(),
                    Some(ClientSendOpts {
                        timeout_ms: 100,
                        retries: 0,
                    }),
                )
                .unwrap()
        });

        assert_eq!(result["STATUS"], json!(Status::NoResults as i64));
        daemon.join().unwrap();
    }

    #[test]
    #[serial]
    fn send_message_normalizes_wildcard_daemon_host() {
        let (port, daemon) = spawn_fake_daemon(|_| {
            let response = APWResponseEnvelope {
                ok: true,
                code: Status::Success,
                payload: Some(serde_json::json!({"status":"ok"})),
                error: None,
                request_id: None,
            };
            serde_json::to_vec(&response).unwrap()
        });

        let result = with_temp_home(|| {
            write_config(WriteConfigInput {
                username: None,
                shared_key: None,
                port: Some(port),
                host: Some("0.0.0.0".to_string()),
                allow_empty: true,
                ..WriteConfigInput::default()
            })
            .unwrap();

            let manager = ApplePasswordManager::new();
            manager
                .send_message(
                    APWMessages::get_capabilities(),
                    Some(ClientSendOpts {
                        timeout_ms: 100,
                        retries: 0,
                    }),
                )
                .unwrap()
        });

        assert_eq!(result["status"], json!("ok"));
        daemon.join().unwrap();
    }

    #[test]
    #[serial]
    fn send_message_maps_error_envelope() {
        let (port, daemon) = spawn_fake_daemon(|_| {
            serde_json::to_vec(&json!({
                "ok": false,
                "code": Status::CommunicationTimeout as i64,
                "error": "daemon timed out",
            }))
            .unwrap()
        });

        let result = with_temp_home(|| {
            write_config(WriteConfigInput {
                username: None,
                shared_key: None,
                port: Some(port),
                host: Some("127.0.0.1".to_string()),
                allow_empty: true,
                ..WriteConfigInput::default()
            })
            .unwrap();

            let manager = ApplePasswordManager::new();
            manager.send_message(
                APWMessages::get_capabilities(),
                Some(ClientSendOpts {
                    timeout_ms: 100,
                    retries: 0,
                }),
            )
        });

        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, Status::CommunicationTimeout);
        daemon.join().unwrap();
    }

    #[test]
    #[serial]
    fn send_message_rejects_oversized_request_payload() {
        let oversized = "x".repeat(MAX_MESSAGE_BYTES + 1);
        let result = with_temp_home(|| {
            write_config(WriteConfigInput {
                username: None,
                shared_key: None,
                port: Some(10_012),
                host: Some("127.0.0.1".to_string()),
                allow_empty: true,
                ..WriteConfigInput::default()
            })
            .unwrap();

            let manager = ApplePasswordManager::new();
            manager.send_message(
                Message {
                    cmd: Command::GetPasswordForLoginName as i32,
                    payload: Some(json!(oversized)),
                    msg: None,
                    capabilities: None,
                    set_up_totp_page_url: None,
                    set_up_totp_uri: None,
                    url: None,
                    tab_id: None,
                    frame_id: None,
                },
                Some(ClientSendOpts {
                    timeout_ms: 100,
                    retries: 0,
                }),
            )
        });

        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, Status::ProtoInvalidResponse);
    }

    #[test]
    #[serial]
    fn send_message_retries_and_then_succeeds_after_timeout() {
        let attempt_count = Arc::new(AtomicUsize::new(0));
        let attempt_for_daemon = attempt_count.clone();
        let (port, daemon) = {
            let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
            let port = socket.local_addr().unwrap().port();
            let attempts = attempt_for_daemon.clone();
            let handle = thread::spawn(move || {
                let mut buffer = vec![0_u8; 16 * 1024];
                let mut step = 0_u8;
                loop {
                    let (_size, peer) = match socket.recv_from(&mut buffer) {
                        Ok(value) => value,
                        Err(error)
                            if error.kind() == std::io::ErrorKind::WouldBlock
                                || error.kind() == std::io::ErrorKind::TimedOut =>
                        {
                            break;
                        }
                        Err(_) => break,
                    };

                    attempts.fetch_add(1, Ordering::SeqCst);
                    if step == 0 {
                        thread::sleep(Duration::from_millis(50));
                    } else {
                        let response = APWResponseEnvelope::<serde_json::Value> {
                            ok: true,
                            code: Status::Success,
                            payload: Some(json!({
                                "status": "ok",
                            })),
                            error: None,
                            request_id: None,
                        };
                        let payload = serde_json::to_vec(&response).unwrap();
                        let _ = socket.send_to(&payload, peer);
                        break;
                    }

                    step = step.saturating_add(1);
                }
            });

            (port, handle)
        };

        let result = with_temp_home(|| {
            write_config(WriteConfigInput {
                username: None,
                shared_key: None,
                port: Some(port),
                host: Some("127.0.0.1".to_string()),
                allow_empty: true,
                ..WriteConfigInput::default()
            })
            .unwrap();

            let manager = ApplePasswordManager::new();
            manager.send_message(
                APWMessages::get_capabilities(),
                Some(ClientSendOpts {
                    timeout_ms: 10,
                    retries: 1,
                }),
            )
        });

        assert!(result.is_ok());
        assert_eq!(attempt_count.load(Ordering::SeqCst), 2);
        daemon.join().unwrap();
    }

    #[test]
    #[serial]
    fn send_message_rejects_oversized_binary_response() {
        let oversized_response = vec![0_u8; 2048];
        let (port, daemon) = spawn_fake_daemon(move |_| oversized_response.clone());

        let result = with_temp_home(|| {
            write_config(WriteConfigInput {
                username: None,
                shared_key: None,
                port: Some(port),
                host: Some("127.0.0.1".to_string()),
                allow_empty: true,
                ..WriteConfigInput::default()
            })
            .unwrap();

            let manager = ApplePasswordManager::new();
            manager.send_message(
                APWMessages::get_capabilities(),
                Some(ClientSendOpts {
                    timeout_ms: 100,
                    retries: 0,
                }),
            )
        });

        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, Status::ProtoInvalidResponse);
        daemon.join().unwrap();
    }

    #[test]
    #[serial]
    fn send_message_fuzzes_malformed_binary_responses() {
        let mut seed = 0xA5A5_A5A5_0123_4567_u64;
        let mut malformed_payloads: Vec<Vec<u8>> = Vec::new();

        for _ in 0..24 {
            seed ^= seed << 7;
            seed ^= seed >> 9;
            seed ^= seed << 8;
            let len = (seed % 64) as usize + 1;
            let mut raw = vec![0_u8; len];
            for byte in &mut raw {
                seed = seed
                    .wrapping_mul(0x9E3779B97F4A7C15)
                    .wrapping_add(0xBF58476D1CE4E5B9);
                *byte = (seed >> 56) as u8;
            }
            if serde_json::from_slice::<serde_json::Value>(&raw).is_ok() {
                raw = b"{\"STATUS\":0}".to_vec();
            }
            malformed_payloads.push(raw);
        }

        let malformed_payloads = Arc::new(malformed_payloads);
        let payloads_len = malformed_payloads.len();
        let request_index = Arc::new(AtomicUsize::new(0));
        let (port, daemon) = {
            let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
            let port = socket.local_addr().unwrap().port();
            let index = request_index.clone();
            let payloads = malformed_payloads.clone();
            let handle = thread::spawn(move || {
                let mut buffer = vec![0_u8; 16 * 1024];
                while let Ok((_size, peer)) = socket.recv_from(&mut buffer) {
                    let request = index.fetch_add(1, Ordering::SeqCst);
                    if request >= payloads.len() {
                        break;
                    }

                    let _ = socket.send_to(&payloads[request], peer);
                    if request + 1 >= payloads.len() {
                        break;
                    }
                }
            });
            (port, handle)
        };

        let failures = with_temp_home(|| {
            write_config(WriteConfigInput {
                username: None,
                shared_key: None,
                port: Some(port),
                host: Some("127.0.0.1".to_string()),
                allow_empty: true,
                ..WriteConfigInput::default()
            })
            .unwrap();

            let manager = ApplePasswordManager::new();
            let mut failures = 0_usize;
            for _ in 0..payloads_len {
                let result = manager.send_message(
                    APWMessages::get_capabilities(),
                    Some(ClientSendOpts {
                        timeout_ms: 100,
                        retries: 0,
                    }),
                );
                assert!(result.is_err());
                assert_eq!(result.unwrap_err().code, Status::ProtoInvalidResponse);
                failures += 1;
            }
            failures
        });

        assert_eq!(failures, payloads_len);
        daemon.join().unwrap();
    }

    #[test]
    #[serial]
    fn send_message_rejects_failed_launch_state() {
        let result = with_temp_home(|| {
            supports_keychain_for_tests(Some(false));
            write_config(WriteConfigInput {
                username: Some("alice".to_string()),
                shared_key: Some(BigUint::from(1u32)),
                port: Some(10_012),
                host: Some("127.0.0.1".to_string()),
                allow_empty: false,
                last_launch_status: Some(LAUNCH_STATUS_FAILED.to_string()),
                last_launch_error: Some("helper test failure".to_string()),
                ..WriteConfigInput::default()
            })
            .unwrap();

            let manager = ApplePasswordManager::new();
            manager.send_message(
                APWMessages::get_capabilities(),
                Some(ClientSendOpts {
                    timeout_ms: 100,
                    retries: 0,
                }),
            )
        });

        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.code, Status::ProcessNotRunning);
        assert!(error.message.contains("helper test failure"));
        assert!(error.message.contains("daemon.preflight.status="));
        supports_keychain_for_tests(None);
    }

    #[test]
    #[serial]
    fn verify_challenge_succeeds_with_server_computed_hmac() {
        with_temp_home(|| {
            supports_keychain_for_tests(Some(false));

            let session = Arc::new(Mutex::new(SRPSession::new(true)));
            let (port, daemon) = spawn_stateful_fake_daemon({
                let session = session.clone();
                move |request, step| {
                    let parsed =
                        serde_json::from_slice::<Value>(request).unwrap_or_else(|_| json!({}));
                    let raw_payload = parsed
                        .get("msg")
                        .and_then(|msg| msg.get("PAKE"))
                        .and_then(Value::as_str);
                    let response = match (step, raw_payload) {
                        (0, Some(raw_payload)) => {
                            let raw = general_purpose::STANDARD
                                .decode(raw_payload)
                                .ok()
                                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                                .unwrap_or_else(|| json!({}));
                            let message = json!({
                                "TID": raw.get("TID").cloned().unwrap_or_else(|| json!("alice")),
                                "MSG": MSGTypes::ServerKeyExchange as i32,
                                "A": "AQ==",
                                "s": "AQ==",
                                "B": general_purpose::STANDARD.encode(vec![0xff_u8; 384]),
                                "PROTO": [SecretSessionVersion::SrpWithRfcVerification as i64],
                                "VER": "1",
                                "ErrCode": 0,
                            });
                            APWResponseEnvelope::<serde_json::Value> {
                                ok: true,
                                code: Status::Success,
                                payload: Some(json!({
                                    "PAKE": to_base64(&serde_json::to_vec(&message).unwrap()),
                                })),
                                error: None,
                                request_id: None,
                            }
                        }
                        (1, Some(raw_payload)) => {
                            let raw = general_purpose::STANDARD
                                .decode(raw_payload)
                                .ok()
                                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                                .unwrap_or_else(|| json!({}));
                            let proof = raw
                                .get("M")
                                .and_then(Value::as_str)
                                .and_then(|candidate| {
                                    session.lock().unwrap().deserialize(candidate).ok()
                                })
                                .unwrap_or_default();
                            let shared = session.lock().unwrap();
                            let hamk = shared.compute_hmac(&proof).unwrap_or_default();
                            let message = json!({
                                "TID": raw.get("TID").cloned().unwrap_or_else(|| json!("alice")),
                                "MSG": MSGTypes::ServerVerification as i32,
                                "A": "AQ==",
                                "s": "AQ==",
                                "B": general_purpose::STANDARD.encode(vec![0xff_u8; 384]),
                                "PROTO": [SecretSessionVersion::SrpWithRfcVerification as i64],
                                "HAMK": shared.serialize(&hamk, false),
                                "ErrCode": 0,
                                "VER": "1",
                            });
                            APWResponseEnvelope::<serde_json::Value> {
                                ok: true,
                                code: Status::Success,
                                payload: Some(json!({
                                    "PAKE": to_base64(&serde_json::to_vec(&message).unwrap()),
                                })),
                                error: None,
                                request_id: None,
                            }
                        }
                        _ => APWResponseEnvelope::<serde_json::Value> {
                            ok: false,
                            code: Status::ServerError,
                            payload: None,
                            error: Some("unexpected request".to_string()),
                            request_id: None,
                        },
                    };
                    serde_json::to_vec(&response).unwrap()
                }
            });

            write_config(WriteConfigInput {
                username: Some("alice".to_string()),
                shared_key: Some(1u32.into()),
                port: Some(port),
                host: Some("127.0.0.1".to_string()),
                allow_empty: false,
                ..WriteConfigInput::default()
            })
            .unwrap();
            let mut manager = ApplePasswordManager::new();
            manager.remote_port = port;

            manager.request_challenge().unwrap();
            {
                let mut shared = session.lock().unwrap();
                *shared = manager.session.clone();
                shared.set_shared_key("123456").unwrap();
            }

            let verified = manager.verify_challenge("123456".to_string());
            daemon.join().unwrap();
            supports_keychain_for_tests(None);

            assert!(verified.is_ok());
            assert!(manager.session.shared_key().is_some());
            assert!(read_config(Some(ConfigReadOptions {
                require_auth: true,
                max_age_ms: SESSION_MAX_AGE_MS,
                ignore_expiry: false,
            }))
            .is_ok());
        });
    }

    #[test]
    #[serial]
    fn verify_challenge_propagates_persistence_failures() {
        let result = with_temp_home(|| {
            supports_keychain_for_tests(Some(false));

            let session = Arc::new(Mutex::new(SRPSession::new(true)));
            let config_root = config_root_path();
            let config_file = config_path();
            let (port, daemon) = spawn_stateful_fake_daemon({
                let session = session.clone();
                let config_root = config_root.clone();
                let config_file = config_file.clone();
                move |request, step| {
                    let parsed =
                        serde_json::from_slice::<Value>(request).unwrap_or_else(|_| json!({}));
                    let raw_payload = parsed
                        .get("msg")
                        .and_then(|msg| msg.get("PAKE"))
                        .and_then(Value::as_str);
                    let response = match (step, raw_payload) {
                        (0, Some(raw_payload)) => {
                            let raw = general_purpose::STANDARD
                                .decode(raw_payload)
                                .ok()
                                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                                .unwrap_or_else(|| json!({}));
                            let message = json!({
                                "TID": raw.get("TID").cloned().unwrap_or_else(|| json!("alice")),
                                "MSG": MSGTypes::ServerKeyExchange as i32,
                                "A": "AQ==",
                                "s": "AQ==",
                                "B": general_purpose::STANDARD.encode(vec![0xff_u8; 384]),
                                "PROTO": [SecretSessionVersion::SrpWithRfcVerification as i64],
                                "VER": "1",
                                "ErrCode": 0,
                            });
                            APWResponseEnvelope::<serde_json::Value> {
                                ok: true,
                                code: Status::Success,
                                payload: Some(json!({
                                    "PAKE": to_base64(&serde_json::to_vec(&message).unwrap()),
                                })),
                                error: None,
                                request_id: None,
                            }
                        }
                        (1, Some(raw_payload)) => {
                            let raw = general_purpose::STANDARD
                                .decode(raw_payload)
                                .ok()
                                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                                .unwrap_or_else(|| json!({}));
                            let proof = raw
                                .get("M")
                                .and_then(Value::as_str)
                                .and_then(|candidate| {
                                    session.lock().unwrap().deserialize(candidate).ok()
                                })
                                .unwrap_or_default();
                            let shared = session.lock().unwrap();
                            let _ = fs::remove_file(&config_file);
                            let _ = fs::remove_dir(&config_root);
                            fs::write(&config_root, b"not-a-directory").unwrap();
                            let hamk = shared.compute_hmac(&proof).unwrap_or_default();
                            let message = json!({
                                "TID": raw.get("TID").cloned().unwrap_or_else(|| json!("alice")),
                                "MSG": MSGTypes::ServerVerification as i32,
                                "A": "AQ==",
                                "s": "AQ==",
                                "B": general_purpose::STANDARD.encode(vec![0xff_u8; 384]),
                                "PROTO": [SecretSessionVersion::SrpWithRfcVerification as i64],
                                "HAMK": shared.serialize(&hamk, false),
                                "ErrCode": 0,
                                "VER": "1",
                            });
                            APWResponseEnvelope::<serde_json::Value> {
                                ok: true,
                                code: Status::Success,
                                payload: Some(json!({
                                    "PAKE": to_base64(&serde_json::to_vec(&message).unwrap()),
                                })),
                                error: None,
                                request_id: None,
                            }
                        }
                        _ => APWResponseEnvelope::<serde_json::Value> {
                            ok: false,
                            code: Status::ServerError,
                            payload: None,
                            error: Some("unexpected request".to_string()),
                            request_id: None,
                        },
                    };
                    serde_json::to_vec(&response).unwrap()
                }
            });

            write_config(WriteConfigInput {
                username: Some("alice".to_string()),
                shared_key: Some(1u32.into()),
                port: Some(port),
                host: Some("127.0.0.1".to_string()),
                allow_empty: false,
                ..WriteConfigInput::default()
            })
            .unwrap();

            let mut manager = ApplePasswordManager::new();
            manager.remote_port = port;
            manager.request_challenge().unwrap();
            {
                let mut shared = session.lock().unwrap();
                *shared = manager.session.clone();
                shared.set_shared_key("123456").unwrap();
            }

            let verify = manager.verify_challenge("123456".to_string());

            daemon.join().unwrap();
            supports_keychain_for_tests(None);
            verify
        });

        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.code, Status::InvalidConfig);
        assert!(error.message.contains("Failed"));
    }

    #[test]
    #[serial]
    fn data_plane_workflow_with_existing_session_encrypts_and_decrypts() {
        with_temp_home(|| {
            supports_keychain_for_tests(Some(false));

            let shared = Arc::new(Mutex::new(SRPSession::new(true)));
            let (port, daemon) = spawn_stateful_fake_daemon({
                let shared = shared.clone();
                move |request, step| {
                    let parsed =
                        serde_json::from_slice::<Value>(request).unwrap_or_else(|_| json!({}));
                    let raw_payload = parsed
                        .get("msg")
                        .and_then(|msg| msg.get("PAKE"))
                        .and_then(Value::as_str);

                    if let Some(raw_payload) = raw_payload {
                        let raw = general_purpose::STANDARD
                            .decode(raw_payload)
                            .ok()
                            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                            .unwrap_or_else(|| json!({}));
                        if parse_pake_message_type(raw.get(PAKE_FIELD_MSG).unwrap_or(&Value::Null))
                            .unwrap_or(-1)
                            == MSGTypes::ClientKeyExchange as i64
                        {
                            let response = json!({
                                "TID": raw.get("TID").cloned().unwrap_or_else(|| json!("alice")),
                                "MSG": MSGTypes::ServerKeyExchange as i32,
                                "A": "AQ==",
                                "s": "AQ==",
                                "B": general_purpose::STANDARD.encode(vec![0xff_u8; 384]),
                                "PROTO": [SecretSessionVersion::SrpWithRfcVerification as i64],
                                "VER": "1",
                                "ErrCode": 0,
                            });
                            return serde_json::to_vec(&APWResponseEnvelope::<serde_json::Value> {
                                ok: true,
                                code: Status::Success,
                                payload: Some(json!({
                                    "PAKE": to_base64(&serde_json::to_vec(&response).unwrap()),
                                })),
                                error: None,
                                request_id: None,
                            })
                            .unwrap();
                        }

                        let proof = raw
                            .get("M")
                            .and_then(Value::as_str)
                            .and_then(|candidate| {
                                shared.lock().unwrap().deserialize(candidate).ok()
                            })
                            .unwrap_or_default();
                        let session = shared.lock().unwrap();
                        let hamk = session.compute_hmac(&proof).unwrap_or_default();
                        let response = json!({
                            "TID": raw.get("TID").cloned().unwrap_or_else(|| json!("alice")),
                            "MSG": MSGTypes::ServerVerification as i32,
                            "A": "AQ==",
                            "s": "AQ==",
                            "B": general_purpose::STANDARD.encode(vec![0xff_u8; 384]),
                            "PROTO": [SecretSessionVersion::SrpWithRfcVerification as i64],
                            "HAMK": session.serialize(&hamk, false),
                            "ErrCode": 0,
                            "VER": "1",
                        });
                        return serde_json::to_vec(&APWResponseEnvelope::<serde_json::Value> {
                            ok: true,
                            code: Status::Success,
                            payload: Some(json!({
                                "PAKE": to_base64(&serde_json::to_vec(&response).unwrap()),
                            })),
                            error: None,
                            request_id: None,
                        })
                        .unwrap();
                    }

                    let command = parsed.get("cmd").and_then(Value::as_i64).unwrap_or(-1);
                    if step == 2 && command == Command::GetCapabilities as i64 {
                        let payload = json!({
                          "canFillOneTimeCodes": true,
                          "scanForOTPURI": false,
                        });
                        return serde_json::to_vec(&APWResponseEnvelope::<serde_json::Value> {
                            ok: true,
                            code: Status::Success,
                            payload: Some(payload),
                            error: None,
                            request_id: None,
                        })
                        .unwrap();
                    }

                    let payload = match command {
                        c if c == Command::GetLoginNamesForUrl as i64 => {
                            json!({
                                "STATUS": Status::Success,
                                "Entries": [{
                                    "USR": "alice",
                                    "sites": ["https://example.com/"],
                                    "PWD": "password",
                                }],
                            })
                        }
                        c if c == Command::GetPasswordForLoginName as i64 => {
                            json!({
                                "STATUS": Status::Success,
                                "Entries": [{
                                    "USR": "alice",
                                    "sites": ["https://example.com/"],
                                    "PWD": "hunter2",
                                }],
                            })
                        }
                        c if c == Command::GetOneTimeCodes as i64 => {
                            json!({
                                "STATUS": Status::Success,
                                "Entries": [{
                                    "code": "111111",
                                    "username": "alice",
                                    "source": "totp",
                                    "domain": "example.com",
                                }],
                            })
                        }
                        c if c == Command::DidFillOneTimeCode as i64 => {
                            json!({
                                "STATUS": Status::Success,
                                "Entries": [{
                                    "code": "111111",
                                    "username": "alice",
                                    "source": "totp",
                                    "domain": "example.com",
                                }],
                            })
                        }
                        _ => json!({
                            "STATUS": Status::NoResults,
                            "Entries": [],
                        }),
                    };

                    let response = {
                        let session = shared.lock().unwrap();
                        match session.encrypt(&payload).map(|encoded| {
                            serde_json::json!({
                                "SMSG": {
                                    "TID": session.username.clone(),
                                    "SDATA": session.serialize(&encoded, true),
                                },
                            })
                        }) {
                            Ok(smsg) => APWResponseEnvelope::<serde_json::Value> {
                                ok: true,
                                code: Status::Success,
                                payload: Some(smsg),
                                error: None,
                                request_id: None,
                            },
                            Err(error) => APWResponseEnvelope::<serde_json::Value> {
                                ok: false,
                                code: error.code,
                                payload: None,
                                error: Some(error.message),
                                request_id: None,
                            },
                        }
                    };
                    serde_json::to_vec(&response).unwrap()
                }
            });

            write_config(WriteConfigInput {
                username: Some("alice".to_string()),
                shared_key: Some(1u32.into()),
                port: Some(port),
                host: Some("127.0.0.1".to_string()),
                allow_empty: false,
                ..WriteConfigInput::default()
            })
            .unwrap();
            let mut manager = ApplePasswordManager::new();
            manager.remote_port = port;

            manager.request_challenge().unwrap();
            {
                let mut guard = shared.lock().unwrap();
                *guard = manager.session.clone();
                guard.set_shared_key("123456").unwrap();
            }
            manager.verify_challenge("123456".to_string()).unwrap();

            let capabilities = manager
                .send_message(APWMessages::get_capabilities(), None)
                .expect("capabilities");
            assert_eq!(capabilities["canFillOneTimeCodes"], json!(true));

            let login_names = manager
                .get_login_names_for_url("https://example.com/")
                .expect("login names");
            assert_eq!(login_names.status, Status::Success);
            assert_eq!(login_names.entries[0]["USR"], json!("alice"));

            let password = manager
                .get_password_for_url("https://example.com/", "alice")
                .expect("password");
            assert_eq!(password.status, Status::Success);
            assert_eq!(password.entries[0]["PWD"], json!("hunter2"));

            let otp = manager.get_otp_for_url("example.com").expect("otp");
            assert_eq!(otp.status, Status::Success);
            assert_eq!(otp.entries[0]["code"], json!("111111"));

            supports_keychain_for_tests(None);
            daemon.join().unwrap();
        });
    }

    #[test]
    fn decrypt_payload_rejects_invalid_smsg_body() {
        let manager = ApplePasswordManager::new();
        let result = manager.decrypt_payload(json!({"SMSG": "{bad-json"}));
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, Status::ProtoInvalidResponse);
    }

    #[test]
    fn parse_response_envelope_fuzzes_random_json_inputs() {
        let mut rng = thread_rng();

        for _ in 0..256 {
            let len = (rng.next_u32() as usize) % 2048;
            let mut raw = vec![0_u8; len];
            rng.fill_bytes(&mut raw);
            let payload = match serde_json::from_slice::<Value>(&raw) {
                Ok(parsed) => parsed,
                Err(_) => json!(String::from_utf8_lossy(&raw).to_string()),
            };
            let _ = parse_response_envelope(&payload);
        }
    }

    #[test]
    fn parse_response_envelope_accepts_named_string_codes() {
        let with_timeout = parse_response_envelope(&json!({
            "ok": false,
            "code": "CommunicationTimeout",
            "error": "timed out"
        }));
        assert!(with_timeout.is_err());
        assert_eq!(with_timeout.unwrap_err().code, Status::CommunicationTimeout);

        let missing = parse_response_envelope(&json!({
            "ok": false,
            "code": "UnknownStatus",
            "error": "unknown"
        }));
        assert!(missing.is_err());
        assert_eq!(missing.unwrap_err().code, Status::GenericError);
    }

    #[test]
    fn parse_response_envelope_rejects_legacy_missing_status_fields() {
        let result = parse_response_envelope(&json!({
          "STATUS": Status::Success,
        }));
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, Status::ProtoInvalidResponse);
    }

    #[test]
    #[serial]
    fn request_challenge_rejects_unsupported_protocol() {
        with_temp_home(|| {
            supports_keychain_for_tests(Some(false));
            let (port, daemon) = spawn_fake_daemon(|request| {
                let parsed = serde_json::from_slice::<Value>(request).ok();
                let raw = parsed
                    .as_ref()
                    .and_then(|payload| payload.get("msg"))
                    .and_then(|msg| msg.get("PAKE"))
                    .and_then(Value::as_str)
                    .and_then(|candidate| general_purpose::STANDARD.decode(candidate).ok())
                    .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
                if let Some(raw) = raw {
                    let response = json!({
                        "TID": raw["TID"],
                        "MSG": MSGTypes::ServerKeyExchange as i32,
                        "A": "AQ==",
                        "s": "AQ==",
                        "B": general_purpose::STANDARD.encode(vec![0xff_u8; 384]),
                        "PROTO": [SecretSessionVersion::SrpWithOldVerification as i64],
                        "VER": "1",
                        "ErrCode": 0,
                    });
                    serde_json::to_vec(&json!({
                        "ok": true,
                        "code": Status::Success as i64,
                        "payload": {
                          "PAKE": to_base64(&serde_json::to_vec(&response).unwrap()),
                        },
                    }))
                    .unwrap()
                } else {
                    Vec::new()
                }
            });

            write_config(WriteConfigInput {
                username: Some("alice".to_string()),
                shared_key: Some(0x010203u32.into()),
                port: Some(port),
                host: Some("127.0.0.1".to_string()),
                allow_empty: false,
                ..WriteConfigInput::default()
            })
            .unwrap();

            let mut manager = ApplePasswordManager::new();
            manager.set_session_for_response(
                "alice".to_string(),
                1u32.into(),
                BigUint::from(0u32),
                BigUint::from(0u32),
            );

            let outcome = manager.request_challenge();
            daemon.join().unwrap();
            supports_keychain_for_tests(None);
            assert!(outcome.is_err());
            assert_eq!(outcome.unwrap_err().code, Status::ServerError);
        });
    }

    #[test]
    #[serial]
    fn verify_challenge_returns_invalid_session_on_pin_error() {
        with_temp_home(|| {
            supports_keychain_for_tests(Some(false));
            let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
            let port = socket.local_addr().unwrap().port();
            let daemon = thread::spawn(move || {
                let mut step = 0_u8;
                let mut buffer = vec![0_u8; 16 * 1024];
                loop {
                    let (size, peer) = socket.recv_from(&mut buffer).unwrap();
                    let parsed = serde_json::from_slice::<Value>(&buffer[..size]).ok();
                    let raw = parsed
                        .as_ref()
                        .and_then(|payload| payload.get("msg"))
                        .and_then(|msg| msg.get("PAKE"))
                        .and_then(Value::as_str)
                        .and_then(|candidate| general_purpose::STANDARD.decode(candidate).ok())
                        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                        .unwrap_or_else(|| json!({}));
                    let msg_type = raw.get("MSG").and_then(Value::as_i64);

                    let response_body =
                        if step == 0 && msg_type == Some(MSGTypes::ClientKeyExchange as i64) {
                            json!({
                                "TID": raw.get("TID").cloned().unwrap_or_else(|| json!("alice")),
                                "MSG": MSGTypes::ServerKeyExchange as i64,
                                "A": "AQ==",
                                "s": "AQ==",
                                "B": general_purpose::STANDARD.encode(vec![0xff_u8; 384]),
                                "PROTO": [SecretSessionVersion::SrpWithRfcVerification as i64],
                                "VER": "1",
                                "ErrCode": 0,
                            })
                        } else {
                            json!({
                                "TID": raw.get("TID").cloned().unwrap_or_else(|| json!("alice")),
                                "MSG": MSGTypes::ServerVerification as i64,
                                "A": "AQ==",
                                "s": "AQ==",
                                "B": general_purpose::STANDARD.encode(vec![0xff_u8; 384]),
                                "PROTO": [SecretSessionVersion::SrpWithRfcVerification as i64],
                                "HAMK": raw.get("M").cloned().unwrap_or_else(|| json!("")),
                                "ErrCode": 1,
                                "VER": "1",
                            })
                        };

                    let payload = serde_json::to_vec(&json!({
                        "ok": true,
                        "code": Status::Success as i64,
                        "payload": {
                            "PAKE": to_base64(&serde_json::to_vec(&response_body).unwrap()),
                        },
                    }))
                    .unwrap();

                    let _ = socket.send_to(&payload, peer);
                    step = step.saturating_add(1);
                    if step >= 2 {
                        break;
                    }
                }
            });

            write_config(WriteConfigInput {
                username: Some("alice".to_string()),
                shared_key: Some(0x010203u32.into()),
                port: Some(port),
                host: Some("127.0.0.1".to_string()),
                allow_empty: false,
                ..WriteConfigInput::default()
            })
            .unwrap();

            let mut manager = ApplePasswordManager::new();
            manager.request_challenge().unwrap();
            let result = manager.verify_challenge("123456".to_string());
            daemon.join().unwrap();
            supports_keychain_for_tests(None);
            assert!(result.is_err());
            assert_eq!(result.unwrap_err().code, Status::InvalidSession);
        });
    }

    #[test]
    #[serial]
    fn ensure_authenticated_loads_runtime_configuration() {
        let result = with_temp_home(|| {
            supports_keychain_for_tests(Some(false));
            let write_result = write_config(WriteConfigInput {
                username: Some("alice".to_string()),
                shared_key: Some(BigUint::from_bytes_be(&[1u8; 16])),
                port: Some(10_012),
                host: Some("127.0.0.1".to_string()),
                allow_empty: false,
                ..WriteConfigInput::default()
            });
            assert!(write_result.is_ok());
            let mut manager = ApplePasswordManager::new();
            manager.ensure_authenticated(None).map(|config| {
                (
                    manager.remote_host,
                    manager.remote_port,
                    config.username,
                    config.shared_key,
                )
            })
        });
        supports_keychain_for_tests(None);
        assert!(result.is_ok());
        let (host, port, username, shared_key) = result.unwrap();
        assert_eq!(host, "127.0.0.1");
        assert_eq!(port, 10_012);
        assert_eq!(username, "alice");
        assert_eq!(shared_key, BigUint::from_bytes_be(&[1u8; 16]));
    }

    #[test]
    #[serial]
    fn request_challenge_accepts_valid_server_handshake() {
        let (ok, salt) = with_temp_home(|| {
            supports_keychain_for_tests(Some(false));
            let (port, daemon) = spawn_fake_daemon(|_request| {
                let message = json!({
                      "TID": "alice",
                  "MSG": MSGTypes::ServerKeyExchange as i32,
                  "A": "Ag==",
                  "s": "AQ==",
                  "B": "Aw==",
                  "ErrCode": 0,
                  "PROTO": [SecretSessionVersion::SrpWithRfcVerification as i64],
                  "VER": "1",
                });
                serde_json::to_vec(&APWResponseEnvelope::<serde_json::Value> {
                    ok: true,
                    code: Status::Success,
                    payload: Some(json!({
                      "PAKE": to_base64(&serde_json::to_vec(&message).unwrap()),
                    })),
                    error: None,
                    request_id: None,
                })
                .unwrap()
            });

            let _write = write_config(WriteConfigInput {
                username: Some("alice".to_string()),
                shared_key: Some(1u32.into()),
                port: Some(port),
                host: Some("127.0.0.1".to_string()),
                allow_empty: false,
                ..WriteConfigInput::default()
            })
            .unwrap();

            let mut manager = ApplePasswordManager::new();
            let result = manager.request_challenge();
            let values = manager.session.return_values();
            let salt = values.salt;
            daemon.join().unwrap();
            supports_keychain_for_tests(None);
            (result.is_ok(), salt)
        });
        assert!(ok);
        assert_eq!(salt.unwrap_or_default(), BigUint::from(1u8));
    }

    #[test]
    #[serial]
    fn get_password_for_url_decrypts_encrypted_payload() {
        with_temp_home(|| {
            supports_keychain_for_tests(Some(false));
            let shared_key: BigUint = BigUint::from_bytes_be(&[1u8; 16]);

            let (port, daemon) = {
                let shared_key = shared_key.clone();
                spawn_fake_daemon(move |_| {
                    let shared_key = shared_key.clone();
                    let mut helper_session = SRPSession::new(true);
                    helper_session.update_with_values(SessionValues {
                        username: Some("alice".to_string()),
                        shared_key: Some(shared_key),
                        client_private_key: None,
                        salt: None,
                        server_public_key: None,
                    });
                    let encrypted = helper_session
                        .encrypt(&json!({
                          "STATUS": Status::Success,
                          "Entries": [{
                            "USR": "alice",
                            "sites": ["https://example.com/"],
                            "PWD": "secret",
                          }],
                        }))
                        .unwrap();
                    let response_payload = json!({
                        "SMSG": {
                            "TID": "alice",
                            "SDATA": helper_session.serialize(&encrypted, true),
                        },
                    });
                    serde_json::to_vec(&APWResponseEnvelope::<serde_json::Value> {
                        ok: true,
                        code: Status::Success,
                        payload: Some(response_payload),
                        error: None,
                        request_id: None,
                    })
                    .unwrap()
                })
            };

            write_config(WriteConfigInput {
                username: Some("alice".to_string()),
                shared_key: Some(shared_key.clone()),
                port: Some(port),
                host: Some("127.0.0.1".to_string()),
                allow_empty: false,
                ..WriteConfigInput::default()
            })
            .unwrap();

            let mut manager = ApplePasswordManager::new();
            let output = manager
                .get_password_for_url("example.com", "alice")
                .expect("expected response payload");
            assert_eq!(output.status, Status::Success);
            assert_eq!(output.entries[0]["USR"], serde_json::json!("alice"));
            daemon.join().unwrap();
        });
        supports_keychain_for_tests(None);
    }

    #[test]
    #[serial]
    fn ensure_authenticated_requires_session_material() {
        let result = with_temp_home(|| {
            let stale_config = APWConfigV1 {
                schema: 1,
                port: 10_000,
                host: "127.0.0.1".to_string(),
                username: "alice".to_string(),
                shared_key: String::new(),
                secret_source: Some(SecretSource::File),
                fallback_provider: None,
                fallback_provider_path: None,
                created_at: chrono::Utc::now().to_rfc3339(),
                runtime_mode: RuntimeMode::Auto,
                last_launch_status: None,
                last_launch_error: None,
                last_launch_strategy: None,
                bridge_status: None,
                bridge_browser: None,
                bridge_connected_at: None,
                bridge_last_error: None,
            };

            fs::create_dir_all(config_root_path()).unwrap();
            fs::write(config_path(), serde_json::to_string(&stale_config).unwrap()).unwrap();

            let mut manager = ApplePasswordManager::new();
            manager.ensure_authenticated(Some(ConfigReadOptions {
                require_auth: true,
                max_age_ms: SESSION_MAX_AGE_MS,
                ignore_expiry: false,
            }))
        });

        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, Status::InvalidSession);
    }

    #[test]
    #[serial]
    fn ensure_authenticated_prefers_failed_launch_state_over_missing_session() {
        let result = with_temp_home(|| {
            write_failed_launch_config("helper test failure");

            let mut manager = ApplePasswordManager::new();
            manager.ensure_authenticated(None)
        });

        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.code, Status::ProcessNotRunning);
        assert!(error.message.contains("helper test failure"));
        assert!(error.message.contains("daemon.preflight.status="));
        supports_keychain_for_tests(None);
    }

    #[test]
    #[serial]
    fn get_login_names_for_url_prefers_failed_launch_state_over_missing_session() {
        let result = with_temp_home(|| {
            write_failed_launch_config("helper test failure");

            let mut manager = ApplePasswordManager::new();
            manager.get_login_names_for_url("https://example.com/")
        });

        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.code, Status::ProcessNotRunning);
        assert!(error.message.contains("helper test failure"));
        assert!(error.message.contains("daemon.preflight.status="));
        supports_keychain_for_tests(None);
    }

    #[test]
    #[serial]
    fn list_otp_for_url_prefers_failed_launch_state_over_missing_session() {
        let result = with_temp_home(|| {
            write_failed_launch_config("helper test failure");

            let mut manager = ApplePasswordManager::new();
            manager.list_otp_for_url("https://example.com/")
        });

        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.code, Status::ProcessNotRunning);
        assert!(error.message.contains("helper test failure"));
        assert!(error.message.contains("daemon.preflight.status="));
        supports_keychain_for_tests(None);
    }

    #[test]
    fn status_reports_expired_session_state() {
        let payload = with_temp_home(|| {
            let stale = APWConfigV1 {
                schema: 1,
                port: 10_012,
                host: "127.0.0.1".to_string(),
                username: "alice".to_string(),
                shared_key: String::new(),
                secret_source: Some(SecretSource::File),
                fallback_provider: None,
                fallback_provider_path: None,
                created_at: (chrono::Utc::now() - chrono::Duration::days(45)).to_rfc3339(),
                runtime_mode: RuntimeMode::Auto,
                last_launch_status: None,
                last_launch_error: None,
                last_launch_strategy: None,
                bridge_status: None,
                bridge_browser: None,
                bridge_connected_at: None,
                bridge_last_error: None,
            };

            fs::create_dir_all(config_root_path()).unwrap();
            fs::write(config_path(), serde_json::to_string(&stale).unwrap()).unwrap();

            let manager = ApplePasswordManager::new();
            manager.status()
        });

        assert_eq!(payload["session"]["expired"], serde_json::json!(true));
        assert_eq!(
            payload["session"]["authenticated"],
            serde_json::json!(false)
        );
        assert_eq!(
            payload["daemon"]["runtimeMode"],
            serde_json::json!(RuntimeMode::Auto)
        );
        assert!(payload["bridge"]["status"].is_null());
        assert!(payload["bridge"]["browser"].is_null());
        assert!(payload["bridge"]["connectedAt"].is_null());
        assert!(payload["bridge"]["lastError"].is_null());
        assert!(payload["daemon"]["lastLaunchStatus"].is_null());
        assert!(payload["daemon"]["lastLaunchError"].is_null());
        assert!(payload["daemon"]["lastLaunchStrategy"].is_null());
        assert!(payload["daemon"]["preflight"].is_object());
        assert!(payload["daemon"]["preflight"]["status"].is_string());
        assert!(payload["daemon"]["preflight"]["launchStrategies"].is_array());
    }

    #[test]
    #[serial]
    fn status_preserves_failed_launch_metadata_after_launch_failure() {
        let payload = with_temp_home(|| {
            write_failed_launch_config("helper test failure");

            let manager = ApplePasswordManager::new();
            manager.status()
        });

        assert_eq!(
            payload["session"]["authenticated"],
            serde_json::json!(false)
        );
        assert_eq!(
            payload["daemon"]["runtimeMode"],
            serde_json::json!(RuntimeMode::Auto)
        );
        assert_eq!(
            payload["daemon"]["lastLaunchStatus"],
            serde_json::json!(LAUNCH_STATUS_FAILED)
        );
        assert_eq!(
            payload["daemon"]["lastLaunchError"],
            serde_json::json!("helper test failure")
        );
        assert_eq!(
            payload["daemon"]["lastLaunchStrategy"],
            serde_json::json!("direct")
        );
        supports_keychain_for_tests(None);
    }

    #[test]
    #[serial]
    fn browser_mode_requires_attached_bridge_before_auth_or_queries() {
        let result = with_temp_home(|| {
            supports_keychain_for_tests(Some(false));
            write_config(WriteConfigInput {
                port: Some(10_012),
                host: Some("127.0.0.1".to_string()),
                allow_empty: true,
                clear_auth: true,
                runtime_mode: Some(RuntimeMode::Browser),
                bridge_status: Some(BRIDGE_STATUS_WAITING.to_string()),
                bridge_browser: Some("chrome".to_string()),
                reset_bridge_metadata: true,
                reset_launch_metadata: true,
                ..WriteConfigInput::default()
            })
            .unwrap();

            let mut manager = ApplePasswordManager::new();
            manager.ensure_authenticated(None)
        });

        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.code, Status::ProcessNotRunning);
        assert!(error.message.contains("bridge.status=attached"));
        assert!(error.message.contains("daemon.preflight.status="));
        supports_keychain_for_tests(None);
    }

    #[test]
    #[serial]
    fn browser_mode_uses_context_specific_unauthenticated_guidance_once_attached() {
        let result = with_temp_home(|| {
            supports_keychain_for_tests(Some(false));
            write_config(WriteConfigInput {
                port: Some(10_012),
                host: Some("127.0.0.1".to_string()),
                allow_empty: true,
                clear_auth: true,
                runtime_mode: Some(RuntimeMode::Browser),
                bridge_status: Some(BRIDGE_STATUS_ATTACHED.to_string()),
                bridge_browser: Some("chrome".to_string()),
                bridge_connected_at: Some(chrono::Utc::now().to_rfc3339()),
                reset_bridge_metadata: true,
                reset_launch_metadata: true,
                ..WriteConfigInput::default()
            })
            .unwrap();

            let mut manager = ApplePasswordManager::new();
            manager.ensure_authenticated(None)
        });

        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.code, Status::InvalidSession);
        assert_eq!(error.message, UNAUTHENTICATED_DAEMON_MESSAGE);
        supports_keychain_for_tests(None);
    }

    #[test]
    #[serial]
    fn status_includes_browser_bridge_state() {
        let payload = with_temp_home(|| {
            supports_keychain_for_tests(Some(false));
            write_config(WriteConfigInput {
                port: Some(10_012),
                host: Some("127.0.0.1".to_string()),
                allow_empty: true,
                clear_auth: true,
                runtime_mode: Some(RuntimeMode::Browser),
                bridge_status: Some(BRIDGE_STATUS_ATTACHED.to_string()),
                bridge_browser: Some("chrome".to_string()),
                bridge_connected_at: Some("2026-03-08T00:00:00Z".to_string()),
                bridge_last_error: Some("stale".to_string()),
                reset_bridge_metadata: true,
                reset_launch_metadata: true,
                ..WriteConfigInput::default()
            })
            .unwrap();

            let manager = ApplePasswordManager::new();
            manager.status()
        });

        assert_eq!(
            payload["daemon"]["runtimeMode"],
            json!(RuntimeMode::Browser)
        );
        assert_eq!(payload["bridge"]["status"], json!(BRIDGE_STATUS_ATTACHED));
        assert_eq!(payload["bridge"]["browser"], json!("chrome"));
        assert_eq!(
            payload["bridge"]["connectedAt"],
            json!("2026-03-08T00:00:00Z")
        );
        assert_eq!(payload["bridge"]["lastError"], json!("stale"));
        supports_keychain_for_tests(None);
    }
}
