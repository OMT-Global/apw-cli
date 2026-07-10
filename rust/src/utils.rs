#![allow(dead_code)]

use crate::error::{APWError, Result};
use crate::secrets::{delete_shared_key, read_shared_key, supports_keychain, write_shared_key};
use crate::state_root;
use crate::types::{
    normalize_status, APWConfig, APWConfigV1, APWRuntimeConfig, ExternalFallbackProvider,
    RuntimeMode, SecretSource, DEFAULT_HOST, DEFAULT_PORT,
};
use base64::{engine::general_purpose, Engine as _};
use chrono::{TimeZone, Utc};
use num_bigint::BigUint;
use num_traits::{One, Zero};
use rand::RngCore;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const SESSION_MAX_AGE_MS: u64 = 30 * 24 * 60 * 60 * 1000;

const CONFIG_DIRECTORY_MODE: u32 = 0o700;
const CONFIG_FILE_MODE: u32 = 0o600;
const CONFIG_SCHEMA: i32 = 1;
const MAX_CONFIG_SIZE_BYTES: usize = 10 * 1024;
const EXTERNAL_PROVIDER_MAX_MODE: u32 = 0o755;
const MANAGED_PREFS_DOMAIN: &str = "dev.omt.apw";
const MANAGED_PREFS_TEST_PLIST_ENV: &str = "APW_MANAGED_PREFS_PLIST";

#[derive(Debug, Clone)]
pub struct ConfigReadOptions {
    pub require_auth: bool,
    pub max_age_ms: u64,
    pub ignore_expiry: bool,
}

impl Default for ConfigReadOptions {
    fn default() -> Self {
        Self {
            require_auth: false,
            max_age_ms: SESSION_MAX_AGE_MS,
            ignore_expiry: false,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct WriteConfigInput {
    pub username: Option<String>,
    pub shared_key: Option<BigUint>,
    pub port: Option<u16>,
    pub host: Option<String>,
    pub allow_empty: bool,
    pub clear_auth: bool,
    pub runtime_mode: Option<RuntimeMode>,
    pub last_launch_status: Option<String>,
    pub last_launch_error: Option<String>,
    pub last_launch_strategy: Option<String>,
    pub bridge_status: Option<String>,
    pub bridge_browser: Option<String>,
    pub bridge_connected_at: Option<String>,
    pub bridge_last_error: Option<String>,
    pub reset_launch_metadata: bool,
    pub reset_bridge_metadata: bool,
    pub refresh_created_at: bool,
}

#[derive(Debug, Clone, Default)]
struct ManagedConfig {
    fallback_provider: Option<ExternalFallbackProvider>,
    fallback_provider_path: Option<String>,
    fallback_provider_timeout_ms: Option<u64>,
    fallback_provider_max_invocations: Option<u32>,
    supported_domains: Option<Vec<String>>,
    disable_demo: Option<bool>,
    managed_keys: Vec<&'static str>,
}

fn config_root() -> PathBuf {
    state_root::apw_state_root()
        .expect("APW state root must be validated before config paths are used")
}

fn config_path() -> PathBuf {
    config_root().join("config.json")
}

fn ensure_config_directory() -> Result<()> {
    let target = config_root();
    fs::create_dir_all(&target).map_err(|error| {
        APWError::new(
            crate::types::Status::InvalidConfig,
            format!("Failed to create config directory: {error}"),
        )
    })?;
    set_permissions(&target, CONFIG_DIRECTORY_MODE);
    Ok(())
}

fn set_permissions(path: &Path, mode: u32) {
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode));
}

fn is_valid_port(port: u16) -> bool {
    port != 0
}

fn is_valid_host(host: &str) -> bool {
    !host.trim().is_empty() && !host.contains('\0')
}

fn parse_created_at(created_at: &str) -> Option<chrono::DateTime<Utc>> {
    chrono::DateTime::parse_from_rfc3339(created_at)
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

fn stale_config(created_at: &str, max_age_ms: u64) -> bool {
    parse_created_at(created_at)
        .map(|value| {
            if value > Utc::now() {
                true
            } else {
                (Utc::now() - value).num_milliseconds() > max_age_ms as i64
            }
        })
        .unwrap_or(true)
}

fn decode_plist_text(value: &str) -> String {
    value
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
}

fn plist_value_region<'a>(plist: &'a str, key: &str) -> Option<&'a str> {
    let marker = format!("<key>{key}</key>");
    let start = plist.find(&marker)? + marker.len();
    let remaining = &plist[start..];
    let end = remaining.find("<key>").unwrap_or(remaining.len());
    Some(&remaining[..end])
}

fn plist_string_value(plist: &str, key: &str) -> Option<String> {
    let region = plist_value_region(plist, key)?;
    let start = region.find("<string>")? + "<string>".len();
    let end = region[start..].find("</string>")? + start;
    Some(decode_plist_text(region[start..end].trim()))
}

fn plist_u64_value(plist: &str, key: &str) -> Option<u64> {
    let region = plist_value_region(plist, key)?;
    let start = region.find("<integer>")? + "<integer>".len();
    let end = region[start..].find("</integer>")? + start;
    region[start..end].trim().parse().ok()
}

fn plist_bool_value(plist: &str, key: &str) -> Option<bool> {
    let region = plist_value_region(plist, key)?;
    if region.contains("<true/>") || region.contains("<true />") {
        Some(true)
    } else if region.contains("<false/>") || region.contains("<false />") {
        Some(false)
    } else {
        None
    }
}

fn plist_string_array_value(plist: &str, key: &str) -> Option<Vec<String>> {
    let region = plist_value_region(plist, key)?;
    let start = region.find("<array>")? + "<array>".len();
    let end = region[start..].find("</array>")? + start;
    let mut rest = &region[start..end];
    let mut values = Vec::new();
    while let Some(string_start) = rest.find("<string>") {
        let value_start = string_start + "<string>".len();
        let Some(value_end) = rest[value_start..].find("</string>") else {
            break;
        };
        let value_end = value_start + value_end;
        let value = decode_plist_text(rest[value_start..value_end].trim());
        if !value.is_empty() {
            values.push(value);
        }
        rest = &rest[value_end + "</string>".len()..];
    }
    Some(values)
}

