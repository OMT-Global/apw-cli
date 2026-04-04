#![allow(dead_code)]
#![allow(non_snake_case)]
#![allow(clippy::upper_case_acronyms)]

use chrono::Utc;
use num_bigint::BigUint;
use serde::{
    de::{Deserializer, Error as DeError, Visitor},
    Deserialize, Serialize, Serializer,
};
use serde_json::Value;
use std::fmt;

pub const DEFAULT_HOST: &str = "127.0.0.1";
pub const DEFAULT_PORT: u16 = 10_000;
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Maximum payload size shared by the daemon frame reader and the native-app broker.
/// Both sides must agree on this limit; changing it requires updating both channels.
pub const MAX_MESSAGE_BYTES: usize = 32 * 1024;
/// Shared bridge status strings used by both the client and daemon.
pub const BRIDGE_STATUS_WAITING: &str = "waiting";
pub const BRIDGE_STATUS_ATTACHED: &str = "attached";
pub const BRIDGE_STATUS_DISCONNECTED: &str = "disconnected";
pub const BRIDGE_STATUS_ERROR: &str = "error";
const CONFIG_SCHEMA: i32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenamedPasswordEntry {
    pub username: String,
    pub domain: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TOTPEntry {
    pub code: String,
    pub username: String,
    pub source: String,
    pub domain: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PasswordEntry {
    #[serde(rename = "USR")]
    pub usr: String,
    #[serde(default)]
    pub sites: Vec<String>,
    #[serde(rename = "PWD")]
    pub pwd: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Payload {
    #[serde(rename = "STATUS")]
    pub status: Status,
    #[serde(rename = "Entries")]
    pub entries: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capabilities {
    #[serde(rename = "canFillOneTimeCodes")]
    pub can_fill_one_time_codes: Option<bool>,
    #[serde(rename = "scanForOTPURI")]
    pub scan_for_otp_uri: Option<bool>,
    #[serde(rename = "shouldUseBase64")]
    pub should_use_base64: Option<bool>,
    #[serde(rename = "operatingSystem")]
    pub operating_system: Option<OperatingSystem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatingSystem {
    pub name: String,
    #[serde(rename = "majorVersion")]
    pub major_version: i64,
    #[serde(rename = "minorVersion")]
    pub minor_version: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestConfig {
    pub name: String,
    pub description: String,
    pub path: String,
    #[serde(rename = "type")]
    pub r#type: String,
    #[serde(rename = "allowedOrigins", alias = "allowed_extensions", default)]
    pub allowed_origins: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct APWConfig {
    pub port: Option<u16>,
    #[serde(rename = "sharedKey")]
    pub shared_key: Option<String>,
    pub username: Option<String>,
    pub host: Option<String>,
    #[serde(rename = "createdAt")]
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct APWConfigV1 {
    pub schema: i32,
    pub port: u16,
    pub host: String,
    pub username: String,
    #[serde(rename = "sharedKey", default)]
    pub shared_key: String,
    #[serde(rename = "runtimeMode", default)]
    pub runtime_mode: RuntimeMode,
    #[serde(
        rename = "lastLaunchStatus",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub last_launch_status: Option<String>,
    #[serde(
        rename = "lastLaunchError",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub last_launch_error: Option<String>,
    #[serde(
        rename = "lastLaunchStrategy",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub last_launch_strategy: Option<String>,
    #[serde(
        rename = "bridgeStatus",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub bridge_status: Option<String>,
    #[serde(
        rename = "bridgeBrowser",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub bridge_browser: Option<String>,
    #[serde(
        rename = "bridgeConnectedAt",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub bridge_connected_at: Option<String>,
    #[serde(
        rename = "bridgeLastError",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub bridge_last_error: Option<String>,
    #[serde(rename = "secretSource", default)]
    pub secret_source: Option<SecretSource>,
    #[serde(rename = "createdAt")]
    pub created_at: String,
}

impl Default for APWConfigV1 {
    fn default() -> Self {
        Self {
            schema: CONFIG_SCHEMA,
            port: DEFAULT_PORT,
            host: DEFAULT_HOST.to_string(),
            username: String::new(),
            shared_key: String::new(),
            runtime_mode: RuntimeMode::Auto,
            last_launch_status: None,
            last_launch_error: None,
            last_launch_strategy: None,
            bridge_status: None,
            bridge_browser: None,
            bridge_connected_at: None,
            bridge_last_error: None,
            secret_source: Some(SecretSource::File),
            created_at: Utc::now().to_rfc3339(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct APWRuntimeConfig {
    pub schema: i32,
    pub port: u16,
    pub host: String,
    pub username: String,
    #[serde(rename = "sharedKey")]
    pub shared_key: BigUint,
    #[serde(rename = "runtimeMode", default)]
    pub runtime_mode: RuntimeMode,
    #[serde(
        rename = "lastLaunchStatus",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub last_launch_status: Option<String>,
    #[serde(
        rename = "lastLaunchError",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub last_launch_error: Option<String>,
    #[serde(
        rename = "lastLaunchStrategy",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub last_launch_strategy: Option<String>,
    #[serde(
        rename = "bridgeStatus",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub bridge_status: Option<String>,
    #[serde(
        rename = "bridgeBrowser",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub bridge_browser: Option<String>,
    #[serde(
        rename = "bridgeConnectedAt",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub bridge_connected_at: Option<String>,
    #[serde(
        rename = "bridgeLastError",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub bridge_last_error: Option<String>,
    #[serde(rename = "createdAt")]
    pub created_at: String,
}

impl Default for APWRuntimeConfig {
    fn default() -> Self {
        Self {
            schema: CONFIG_SCHEMA,
            port: DEFAULT_PORT,
            host: DEFAULT_HOST.to_string(),
            username: String::new(),
            shared_key: BigUint::from(0_u8),
            runtime_mode: RuntimeMode::Auto,
            last_launch_status: None,
            last_launch_error: None,
            last_launch_strategy: None,
            bridge_status: None,
            bridge_browser: None,
            bridge_connected_at: None,
            bridge_last_error: None,
            created_at: Utc::now().to_rfc3339(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RuntimeMode {
    #[default]
    Auto,
    Native,
    Browser,
    Direct,
    Launchd,
    Disabled,
}

impl RuntimeMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Native => "native",
            Self::Browser => "browser",
            Self::Direct => "direct",
            Self::Launchd => "launchd",
            Self::Disabled => "disabled",
        }
    }
}

impl Serialize for RuntimeMode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for RuntimeMode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Ok(match value.to_lowercase().as_str() {
            "native" => Self::Native,
            "browser" => Self::Browser,
            "direct" => Self::Direct,
            "launchd" => Self::Launchd,
            "disabled" => Self::Disabled,
            "auto" => Self::Auto,
            _ => Self::Auto,
        })
    }
}

impl fmt::Display for RuntimeMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SecretSource {
    File,
    Keychain,
}

pub type MessagePayload = Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct APWResponseEnvelope<T = Value> {
    pub ok: bool,
    pub code: Status,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(rename = "requestId", skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestResult<T> {
    Ok { data: T },
    Err { error: APWErrorShape },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct APWErrorShape {
    pub code: Status,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PAKEMessage {
    pub TID: String,
    pub MSG: Value,
    pub A: String,
    pub s: String,
    pub B: String,
    pub VER: Option<Value>,
    pub PROTO: Value,
    pub HAMK: Option<String>,
    #[serde(rename = "ErrCode")]
    pub ErrCode: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SMSG {
    pub TID: String,
    pub SDATA: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SRPHandshakeMessage {
    pub QID: String,
    #[serde(rename = "HSTBRSR")]
    pub HSTBRSR: String,
    pub PAKE: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub cmd: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub msg: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Value>,
    #[serde(rename = "setUpTOTPPageURL", skip_serializing_if = "Option::is_none")]
    pub set_up_totp_page_url: Option<String>,
    #[serde(rename = "setUpTOTPURI", skip_serializing_if = "Option::is_none")]
    pub set_up_totp_uri: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(rename = "tabId", skip_serializing_if = "Option::is_none")]
    pub tab_id: Option<i32>,
    #[serde(rename = "frameId", skip_serializing_if = "Option::is_none")]
    pub frame_id: Option<i32>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[repr(i32)]
pub enum Command {
    End = 0,
    Unused = 1,
    Handshake = 2,
    SetIconAndTitle = 3,
    GetLoginNamesForUrl = 4,
    GetPasswordForLoginName = 5,
    SetPasswordForLoginNameAndURL = 6,
    NewAccountForURL = 7,
    TabEvent = 8,
    PasswordsDisabled = 9,
    ReLoginNeeded = 10,
    LaunchICloudPasswords = 11,
    ICloudPasswordsStateChange = 12,
    LaunchPasswordsApp = 13,
    GetCapabilities = 14,
    OneTimeCodeAvailable = 15,
    GetOneTimeCodes = 16,
    DidFillOneTimeCode = 17,
    OpenUrlInSafari = 1984,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum SecretSessionVersion {
    SrpWithOldVerification = 0,
    SrpWithRfcVerification = 1,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[repr(i32)]
pub enum MSGTypes {
    ClientKeyExchange = 0,
    ServerKeyExchange = 1,
    ClientVerification = 2,
    ServerVerification = 3,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[repr(i32)]
pub enum Action {
    Unknown = -1,
    Delete = 0,
    Update = 1,
    Search = 2,
    AddNew = 3,
    MaybeAdd = 4,
    GhostSearch = 5,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum Status {
    Success = 0,
    GenericError = 1,
    InvalidParam = 2,
    NoResults = 3,
    FailedToDelete = 4,
    FailedToUpdate = 5,
    InvalidMessageFormat = 6,
    DuplicateItem = 7,
    UnknownAction = 8,
    InvalidSession = 9,
    ServerError = 100,
    CommunicationTimeout = 101,
    InvalidConfig = 102,
    ProcessNotRunning = 103,
    ProtoInvalidResponse = 104,
}

impl Serialize for Status {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_i32((*self).into())
    }
}

impl<'de> Deserialize<'de> for Status {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct StatusVisitor;

        impl Visitor<'_> for StatusVisitor {
            type Value = Status;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("status code")
            }

            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
            where
                E: DeError,
            {
                Ok(Status::try_from(value).unwrap_or(Status::GenericError))
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
            where
                E: DeError,
            {
                self.visit_i64(value as i64)
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: DeError,
            {
                if let Ok(code) = value.parse::<i64>() {
                    return Ok(Status::try_from(code).unwrap_or(Status::GenericError));
                }

                Ok(match value.to_lowercase().as_str() {
                    "success" => Status::Success,
                    "generic_error" => Status::GenericError,
                    "invalid_param" => Status::InvalidParam,
                    "no_results" => Status::NoResults,
                    "failed_to_delete" => Status::FailedToDelete,
                    "failed_to_update" => Status::FailedToUpdate,
                    "invalid_message_format" => Status::InvalidMessageFormat,
                    "duplicate_item" => Status::DuplicateItem,
                    "unknown_action" => Status::UnknownAction,
                    "invalid_session" => Status::InvalidSession,
                    "server_error" => Status::ServerError,
                    "communication_timeout" => Status::CommunicationTimeout,
                    "invalid_config" => Status::InvalidConfig,
                    "process_not_running" => Status::ProcessNotRunning,
                    "proto_invalid_response" => Status::ProtoInvalidResponse,
                    _ => Status::GenericError,
                })
            }
        }

        deserializer.deserialize_any(StatusVisitor)
    }
}

impl TryFrom<i64> for Status {
    type Error = ();

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        Ok(match value {
            0 => Self::Success,
            1 => Self::GenericError,
            2 => Self::InvalidParam,
            3 => Self::NoResults,
            4 => Self::FailedToDelete,
            5 => Self::FailedToUpdate,
            6 => Self::InvalidMessageFormat,
            7 => Self::DuplicateItem,
            8 => Self::UnknownAction,
            9 => Self::InvalidSession,
            100 => Self::ServerError,
            101 => Self::CommunicationTimeout,
            102 => Self::InvalidConfig,
            103 => Self::ProcessNotRunning,
            104 => Self::ProtoInvalidResponse,
            _ => return Err(()),
        })
    }
}

impl From<Status> for i32 {
    fn from(value: Status) -> Self {
        value as i32
    }
}

impl TryFrom<i32> for Status {
    type Error = ();

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        Self::try_from(i64::from(value))
    }
}

impl fmt::Display for Status {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(status_text(*self))
    }
}

pub fn normalize_status(value: i64) -> Status {
    Status::try_from(value).unwrap_or(Status::GenericError)
}

pub fn status_text(status: Status) -> &'static str {
    match status {
        Status::Success => "Operation successful",
        Status::GenericError => "A generic error occurred",
        Status::InvalidParam => "Invalid parameter provided",
        Status::NoResults => "No results found",
        Status::FailedToDelete => "Failed to delete item",
        Status::FailedToUpdate => "Failed to update item",
        Status::InvalidMessageFormat => "Invalid message format",
        Status::DuplicateItem => "Duplicate item found",
        Status::UnknownAction => "Unknown action requested",
        Status::InvalidSession => "Invalid session, reauthenticate with `apw auth`",
        Status::ServerError => "Server error",
        Status::CommunicationTimeout => "Communication timeout",
        Status::InvalidConfig => "Stored configuration is invalid",
        Status::ProcessNotRunning => "Helper process not running",
        Status::ProtoInvalidResponse => "Invalid response payload",
    }
}

pub const PAKE_FIELD_TID: &str = "TID";
pub const PAKE_FIELD_MSG: &str = "MSG";
pub const PAKE_FIELD_A: &str = "A";
pub const PAKE_FIELD_S: &str = "s";
pub const PAKE_FIELD_B: &str = "B";
pub const PAKE_FIELD_VER: &str = "VER";
pub const PAKE_FIELD_PROTO: &str = "PROTO";
pub const PAKE_FIELD_HAMK: &str = "HAMK";
pub const PAKE_FIELD_ERR_CODE: &str = "ErrCode";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_status_maps_unknown_and_known_values() {
        assert_eq!(normalize_status(0), Status::Success);
        assert_eq!(normalize_status(99), Status::GenericError);
        assert_eq!(normalize_status(101), Status::CommunicationTimeout);
    }

    #[test]
    fn status_text_supports_fallback_and_known_messages() {
        assert_eq!(status_text(Status::Success), "Operation successful");
        assert_eq!(
            status_text(Status::InvalidSession),
            "Invalid session, reauthenticate with `apw auth`"
        );
        assert_eq!(
            status_text(Status::GenericError),
            "A generic error occurred"
        );
    }
}
