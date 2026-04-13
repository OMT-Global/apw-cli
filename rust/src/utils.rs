use crate::error::{APWError, Result};
use crate::secrets::{delete_shared_key, read_shared_key, supports_keychain, write_shared_key};
use crate::types::{
    normalize_status, APWConfig, APWConfigV1, APWRuntimeConfig, RuntimeMode, SecretSource,
    DEFAULT_HOST, DEFAULT_PORT,
};
use base64::{engine::general_purpose, Engine as _};
use chrono::{TimeZone, Utc};
use num_bigint::BigUint;
use num_traits::{One, Zero};
use rand::RngCore;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub const SESSION_MAX_AGE_MS: u64 = 30 * 24 * 60 * 60 * 1000;

const CONFIG_DIRECTORY_MODE: u32 = 0o700;
const CONFIG_FILE_MODE: u32 = 0o600;
const CONFIG_SCHEMA: i32 = 1;
const MAX_CONFIG_SIZE_BYTES: usize = 10 * 1024;

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

fn config_root() -> PathBuf {
    let home = env::var("HOME")
        .unwrap_or_else(|_| env::var("USERPROFILE").unwrap_or_else(|_| ".".to_string()));
    Path::new(&home).join(".apw")
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

fn read_config_file_or_null() -> Result<APWConfigV1> {
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
        return Ok(v1);
    }

    let legacy = serde_json::from_value::<APWConfig>(parsed).map_err(|_| {
        APWError::new(
            crate::types::Status::InvalidConfig,
            "Invalid config format. Run `apw auth` again.",
        )
    })?;

    Ok(normalize_legacy_config(legacy))
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
        fallback_provider: None,
        fallback_provider_path: None,
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

fn read_config_file() -> Result<APWConfigV1> {
    read_config_file_or_null()
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
        fallback_provider: None,
        fallback_provider_path: None,
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
            created_at: Utc.timestamp_nanos(0).to_rfc3339(),
        });
    }

    if stale_config(&raw.created_at, options.max_age_ms) && !options.ignore_expiry {
        clear_config();
        if options.require_auth {
            return Err(APWError::new(
                crate::types::Status::InvalidSession,
                "Session expired. Run `apw auth` again.",
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
            "No active session. Run `apw auth` again.",
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

    let existing = read_config_file().ok();
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
            "Cannot persist incomplete config. Run `apw auth` again.",
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
                "Cannot persist incomplete config. Run `apw auth` again.",
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
        fallback_provider: existing.as_ref().and_then(|value| value.fallback_provider),
        fallback_provider_path: existing
            .as_ref()
            .and_then(|value| value.fallback_provider_path.clone()),
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
                fallback_provider: None,
                fallback_provider_path: None,
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
                runtime_mode: Some(RuntimeMode::Direct),
                last_launch_status: Some("ok".to_string()),
                last_launch_error: None,
                last_launch_strategy: Some("direct".to_string()),
                ..WriteConfigInput::default()
            })
            .unwrap();

            assert_eq!(written.port, 10_045);
            assert_eq!(written.host, "127.0.0.1");
            assert_eq!(written.username, "");
            assert_eq!(written.shared_key, "");
            assert_eq!(written.runtime_mode, RuntimeMode::Direct);
            assert_eq!(written.last_launch_status.as_deref(), Some("ok"));
            assert_eq!(written.last_launch_error, None);
            assert_eq!(written.last_launch_strategy.as_deref(), Some("direct"));

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
                runtime_mode: Some(RuntimeMode::Direct),
                last_launch_status: Some("failed".to_string()),
                last_launch_error: Some("probe failed".to_string()),
                last_launch_strategy: Some("direct".to_string()),
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
                runtime_mode: Some(RuntimeMode::Direct),
                last_launch_status: Some("failed".to_string()),
                last_launch_error: Some("probe failed".to_string()),
                last_launch_strategy: Some("direct".to_string()),
                ..WriteConfigInput::default()
            })
            .unwrap();

            assert_eq!(preserved.created_at, written.created_at);
            supports_keychain_for_tests(None);
        });
    }

    #[test]
    #[serial]
    fn browser_bridge_metadata_resets_launch_fields_without_clearing_auth() {
        with_temp_home(|| {
            supports_keychain_for_tests(Some(false));
            write_config(WriteConfigInput {
                username: Some("alice".to_string()),
                shared_key: Some(1u32.into()),
                port: Some(10_012),
                host: Some("127.0.0.1".to_string()),
                allow_empty: false,
                refresh_created_at: true,
                runtime_mode: Some(RuntimeMode::Direct),
                last_launch_status: Some("failed".to_string()),
                last_launch_error: Some("probe failed".to_string()),
                last_launch_strategy: Some("direct".to_string()),
                ..WriteConfigInput::default()
            })
            .unwrap();

            let written = write_config(WriteConfigInput {
                port: Some(10_013),
                host: Some("127.0.0.1".to_string()),
                allow_empty: true,
                runtime_mode: Some(RuntimeMode::Browser),
                bridge_status: Some("attached".to_string()),
                bridge_browser: Some("chrome".to_string()),
                bridge_connected_at: Some("2026-03-08T00:00:00Z".to_string()),
                reset_launch_metadata: true,
                reset_bridge_metadata: true,
                ..WriteConfigInput::default()
            })
            .unwrap();

            assert_eq!(written.runtime_mode, RuntimeMode::Browser);
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