fn managed_prefs_plist() -> Option<String> {
    if let Ok(value) = env::var(MANAGED_PREFS_TEST_PLIST_ENV) {
        return Some(value);
    }
    if cfg!(test) {
        return None;
    }
    if !cfg!(target_os = "macos") {
        return None;
    }
    let output = Command::new("defaults")
        .args(["export", MANAGED_PREFS_DOMAIN, "-"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let plist = String::from_utf8(output.stdout).ok()?;
    if plist.trim().is_empty() {
        None
    } else {
        Some(plist)
    }
}

fn read_managed_config() -> Option<ManagedConfig> {
    let plist = managed_prefs_plist()?;
    let mut managed = ManagedConfig::default();

    if let Some(provider) = plist_string_value(&plist, "fallbackProvider") {
        managed.fallback_provider = match provider.as_str() {
            "1password" => Some(ExternalFallbackProvider::OnePassword),
            "bitwarden" => Some(ExternalFallbackProvider::Bitwarden),
            _ => None,
        };
        if managed.fallback_provider.is_some() {
            managed.managed_keys.push("fallbackProvider");
        }
    }
    if let Some(path) = plist_string_value(&plist, "fallbackProviderPath") {
        managed.fallback_provider_path = Some(path);
        managed.managed_keys.push("fallbackProviderPath");
    }
    if let Some(timeout) = plist_u64_value(&plist, "fallbackProviderTimeoutMs") {
        managed.fallback_provider_timeout_ms = Some(timeout);
        managed.managed_keys.push("fallbackProviderTimeoutMs");
    }
    if let Some(max_invocations) = plist_u64_value(&plist, "fallbackProviderMaxInvocations") {
        if let Ok(value) = u32::try_from(max_invocations) {
            managed.fallback_provider_max_invocations = Some(value);
            managed.managed_keys.push("fallbackProviderMaxInvocations");
        }
    }
    if let Some(domains) = plist_string_array_value(&plist, "supportedDomains") {
        managed.supported_domains = Some(domains);
        managed.managed_keys.push("supportedDomains");
    }
    if let Some(disabled) = plist_bool_value(&plist, "disableDemo") {
        managed.disable_demo = Some(disabled);
        managed.managed_keys.push("disableDemo");
    }

    if managed.managed_keys.is_empty() {
        None
    } else {
        Some(managed)
    }
}

fn apply_managed_config(mut config: APWConfigV1, managed: &ManagedConfig) -> APWConfigV1 {
    if let Some(provider) = managed.fallback_provider {
        config.fallback_provider = Some(provider);
    }
    if let Some(path) = managed.fallback_provider_path.clone() {
        config.fallback_provider_path = Some(path);
    }
    if let Some(timeout) = managed.fallback_provider_timeout_ms {
        config.fallback_provider_timeout_ms = Some(timeout);
    }
    if let Some(max_invocations) = managed.fallback_provider_max_invocations {
        config.fallback_provider_max_invocations = Some(max_invocations);
    }
    if let Some(domains) = managed.supported_domains.clone() {
        config.supported_domains = domains;
    }
    if let Some(disabled) = managed.disable_demo {
        config.disable_demo = Some(disabled);
    }
    config
}

fn user_config_has_setting(key: &str) -> bool {
    let Ok(raw) = fs::read_to_string(config_path()) else {
        return false;
    };
    let Ok(Value::Object(config)) = serde_json::from_str::<Value>(&raw) else {
        return false;
    };
    let aliases: &[&str] = match key {
        "fallbackProvider" => &["fallbackProvider", "fallback_provider"],
        "fallbackProviderPath" => &["fallbackProviderPath", "fallback_provider_path"],
        "fallbackProviderTimeoutMs" => {
            &["fallbackProviderTimeoutMs", "fallback_provider_timeout_ms"]
        }
        "fallbackProviderMaxInvocations" => &[
            "fallbackProviderMaxInvocations",
            "fallback_provider_max_invocations",
        ],
        "supportedDomains" => &["supportedDomains", "supported_domains"],
        "disableDemo" => &["disableDemo", "disable_demo"],
        _ => &[key],
    };
    aliases
        .iter()
        .any(|alias| config.get(*alias).is_some_and(|value| !value.is_null()))
}

fn read_user_config_file_or_null() -> Result<APWConfigV1> {
    let path = config_path();
    let metadata = fs::symlink_metadata(&path).map_err(|_| {
        APWError::new(
            crate::types::Status::InvalidConfig,
            format!("No config file at {}.", path.display()),
        )
    })?;

    if metadata.file_type().is_symlink() || !metadata.is_file() {
        clear_config();
        return Err(APWError::new(
            crate::types::Status::InvalidConfig,
            "Config file is not a regular file.",
        ));
    }
    if metadata.len() > MAX_CONFIG_SIZE_BYTES as u64 {
        clear_config();
        return Err(APWError::new(
            crate::types::Status::InvalidConfig,
            "Config file is too large.",
        ));
    }

    let content = fs::read_to_string(&path).map_err(|_| {
        APWError::new(
            crate::types::Status::InvalidConfig,
            format!("No config file at {}.", path.display()),
        )
    })?;

    let parsed: Value = serde_json::from_str(&content).map_err(|_| {
        clear_config();
        APWError::new(
            crate::types::Status::InvalidConfig,
            "Config file contains invalid JSON.",
        )
    })?;

    if let Ok(v1) = serde_json::from_value::<APWConfigV1>(parsed.clone()) {
        if v1.schema != CONFIG_SCHEMA {
            clear_config();
            return Err(APWError::new(
                crate::types::Status::InvalidConfig,
                "Unsupported config schema.",
            ));
        }
        if !is_valid_port(v1.port) || !is_valid_host(&v1.host) {
            clear_config();
            return Err(APWError::new(
                crate::types::Status::InvalidConfig,
                "Invalid config host or port.",
            ));
        }
        return validate_external_provider_config(v1);
    }

    let legacy = serde_json::from_value::<APWConfig>(parsed).map_err(|_| {
        APWError::new(
            crate::types::Status::InvalidConfig,
            "Invalid config format. Run `apw doctor` and use `apw login <url>` through the native app broker.",
        )
    })?;

    Ok(normalize_legacy_config(legacy))
}

fn read_config_file_or_null() -> Result<APWConfigV1> {
    validate_external_provider_config(read_user_config_file_or_null()?)
}

fn validate_external_provider_config(mut config: APWConfigV1) -> Result<APWConfigV1> {
    let Some(provider) = config.fallback_provider else {
        return Ok(config);
    };
    let Some(provider_path) = config.fallback_provider_path.as_deref() else {
        return Err(APWError::new(
            crate::types::Status::InvalidConfig,
            format!(
                "Fallback provider `{}` requires an absolute `fallbackProviderPath`.",
                provider.as_str()
            ),
        ));
    };
    let resolved_path = validate_external_provider_path(provider, provider_path)?;
    config.fallback_provider_path = Some(resolved_path.display().to_string());
    Ok(config)
}

fn resolve_secret_source(raw: &APWConfigV1) -> SecretSource {
    match raw.secret_source {
        Some(value) => value,
        None => {
            if raw.shared_key.is_empty() {
                SecretSource::Keychain
            } else {
                SecretSource::File
            }
        }
    }
}

fn normalize_legacy_config(raw: APWConfig) -> APWConfigV1 {
    APWConfigV1 {
        schema: CONFIG_SCHEMA,
        port: raw.port.unwrap_or(DEFAULT_PORT),
        host: raw.host.unwrap_or_else(|| DEFAULT_HOST.to_string()),
        username: raw.username.unwrap_or_default(),
        shared_key: raw.shared_key.clone().unwrap_or_default(),
        runtime_mode: RuntimeMode::Auto,
        secret_source: if raw
            .shared_key
            .as_ref()
            .filter(|value| !value.is_empty())
            .is_some()
        {
            Some(SecretSource::File)
        } else {
            None
        },
        supported_domains: Vec::new(),
        fallback_provider: None,
        fallback_provider_path: None,
        fallback_provider_database: None,
        fallback_provider_timeout_ms: None,
        fallback_provider_max_invocations: None,
        disable_demo: None,
        last_launch_status: None,
        last_launch_error: None,
        last_launch_strategy: None,
        bridge_status: None,
        bridge_browser: None,
        bridge_connected_at: None,
        bridge_last_error: None,
        created_at: raw.created_at.unwrap_or_else(|| Utc::now().to_rfc3339()),
    }
}

pub fn read_config_file() -> Result<APWConfigV1> {
    let user_config_was_present = fs::symlink_metadata(config_path()).is_ok();
    let user = read_user_config_file_or_null();
    let managed = read_managed_config();

    match (user, managed) {
        (Ok(config), Some(managed_config)) => {
            validate_external_provider_config(apply_managed_config(config, &managed_config))
        }
        (Ok(config), None) => validate_external_provider_config(config),
        (Err(_error), Some(managed_config)) if !user_config_was_present => {
            let base = APWConfigV1 {
                created_at: Utc::now().to_rfc3339(),
                ..APWConfigV1::default()
            };
            validate_external_provider_config(apply_managed_config(base, &managed_config))
        }
        (Err(error), Some(_)) => Err(error),
        (Err(error), None) => Err(error),
    }
}

pub fn config_provenance_details() -> Value {
    let user_config_present = config_path().is_file();
    let managed = read_managed_config();
    let managed_keys = managed
        .as_ref()
        .map(|value| value.managed_keys.clone())
        .unwrap_or_default();
    let setting = |key: &'static str| {
        json!({
            "key": key,
            "source": if managed_keys.contains(&key) {
                "managed"
            } else if user_config_has_setting(key) {
                "user"
            } else {
                "default"
            }
        })
    };

    json!({
        "domain": MANAGED_PREFS_DOMAIN,
        "managed": managed.is_some(),
        "userConfigPresent": user_config_present,
        "settings": [
            setting("fallbackProvider"),
            setting("fallbackProviderPath"),
            setting("fallbackProviderTimeoutMs"),
            setting("fallbackProviderMaxInvocations"),
            setting("supportedDomains"),
            setting("disableDemo")
        ]
    })
}

fn read_user_supported_domains_non_destructive() -> Vec<String> {
    let Ok(content) = fs::read_to_string(config_path()) else {
        return Vec::new();
    };
    let Ok(parsed) = serde_json::from_str::<Value>(&content) else {
        return Vec::new();
    };
    serde_json::from_value::<APWConfigV1>(parsed)
        .ok()
        .filter(|config| config.schema == CONFIG_SCHEMA)
        .map(|config| config.supported_domains)
        .unwrap_or_default()
}

pub fn configured_supported_domains_non_destructive() -> Vec<String> {
    if let Some(domains) = read_managed_config().and_then(|managed| managed.supported_domains) {
        return domains;
    }
    read_user_supported_domains_non_destructive()
}

pub fn validate_external_provider_path(
    provider: ExternalFallbackProvider,
    provider_path: &str,
) -> Result<PathBuf> {
    let raw_path = PathBuf::from(provider_path);
    if provider_path.starts_with('~') || !raw_path.is_absolute() {
        return Err(APWError::new(
            crate::types::Status::InvalidConfig,
            format!(
                "Fallback provider `{}` must use an absolute executable path; `~` and relative paths are not allowed.",
                provider.as_str()
            ),
        ));
    }

    let resolved_path = raw_path.canonicalize().map_err(|error| {
        APWError::new(
            crate::types::Status::InvalidConfig,
            format!(
                "Fallback provider `{}` path {} could not be resolved: {error}",
                provider.as_str(),
                raw_path.display()
            ),
        )
    })?;

    let metadata = fs::metadata(&resolved_path).map_err(|error| {
        APWError::new(
            crate::types::Status::InvalidConfig,
            format!(
                "Fallback provider `{}` path {} could not be inspected: {error}",
                provider.as_str(),
                resolved_path.display()
            ),
        )
    })?;
    if !metadata.is_file() {
        return Err(APWError::new(
            crate::types::Status::InvalidConfig,
            format!(
                "Fallback provider `{}` path {} must resolve to a regular file.",
                provider.as_str(),
                resolved_path.display()
            ),
        ));
    }

    // SAFETY: `geteuid` reads the effective uid for the current process and has
    // no memory-safety preconditions.
    let current_uid = unsafe { libc::geteuid() };
    if metadata.uid() != current_uid {
        return Err(APWError::new(
            crate::types::Status::InvalidConfig,
            format!(
                "Fallback provider `{}` path {} must be owned by the current user.",
                provider.as_str(),
                resolved_path.display()
            ),
        ));
    }

    let mode = metadata.permissions().mode() & 0o7777;
    if mode & !EXTERNAL_PROVIDER_MAX_MODE != 0 {
        return Err(APWError::new(
            crate::types::Status::InvalidConfig,
            format!(
                "Fallback provider `{}` path {} has insecure permissions {:04o}; use 0755 or more restrictive permissions.",
                provider.as_str(),
                resolved_path.display(),
                mode
            ),
        ));
    }

    Ok(resolved_path)
}

#[allow(dead_code)]
pub fn read_config_file_or_empty() -> APWConfigV1 {
    read_config_file().unwrap_or_else(|_| APWConfigV1 {
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
        supported_domains: Vec::new(),
        fallback_provider: None,
        fallback_provider_path: None,
        fallback_provider_database: None,
        fallback_provider_timeout_ms: None,
        fallback_provider_max_invocations: None,
        disable_demo: None,
        created_at: Utc.timestamp_nanos(0).to_rfc3339(),
    })
}

pub fn read_config(opts: Option<ConfigReadOptions>) -> Result<APWRuntimeConfig> {
    let options = opts.unwrap_or_default();
    let raw = match read_config_file() {
        Ok(value) => value,
        Err(error) => {
            if options.require_auth {
                return Err(error);
            }
            return Ok(APWRuntimeConfig {
                schema: CONFIG_SCHEMA,
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
                fallback_provider_database: None,
                fallback_provider_timeout_ms: None,
                fallback_provider_max_invocations: None,
                supported_domains: Vec::new(),
                disable_demo: None,
                created_at: Utc.timestamp_nanos(0).to_rfc3339(),
            });
        }
    };

    if !is_valid_port(raw.port) || !is_valid_host(&raw.host) {
        clear_config();
        if options.require_auth {
            return Err(APWError::new(
                crate::types::Status::InvalidConfig,
                "Invalid config host/port.",
            ));
        }
        return Ok(APWRuntimeConfig {
            schema: CONFIG_SCHEMA,
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
            fallback_provider_database: None,
            fallback_provider_timeout_ms: None,
            fallback_provider_max_invocations: None,
            supported_domains: Vec::new(),
            disable_demo: None,
            created_at: Utc.timestamp_nanos(0).to_rfc3339(),
        });
    }

    if stale_config(&raw.created_at, options.max_age_ms) && !options.ignore_expiry {
        clear_config();
        if options.require_auth {
            return Err(APWError::new(
                crate::types::Status::InvalidSession,
                "Session expired. Use `apw app launch` and `apw login <url>` through the native app broker.",
            ));
        }
        return Ok(APWRuntimeConfig {
            schema: CONFIG_SCHEMA,
            port: raw.port,
            host: raw.host,
            username: raw.username,
            shared_key: BigUint::zero(),
            runtime_mode: raw.runtime_mode,
            last_launch_status: raw.last_launch_status,
            last_launch_error: raw.last_launch_error,
            last_launch_strategy: raw.last_launch_strategy,
            bridge_status: raw.bridge_status,
            bridge_browser: raw.bridge_browser,
            bridge_connected_at: raw.bridge_connected_at,
            bridge_last_error: raw.bridge_last_error,
            fallback_provider: raw.fallback_provider,
            fallback_provider_path: raw.fallback_provider_path,
            fallback_provider_database: raw.fallback_provider_database,
            fallback_provider_timeout_ms: raw.fallback_provider_timeout_ms,
            fallback_provider_max_invocations: raw.fallback_provider_max_invocations,
            supported_domains: raw.supported_domains,
            disable_demo: raw.disable_demo,
            created_at: raw.created_at,
        });
    }

    let secret_source = resolve_secret_source(&raw);
    let shared_secret = match secret_source {
        SecretSource::File => {
            if raw.shared_key.is_empty() {
                None
            } else {
                Some(raw.shared_key.clone())
            }
        }
        SecretSource::Keychain => {
            if raw.username.is_empty() {
                None
            } else {
                read_shared_key(&raw.username).unwrap_or(None).or_else(|| {
                    if raw.shared_key.is_empty() {
                        None
                    } else {
                        Some(raw.shared_key.clone())
                    }
                })
            }
        }
    };

    let shared_key = match shared_secret {
        Some(secret) => {
            let value = read_bigint(&secret).inspect_err(|_| {
                clear_config();
            })?;
            if value.is_zero() {
                None
            } else {
                Some(value)
            }
        }
        None => None,
    };

    if options.require_auth && (raw.username.is_empty() || shared_key.is_none()) {
        clear_config();
        return Err(APWError::new(
            crate::types::Status::InvalidSession,
            "No active session. Use `apw app launch` and `apw login <url>` through the native app broker.",
        ));
    }

    Ok(APWRuntimeConfig {
        schema: raw.schema,
        port: raw.port,
        host: raw.host,
        username: raw.username,
        shared_key: shared_key.unwrap_or_else(BigUint::zero),
        runtime_mode: raw.runtime_mode,
        last_launch_status: raw.last_launch_status,
        last_launch_error: raw.last_launch_error,
        last_launch_strategy: raw.last_launch_strategy,
        bridge_status: raw.bridge_status,
        bridge_browser: raw.bridge_browser,
        bridge_connected_at: raw.bridge_connected_at,
        bridge_last_error: raw.bridge_last_error,
        fallback_provider: raw.fallback_provider,
        fallback_provider_path: raw.fallback_provider_path,
        fallback_provider_database: raw.fallback_provider_database,
        fallback_provider_timeout_ms: raw.fallback_provider_timeout_ms,
        fallback_provider_max_invocations: raw.fallback_provider_max_invocations,
        supported_domains: raw.supported_domains,
        disable_demo: raw.disable_demo,
        created_at: raw.created_at,
    })
}

pub fn clear_config() {
    if let Ok(raw) = fs::read_to_string(config_path()) {
        if let Ok(v1) = serde_json::from_str::<APWConfigV1>(&raw) {
            if v1.secret_source == Some(SecretSource::Keychain) && !v1.username.is_empty() {
                let _ = delete_shared_key(&v1.username);
            }
        } else if let Ok(legacy) = serde_json::from_str::<APWConfig>(&raw) {
            if !legacy.shared_key.clone().unwrap_or_default().is_empty() {
                if let Some(username) = legacy.username {
                    let _ = delete_shared_key(&username);
                }
            }
        }
    }
    let _ = fs::remove_file(config_path());
}

pub fn write_config(input: WriteConfigInput) -> Result<APWConfigV1> {
    ensure_config_directory()?;

    let existing = read_config_file_or_null().ok();
    let clear_auth = input.clear_auth;
    let port = input
        .port
        .or_else(|| existing.as_ref().map(|value| value.port))
        .unwrap_or(DEFAULT_PORT);
    let host = input
        .host
        .as_ref()
        .filter(|value| is_valid_host(value))
        .cloned()
        .or_else(|| existing.as_ref().map(|value| value.host.clone()))
        .unwrap_or_else(|| DEFAULT_HOST.to_string());
    let username = if clear_auth {
        input.username.unwrap_or_default()
    } else {
        input
            .username
            .or_else(|| existing.as_ref().map(|value| value.username.clone()))
            .unwrap_or_default()
    };

    if port == 0 || !is_valid_host(&host) {
        return Err(APWError::new(
            crate::types::Status::InvalidParam,
            "Invalid config host/port.",
        ));
    }

    if !input.allow_empty && username.is_empty() {
        return Err(APWError::new(
            crate::types::Status::InvalidConfig,
            "Cannot persist incomplete config. Use `apw app launch` and `apw login <url>` through the native app broker.",
        ));
    }

    let mut secret_source = if clear_auth {
        SecretSource::File
    } else {
        existing
            .as_ref()
            .and_then(|value| value.secret_source)
            .unwrap_or(SecretSource::File)
    };

    let mut shared_key = if clear_auth {
        String::new()
    } else {
        existing
            .as_ref()
            .map(|value| value.shared_key.clone())
            .unwrap_or_default()
    };

    if clear_auth {
        if let Some(value) = existing
            .as_ref()
            .filter(|value| value.secret_source == Some(SecretSource::Keychain))
            .filter(|value| !value.username.is_empty())
        {
            let _ = delete_shared_key(&value.username);
        }
    }

    if let Some(incoming_shared_key) = input.shared_key.as_ref() {
        if !username.is_empty() && supports_keychain() {
            write_shared_key(&username, &bigint_to_base64(incoming_shared_key))?;
            secret_source = SecretSource::Keychain;
            shared_key.clear();
        } else {
            secret_source = SecretSource::File;
            shared_key = bigint_to_base64(incoming_shared_key);
        }
    } else if input.allow_empty
        && existing
            .as_ref()
            .is_some_and(|value| value.secret_source == Some(SecretSource::Keychain))
        && !username.is_empty()
    {
        let _ = delete_shared_key(&username);
        shared_key.clear();
        secret_source = SecretSource::Keychain;
    } else if existing.as_ref().is_none()
        && secret_source == SecretSource::Keychain
        && !username.is_empty()
    {
        let _ = delete_shared_key(&username);
        shared_key.clear();
    }

    if !input.allow_empty {
        if username.is_empty() || (secret_source == SecretSource::File && shared_key.is_empty()) {
            return Err(APWError::new(
                crate::types::Status::InvalidConfig,
                "Cannot persist incomplete config. Use `apw app launch` and `apw login <url>` through the native app broker.",
            ));
        }
        if secret_source == SecretSource::Keychain && !supports_keychain() {
            secret_source = SecretSource::File;
        }
    }

    let runtime_mode = input.runtime_mode.unwrap_or_else(|| {
        existing
            .as_ref()
            .map(|value| value.runtime_mode)
            .unwrap_or(RuntimeMode::Auto)
    });
    let last_launch_status = input.last_launch_status.or_else(|| {
        if input.reset_launch_metadata {
            None
        } else {
            existing
                .as_ref()
                .and_then(|value| value.last_launch_status.clone())
        }
    });
    let last_launch_error = input.last_launch_error.or_else(|| {
        if input.reset_launch_metadata {
            None
        } else {
            existing
                .as_ref()
                .and_then(|value| value.last_launch_error.clone())
        }
    });
    let last_launch_strategy = input.last_launch_strategy.or_else(|| {
        if input.reset_launch_metadata {
            None
        } else {
            existing
                .as_ref()
                .and_then(|value| value.last_launch_strategy.clone())
        }
    });
    let bridge_status = input.bridge_status.or_else(|| {
        if input.reset_bridge_metadata {
            None
        } else {
            existing
                .as_ref()
                .and_then(|value| value.bridge_status.clone())
        }
    });
    let bridge_browser = input.bridge_browser.or_else(|| {
        if input.reset_bridge_metadata {
            None
        } else {
            existing
                .as_ref()
                .and_then(|value| value.bridge_browser.clone())
        }
    });
    let bridge_connected_at = input.bridge_connected_at.or_else(|| {
        if input.reset_bridge_metadata {
            None
        } else {
            existing
                .as_ref()
                .and_then(|value| value.bridge_connected_at.clone())
        }
    });
    let bridge_last_error = input.bridge_last_error.or_else(|| {
        if input.reset_bridge_metadata {
            None
        } else {
            existing
                .as_ref()
                .and_then(|value| value.bridge_last_error.clone())
        }
    });
    let created_at = if input.refresh_created_at || clear_auth || existing.is_none() {
        Utc::now().to_rfc3339()
    } else {
        existing
            .as_ref()
            .map(|value| value.created_at.clone())
            .unwrap_or_else(|| Utc::now().to_rfc3339())
    };

    let updated = APWConfigV1 {
        schema: CONFIG_SCHEMA,
        port,
        host,
        username,
        shared_key,
        runtime_mode,
        secret_source: Some(secret_source),
        last_launch_status,
        last_launch_error,
        last_launch_strategy,
        bridge_status,
        bridge_browser,
        bridge_connected_at,
        bridge_last_error,
        created_at,
        supported_domains: existing
            .as_ref()
            .map(|value| value.supported_domains.clone())
            .unwrap_or_default(),
        fallback_provider: existing.as_ref().and_then(|value| value.fallback_provider),
        fallback_provider_path: existing
            .as_ref()
            .and_then(|value| value.fallback_provider_path.clone()),
        fallback_provider_database: existing
            .as_ref()
            .and_then(|value| value.fallback_provider_database.clone()),
        fallback_provider_timeout_ms: existing
            .as_ref()
            .and_then(|value| value.fallback_provider_timeout_ms),
        fallback_provider_max_invocations: existing
            .as_ref()
            .and_then(|value| value.fallback_provider_max_invocations),
        disable_demo: existing.as_ref().and_then(|value| value.disable_demo),
    };

    let mut serialized = serde_json::to_string_pretty(&updated).map_err(|error| {
        APWError::new(
            crate::types::Status::GenericError,
            format!("Failed to serialize config: {error}"),
        )
    })?;

    if serialized.len() > MAX_CONFIG_SIZE_BYTES {
        return Err(APWError::new(
            crate::types::Status::InvalidConfig,
            "Config payload too large.",
        ));
    }

    let temp_suffix = to_hex(&random_bytes(8));
    let path = config_path();
    let temp = path.with_extension(format!("tmp.{temp_suffix}"));

    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&temp)
        .map_err(|error| {
            APWError::new(
                crate::types::Status::InvalidConfig,
                format!("Failed to create config file: {error}"),
            )
        })?;
    set_permissions(&temp, CONFIG_FILE_MODE);
    file.write_all(serialized.as_bytes()).map_err(|error| {
        APWError::new(
            crate::types::Status::InvalidConfig,
            format!("Failed to write config: {error}"),
        )
    })?;
    file.sync_all().map_err(|error| {
        APWError::new(
            crate::types::Status::InvalidConfig,
            format!("Failed to sync config: {error}"),
        )
    })?;
    drop(file);
    fs::rename(&temp, &path).map_err(|error| {
        APWError::new(
            crate::types::Status::InvalidConfig,
            format!("Failed to save config: {error}"),
        )
    })?;
    set_permissions(&path, CONFIG_FILE_MODE);

    serialized.clear();
    Ok(updated)
}

pub fn read_bigint(input: &str) -> Result<BigUint> {
    let bytes = general_purpose::STANDARD.decode(input).map_err(|_| {
        APWError::new(
            crate::types::Status::InvalidConfig,
            "Invalid config payload format.",
        )
    })?;
    Ok(BigUint::from_bytes_be(&bytes))
}

pub fn bigint_to_base64(value: &BigUint) -> String {
    general_purpose::STANDARD.encode(value.to_bytes_be())
}

pub fn to_base64(bytes: &[u8]) -> String {
    general_purpose::STANDARD.encode(bytes)
}

pub fn random_bytes(count: usize) -> Vec<u8> {
    let mut output = vec![0_u8; count];
    rand::thread_rng().fill_bytes(&mut output);
    output
}

pub fn to_hex(bytes: &[u8]) -> String {
    hex::encode(bytes)
}

pub fn pad(input: &[u8], length: usize) -> Vec<u8> {
    if input.len() >= length {
        return input[input.len() - length..].to_vec();
    }

    let mut output = vec![0_u8; length];
    output[length - input.len()..].copy_from_slice(input);
    output
}

pub fn sha256(data: &[u8]) -> Vec<u8> {
    let mut digest = Sha256::new();
    digest.update(data);
    digest.finalize().to_vec()
}

pub fn mod_(left: &BigUint, modulus: &BigUint) -> BigUint {
    if modulus.is_zero() {
        return BigUint::zero();
    }

    let mut remainder = left % modulus;
    if remainder > *modulus {
        remainder %= modulus;
    }
    remainder
}

pub fn powermod(base: &BigUint, exponent: &BigUint, modulus: &BigUint) -> Result<BigUint> {
    if exponent.is_zero() {
        return Ok(BigUint::one());
    }

    let mut result = BigUint::one();
    let mut base = mod_(base, modulus);
    let mut exp = exponent.clone();

    while !exp.is_zero() {
        if (&exp & BigUint::one()) == BigUint::one() {
            result = mod_(&(result * &base), modulus);
        }
        exp >>= 1u8;
        if !exp.is_zero() {
            base = mod_(&(&base * &base), modulus);
        }
    }

    Ok(result)
}

#[allow(dead_code)]
pub fn normalize_status_code(code: i64) -> crate::types::Status {
    normalize_status(code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::{
        reset_security_command_runner_for_tests, set_security_command_runner_for_tests,
        supports_keychain_for_tests,
    };
    use serial_test::serial;
    use std::env;
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    fn with_temp_home<F, R>(run: F) -> R
    where
        F: FnOnce() -> R,
    {
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

    fn config_path_for_test() -> std::path::PathBuf {
        config_root().join("config.json")
    }

    fn managed_prefs_plist_for_test(provider_path: &Path) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>fallbackProvider</key>
  <string>1password</string>
  <key>fallbackProviderPath</key>
  <string>{}</string>
  <key>fallbackProviderTimeoutMs</key>
  <integer>2500</integer>
  <key>fallbackProviderMaxInvocations</key>
  <integer>2</integer>
  <key>supportedDomains</key>
  <array>
    <string>example.com</string>
    <string>login.example.com</string>
  </array>
  <key>disableDemo</key>
  <true/>
</dict>
</plist>"#,
            provider_path.display()
        )
    }

    fn write_test_provider(path: &Path) {
        fs::write(path, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    #[serial]
    fn read_config_migrates_legacy_shape() {
        with_temp_home(|| {
            let created_at = chrono::Utc::now().to_rfc3339();
            let legacy = APWConfig {
                port: Some(10_012),
                shared_key: Some(bigint_to_base64(&1u32.into())),
                username: Some("alice".to_string()),
                host: Some("127.0.0.1".to_string()),
                created_at: Some(created_at.to_string()),
            };

            fs::create_dir_all(config_root()).unwrap();
            fs::write(
                config_path_for_test(),
                serde_json::to_string(&legacy).unwrap(),
            )
            .unwrap();

            let migrated = read_config_file_or_null().unwrap();
            assert_eq!(migrated.schema, 1);
            assert_eq!(migrated.port, 10_012);
            assert_eq!(migrated.host, "127.0.0.1");
            assert_eq!(migrated.username, "alice");
            assert_eq!(migrated.shared_key, bigint_to_base64(&1u32.into()));
            assert_eq!(migrated.created_at, created_at);

            let runtime = read_config(Some(ConfigReadOptions {
                require_auth: false,
                max_age_ms: 1000 * 60 * 60 * 24 * 365,
                ignore_expiry: false,
            }))
            .unwrap();

            assert_eq!(runtime.username, "alice");
            assert_eq!(runtime.shared_key, 1u32.into());
            assert_eq!(runtime.port, 10_012);
            assert_eq!(runtime.host, "127.0.0.1");
            assert_eq!(runtime.created_at, created_at.to_string());
        });
    }

    #[test]
    #[serial]
    fn read_config_applies_managed_preferences_before_user_config() {
        with_temp_home(|| {
            let managed_provider = config_root().join("managed-provider");
            let user_provider = config_root().join("user-provider");
            fs::create_dir_all(config_root()).unwrap();
            write_test_provider(&managed_provider);
            write_test_provider(&user_provider);

            let user = APWConfigV1 {
                username: "alice".to_string(),
                shared_key: bigint_to_base64(&1u32.into()),
                fallback_provider: Some(ExternalFallbackProvider::Bitwarden),
                fallback_provider_path: Some(user_provider.display().to_string()),
                fallback_provider_timeout_ms: Some(9000),
                fallback_provider_max_invocations: Some(9),
                supported_domains: vec!["user.example.com".to_string()],
                disable_demo: Some(false),
                ..APWConfigV1::default()
            };
            fs::write(
                config_path_for_test(),
                serde_json::to_string(&user).unwrap(),
            )
            .unwrap();

            env::set_var(
                MANAGED_PREFS_TEST_PLIST_ENV,
                managed_prefs_plist_for_test(&managed_provider),
            );
            let config = read_config_file().unwrap();
            let runtime = read_config(Some(ConfigReadOptions {
                require_auth: false,
                max_age_ms: SESSION_MAX_AGE_MS,
                ignore_expiry: true,
            }))
            .unwrap();
            env::remove_var(MANAGED_PREFS_TEST_PLIST_ENV);

            assert_eq!(
                config.fallback_provider,
                Some(ExternalFallbackProvider::OnePassword)
            );
            assert_eq!(
                config.fallback_provider_path.as_deref(),
                Some(managed_provider.canonicalize().unwrap().to_str().unwrap())
            );
            assert_eq!(config.fallback_provider_timeout_ms, Some(2500));
            assert_eq!(config.fallback_provider_max_invocations, Some(2));
            assert_eq!(
                config.supported_domains,
                vec!["example.com", "login.example.com"]
            );
            assert_eq!(config.disable_demo, Some(true));
            assert_eq!(
                runtime.supported_domains,
                vec!["example.com", "login.example.com"]
            );
            assert_eq!(runtime.disable_demo, Some(true));
        });
    }

    #[test]
    #[serial]
    fn managed_preferences_do_not_mask_invalid_user_config() {
        with_temp_home(|| {
            let managed_provider = config_root().join("managed-provider");
            fs::create_dir_all(config_root()).unwrap();
            write_test_provider(&managed_provider);
            fs::write(config_path_for_test(), "{invalid").unwrap();

            env::set_var(
                MANAGED_PREFS_TEST_PLIST_ENV,
                managed_prefs_plist_for_test(&managed_provider),
            );
            let result = read_config_file();
            env::remove_var(MANAGED_PREFS_TEST_PLIST_ENV);

            assert!(result.is_err());
            assert_eq!(
                result.unwrap_err().code,
                crate::types::Status::InvalidConfig
            );
            assert!(!config_path_for_test().exists());
        });
    }

    #[test]
    #[serial]
    fn supported_domain_probe_read_preserves_invalid_user_config() {
        with_temp_home(|| {
            fs::create_dir_all(config_root()).unwrap();
            fs::write(config_path_for_test(), "{invalid").unwrap();

            let domains = configured_supported_domains_non_destructive();

            assert!(domains.is_empty());
            assert!(config_path_for_test().exists());
        });
    }

    #[test]
    #[serial]
    fn supported_domain_probe_read_uses_managed_domains_without_clearing_user_config() {
        with_temp_home(|| {
            let managed_provider = config_root().join("managed-provider");
            fs::create_dir_all(config_root()).unwrap();
            write_test_provider(&managed_provider);
            fs::write(config_path_for_test(), "{invalid").unwrap();

            env::set_var(
                MANAGED_PREFS_TEST_PLIST_ENV,
                managed_prefs_plist_for_test(&managed_provider),
            );
            let domains = configured_supported_domains_non_destructive();
            env::remove_var(MANAGED_PREFS_TEST_PLIST_ENV);

            assert_eq!(domains, vec!["example.com", "login.example.com"]);
            assert!(config_path_for_test().exists());
        });
    }

    #[test]
    #[serial]
    fn config_provenance_reports_specific_setting_sources() {
        with_temp_home(|| {
            fs::create_dir_all(config_root()).unwrap();
            fs::write(
                config_path_for_test(),
                r#"{"schema":1,"port":10000,"host":"127.0.0.1","username":"alice","sharedKey":"","createdAt":"1970-01-01T00:00:00+00:00","fallbackProvider":null,"disableDemo":false}"#,
            )
            .unwrap();

            let details = config_provenance_details();
            let settings = details["settings"].as_array().unwrap();
            let source_for = |key: &str| {
                settings
                    .iter()
                    .find(|setting| setting["key"] == key)
                    .and_then(|setting| setting["source"].as_str())
                    .unwrap()
            };

            assert_eq!(source_for("disableDemo"), "user");
            assert_eq!(source_for("fallbackProvider"), "default");
            assert_eq!(source_for("supportedDomains"), "default");
        });
    }

    #[test]
    #[serial]
    fn read_config_clears_invalid_json() {
        with_temp_home(|| {
            fs::create_dir_all(config_root()).unwrap();
            fs::write(config_path_for_test(), "{invalid").unwrap();

            assert!(read_config_file_or_null().is_err());
            assert!(!config_path_for_test().exists());
        });
    }

    #[test]
    #[serial]
    fn read_config_rejects_oversized_payload() {
        with_temp_home(|| {
            fs::create_dir_all(config_root()).unwrap();
            let oversized = "a".repeat(MAX_CONFIG_SIZE_BYTES + 1);
            fs::write(config_path_for_test(), oversized).unwrap();

            let result = read_config_file_or_null();
            assert!(result.is_err());
            assert_eq!(
                result.unwrap_err().code,
                crate::types::Status::InvalidConfig
            );
            assert!(!config_path_for_test().exists());
        });
    }

    #[test]
    #[serial]
    fn read_config_rejects_symlink_file_path() {
        with_temp_home(|| {
            let target = config_root().join("payload.json");
            let link = config_path_for_test();
            fs::create_dir_all(config_root()).unwrap();
            fs::write(&target, "{}").unwrap();
            symlink(&target, &link).unwrap();

            let result = read_config_file_or_null();
            assert!(result.is_err());
            assert_eq!(
                result.unwrap_err().code,
                crate::types::Status::InvalidConfig
            );
            assert!(!link.exists());
            assert!(target.exists());
        });
    }

    #[test]
    #[serial]
    fn stale_config_is_invalid_with_reauth_path() {
        with_temp_home(|| {
            let stale = APWConfigV1 {
                schema: 1,
                port: 10_012,
                host: "127.0.0.1".to_string(),
                username: "alice".to_string(),
                shared_key: bigint_to_base64(&1u32.into()),
                secret_source: Some(SecretSource::File),
                supported_domains: Vec::new(),
                fallback_provider: None,
                fallback_provider_path: None,
                fallback_provider_database: None,
                fallback_provider_timeout_ms: None,
                fallback_provider_max_invocations: None,
                disable_demo: None,
                created_at: (chrono::Utc::now() - chrono::Duration::days(40)).to_rfc3339(),
                runtime_mode: RuntimeMode::Auto,
                last_launch_status: None,
                last_launch_error: None,
                last_launch_strategy: None,
                bridge_status: None,
                bridge_browser: None,
                bridge_connected_at: None,
                bridge_last_error: None,
            };

            fs::create_dir_all(config_root()).unwrap();
            fs::write(
                config_path_for_test(),
                serde_json::to_string(&stale).unwrap(),
            )
            .unwrap();

            let result = read_config(Some(ConfigReadOptions {
                require_auth: true,
                max_age_ms: SESSION_MAX_AGE_MS,
                ignore_expiry: false,
            }));

            assert!(result.is_err());
            assert!(!config_path_for_test().exists());
        });
    }

    #[test]
    #[serial]
    fn clear_config_removes_keychain_secret_for_keychain_metadata() {
        let delete_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Vec<String>>::new()));

        with_temp_home(|| {
            let calls = delete_calls.clone();
            supports_keychain_for_tests(Some(true));
            set_security_command_runner_for_tests(move |args| {
                if args.first() == Some(&"delete-generic-password") {
                    calls
                        .lock()
                        .expect("security args lock")
                        .push(args.iter().map(|value| value.to_string()).collect());
                }

                Ok(crate::secrets::make_security_result(0, "", ""))
            });

            let stale = APWConfigV1 {
                schema: 1,
                port: 10_012,
                host: "127.0.0.1".to_string(),
                username: "alice".to_string(),
                shared_key: String::new(),
                secret_source: Some(SecretSource::Keychain),
                supported_domains: Vec::new(),
                fallback_provider: None,
                fallback_provider_path: None,
                fallback_provider_database: None,
                fallback_provider_timeout_ms: None,
                fallback_provider_max_invocations: None,
                disable_demo: None,
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

            fs::create_dir_all(config_root()).unwrap();
            fs::write(
                config_path_for_test(),
                serde_json::to_string(&stale).unwrap(),
            )
            .unwrap();

            clear_config();
            assert!(!config_path_for_test().exists());

            let captured = delete_calls.lock().expect("security args lock");
            assert_eq!(captured.len(), 1);
            assert!(captured[0].contains(&"alice".to_string()));
            assert!(captured[0].contains(&"delete-generic-password".to_string()));

            supports_keychain_for_tests(None);
            reset_security_command_runner_for_tests();
        });
    }

    #[test]
    #[serial]
    fn write_config_enforces_permissions_and_modes() {
        with_temp_home(|| {
            supports_keychain_for_tests(Some(false));
            let written = write_config(WriteConfigInput {
                username: Some("alice".to_string()),
                shared_key: Some(42u32.into()),
                port: Some(10_012),
                host: Some("127.0.0.1".to_string()),
                allow_empty: false,
                ..WriteConfigInput::default()
            })
            .unwrap();
            supports_keychain_for_tests(None);

            let dir_meta = fs::metadata(config_root()).unwrap();
            let dir_mode = dir_meta.permissions().mode() & 0o777;
            assert_eq!(dir_mode, 0o700);

            let file_meta = fs::metadata(config_path_for_test()).unwrap();
            let file_mode = file_meta.permissions().mode() & 0o777;
            assert_eq!(file_mode, 0o600);

            assert_eq!(written.port, 10_012);
            assert_eq!(written.username, "alice");
        });
    }

    #[test]
    #[serial]
    fn read_config_rejects_invalid_host_payload() {
        with_temp_home(|| {
            fs::create_dir_all(config_root()).unwrap();
            let invalid = APWConfigV1 {
                schema: 1,
                port: 10_012,
                host: "\0bad".to_string(),
                username: "alice".to_string(),
                shared_key: String::new(),
                secret_source: Some(SecretSource::File),
                supported_domains: Vec::new(),
                fallback_provider: None,
                fallback_provider_path: None,
                fallback_provider_database: None,
                fallback_provider_timeout_ms: None,
                fallback_provider_max_invocations: None,
                disable_demo: None,
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
            fs::write(
                config_path_for_test(),
                serde_json::to_string(&invalid).unwrap(),
            )
            .unwrap();

            let result = read_config_file_or_null();

            assert!(result.is_err());
            assert_eq!(
                result.unwrap_err().code,
                crate::types::Status::InvalidConfig
            );
            assert!(!config_path_for_test().exists());
        });
    }

    #[test]
    #[serial]
    fn write_config_rejects_zero_port() {
        with_temp_home(|| {
            let result = write_config(WriteConfigInput {
                username: Some("alice".to_string()),
                shared_key: Some(1u32.into()),
                port: Some(0),
                host: Some("127.0.0.1".to_string()),
                allow_empty: false,
                ..WriteConfigInput::default()
            });
            assert!(result.is_err());
            assert_eq!(result.unwrap_err().code, crate::types::Status::InvalidParam);
        });
    }

    #[test]
    #[serial]
    fn write_config_rejects_incomplete_input() {
        with_temp_home(|| {
            fs::create_dir_all(config_root()).unwrap();
            let result = write_config(WriteConfigInput {
                username: None,
                shared_key: Some(1u32.into()),
                port: Some(10_012),
                host: Some("127.0.0.1".to_string()),
                allow_empty: false,
                ..WriteConfigInput::default()
            });
            assert!(result.is_err());
        });
    }

    #[test]
    #[serial]
    fn write_config_can_clear_auth_while_preserving_launch_metadata() {
        with_temp_home(|| {
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

            let written = write_config(WriteConfigInput {
                port: Some(10_045),
                host: Some("127.0.0.1".to_string()),
                allow_empty: true,
                clear_auth: true,
                runtime_mode: Some(RuntimeMode::Auto),
                last_launch_status: Some("ok".to_string()),
                last_launch_error: None,
                last_launch_strategy: Some("archived".to_string()),
                ..WriteConfigInput::default()
            })
            .unwrap();

            assert_eq!(written.port, 10_045);
            assert_eq!(written.host, "127.0.0.1");
            assert_eq!(written.username, "");
            assert_eq!(written.shared_key, "");
            assert_eq!(written.runtime_mode, RuntimeMode::Auto);
            assert_eq!(written.last_launch_status.as_deref(), Some("ok"));
            assert_eq!(written.last_launch_error, None);
            assert_eq!(written.last_launch_strategy.as_deref(), Some("archived"));

            let runtime = read_config(Some(ConfigReadOptions {
                require_auth: false,
                max_age_ms: SESSION_MAX_AGE_MS,
                ignore_expiry: false,
            }))
            .unwrap();
            assert_eq!(runtime.username, "");
            assert!(runtime.shared_key.is_zero());

            supports_keychain_for_tests(None);
        });
    }

    #[test]
    #[serial]
    fn write_config_allow_empty_preserves_existing_credentials() {
        with_temp_home(|| {
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

            let written = write_config(WriteConfigInput {
                port: Some(10_013),
                host: Some("127.0.0.1".to_string()),
                allow_empty: true,
                runtime_mode: Some(RuntimeMode::Auto),
                last_launch_status: Some("failed".to_string()),
                last_launch_error: Some("probe failed".to_string()),
                last_launch_strategy: Some("archived".to_string()),
                ..WriteConfigInput::default()
            })
            .unwrap();

            assert_eq!(written.username, "alice");
            assert!(!written.shared_key.is_empty());
            assert_eq!(written.last_launch_status.as_deref(), Some("failed"));

            let runtime = read_config(Some(ConfigReadOptions {
                require_auth: false,
                max_age_ms: SESSION_MAX_AGE_MS,
                ignore_expiry: false,
            }))
            .unwrap();
            assert_eq!(runtime.username, "alice");
            assert!(!runtime.shared_key.is_zero());

            supports_keychain_for_tests(None);
        });
    }

    #[test]
    #[serial]
    fn metadata_only_writes_preserve_existing_created_at() {
        with_temp_home(|| {
            supports_keychain_for_tests(Some(false));
            let written = write_config(WriteConfigInput {
                username: Some("alice".to_string()),
                shared_key: Some(1u32.into()),
                port: Some(10_012),
                host: Some("127.0.0.1".to_string()),
                allow_empty: false,
                refresh_created_at: true,
                ..WriteConfigInput::default()
            })
            .unwrap();

            let preserved = write_config(WriteConfigInput {
                port: Some(10_013),
                host: Some("127.0.0.1".to_string()),
                allow_empty: true,
                runtime_mode: Some(RuntimeMode::Auto),
                last_launch_status: Some("failed".to_string()),
                last_launch_error: Some("probe failed".to_string()),
                last_launch_strategy: Some("archived".to_string()),
                ..WriteConfigInput::default()
            })
            .unwrap();

            assert_eq!(preserved.created_at, written.created_at);
            supports_keychain_for_tests(None);
        });
    }

    #[test]
    #[serial]
    fn archived_bridge_metadata_resets_launch_fields_without_clearing_auth() {
        with_temp_home(|| {
            supports_keychain_for_tests(Some(false));
            write_config(WriteConfigInput {
                username: Some("alice".to_string()),
                shared_key: Some(1u32.into()),
                port: Some(10_012),
                host: Some("127.0.0.1".to_string()),
                allow_empty: false,
                refresh_created_at: true,
                runtime_mode: Some(RuntimeMode::Auto),
                last_launch_status: Some("failed".to_string()),
                last_launch_error: Some("probe failed".to_string()),
                last_launch_strategy: Some("archived".to_string()),
                ..WriteConfigInput::default()
            })
            .unwrap();

            let written = write_config(WriteConfigInput {
                port: Some(10_013),
                host: Some("127.0.0.1".to_string()),
                allow_empty: true,
                runtime_mode: Some(RuntimeMode::Auto),
                bridge_status: Some("attached".to_string()),
                bridge_browser: Some("chrome".to_string()),
                bridge_connected_at: Some("2026-03-08T00:00:00Z".to_string()),
                reset_launch_metadata: true,
                reset_bridge_metadata: true,
                ..WriteConfigInput::default()
            })
            .unwrap();

            assert_eq!(written.runtime_mode, RuntimeMode::Auto);
            assert_eq!(written.username, "alice");
            assert_eq!(written.bridge_status.as_deref(), Some("attached"));
            assert_eq!(written.bridge_browser.as_deref(), Some("chrome"));
            assert_eq!(
                written.bridge_connected_at.as_deref(),
                Some("2026-03-08T00:00:00Z")
            );
            assert!(written.last_launch_status.is_none());
            assert!(written.last_launch_error.is_none());
            assert!(written.last_launch_strategy.is_none());

            let runtime = read_config(Some(ConfigReadOptions {
                require_auth: false,
                max_age_ms: SESSION_MAX_AGE_MS,
                ignore_expiry: false,
            }))
            .unwrap();
            assert_eq!(runtime.username, "alice");
            assert!(!runtime.shared_key.is_zero());
            assert_eq!(runtime.bridge_status.as_deref(), Some("attached"));

            supports_keychain_for_tests(None);
        });
    }
}
