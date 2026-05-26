#![allow(dead_code)]

use crate::error::{APWError, Result};
use crate::types::Status;
use std::process::Command;
use std::sync::{Mutex, OnceLock};

const KEYCHAIN_SERVICE: &str = "dev.benjaminedwards.apw.sharedKey";

type CommandRunner = dyn Fn(&[&str]) -> Result<SecurityResult> + Send + Sync;

#[derive(Debug, Clone)]
pub(crate) struct SecurityResult {
    code: i32,
    stdout: String,
    stderr: String,
}

fn is_not_found(result: &SecurityResult) -> bool {
    if result.code == 0 {
        return false;
    }

    let lowered = result.stderr.to_ascii_lowercase();
    lowered.contains("not found")
        || lowered.contains("no such item")
        || lowered.contains("could not be found")
}

fn default_security_runner(args: &[&str]) -> Result<SecurityResult> {
    let output = Command::new("security")
        .args(args)
        .output()
        .map_err(|error| {
            APWError::new(
                Status::GenericError,
                format!("Failed to execute security command: {error}"),
            )
        })?;

    Ok(SecurityResult {
        code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).trim().to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
    })
}

static COMMAND_RUNNER: OnceLock<Mutex<Box<CommandRunner>>> = OnceLock::new();
static KEYCHAIN_OVERRIDE: OnceLock<Mutex<Option<bool>>> = OnceLock::new();

fn command_runner() -> &'static Mutex<Box<CommandRunner>> {
    COMMAND_RUNNER.get_or_init(|| Mutex::new(Box::new(default_security_runner)))
}

fn keychain_override() -> &'static Mutex<Option<bool>> {
    KEYCHAIN_OVERRIDE.get_or_init(|| Mutex::new(None))
}

fn with_runner<T>(run: impl FnOnce(&mut Box<CommandRunner>) -> T) -> T {
    let mut guard = command_runner()
        .lock()
        .expect("security command runner lock");
    run(&mut guard)
}

pub fn supports_keychain() -> bool {
    let override_value = *keychain_override().lock().expect("keychain override lock");
    if let Some(value) = override_value {
        return value;
    }
    cfg!(target_os = "macos")
}

#[cfg(test)]
pub fn supports_keychain_for_tests(value: Option<bool>) {
    let mut guard = keychain_override().lock().expect("keychain override lock");
    *guard = value;
}

#[cfg(test)]
pub fn set_security_command_runner_for_tests<R>(runner: R)
where
    R: Fn(&[&str]) -> Result<SecurityResult> + Send + Sync + 'static,
{
    with_runner(|current| *current = Box::new(runner));
}

#[cfg(test)]
pub(crate) fn make_security_result(code: i32, stdout: &str, stderr: &str) -> SecurityResult {
    SecurityResult {
        code,
        stdout: stdout.to_string(),
        stderr: stderr.to_string(),
    }
}

#[cfg(test)]
pub fn reset_security_command_runner_for_tests() {
    with_runner(|current| *current = Box::new(default_security_runner));
}

pub fn read_shared_key(username: &str) -> Result<Option<String>> {
    if username.is_empty() {
        return Ok(None);
    }
    if !supports_keychain() {
        return Err(APWError::new(
            Status::ProcessNotRunning,
            "Keychain storage is only available on macOS.",
        ));
    }

    let result = {
        with_runner(|runner| {
            runner(&[
                "find-generic-password",
                "-a",
                username,
                "-s",
                KEYCHAIN_SERVICE,
                "-w",
            ])
        })?
    };

    if result.code == 0 {
        return Ok(Some(result.stdout));
    }
    if is_not_found(&result) {
        return Ok(None);
    }
    if result.stderr.is_empty() {
        return Err(APWError::new(
            Status::InvalidConfig,
            "Keychain lookup failed.",
        ));
    }
    Err(APWError::new(Status::InvalidConfig, result.stderr))
}

pub fn write_shared_key(username: &str, shared_key: &str) -> Result<()> {
    if username.is_empty() {
        return Err(APWError::new(
            Status::InvalidConfig,
            "Invalid session username.",
        ));
    }
    if shared_key.is_empty() {
        return Err(APWError::new(
            Status::InvalidConfig,
            "Invalid shared key value.",
        ));
    }
    if !supports_keychain() {
        return Err(APWError::new(
            Status::ProcessNotRunning,
            "Keychain storage is only available on macOS.",
        ));
    }

    let result = {
        with_runner(|runner| {
            runner(&[
                "add-generic-password",
                "-a",
                username,
                "-s",
                KEYCHAIN_SERVICE,
                "-w",
                shared_key,
                "-U",
            ])
        })?
    };
    if result.code == 0 {
        return Ok(());
    }
    Err(APWError::new(
        Status::InvalidConfig,
        if result.stderr.is_empty() {
            "Failed to store secret."
        } else {
            &result.stderr
        },
    ))
}

pub fn delete_shared_key(username: &str) -> Result<()> {
    if username.is_empty() {
        return Ok(());
    }
    if !supports_keychain() {
        return Err(APWError::new(
            Status::ProcessNotRunning,
            "Keychain storage is only available on macOS.",
        ));
    }

    let result = {
        with_runner(|runner| {
            runner(&[
                "delete-generic-password",
                "-a",
                username,
                "-s",
                KEYCHAIN_SERVICE,
            ])
        })?
    };

    if result.code == 0 || is_not_found(&result) {
        return Ok(());
    }
    if result.stderr.is_empty() {
        return Err(APWError::new(
            Status::InvalidConfig,
            "Failed to delete secret.",
        ));
    }
    Err(APWError::new(Status::InvalidConfig, result.stderr))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::sync::{Arc, Mutex};

    #[test]
    #[serial]
    fn write_and_delete_fail_without_keychain_support() {
        supports_keychain_for_tests(Some(false));

        let write_error = write_shared_key("alice", "secret").unwrap_err();
        assert_eq!(write_error.code, Status::ProcessNotRunning);

        let delete_error = delete_shared_key("alice").unwrap_err();
        assert_eq!(delete_error.code, Status::ProcessNotRunning);

        supports_keychain_for_tests(None);
    }

    #[test]
    #[serial]
    fn keychain_not_found_is_tolerant() {
        supports_keychain_for_tests(Some(true));
        set_security_command_runner_for_tests(|args| {
            if args.len() == 6 {
                Ok(SecurityResult {
                    code: 44,
                    stdout: String::new(),
                    stderr: "item: no such item found".to_string(),
                })
            } else {
                Ok(SecurityResult {
                    code: 0,
                    stdout: String::new(),
                    stderr: String::new(),
                })
            }
        });

        let read_result = read_shared_key("alice").unwrap();
        assert_eq!(read_result, None);

        supports_keychain_for_tests(None);
        reset_security_command_runner_for_tests();
    }

    #[test]
    #[serial]
    fn keychain_errors_are_reported() {
        supports_keychain_for_tests(Some(true));
        set_security_command_runner_for_tests(|_| {
            Ok(SecurityResult {
                code: 1,
                stdout: String::new(),
                stderr: "boom".to_string(),
            })
        });

        let result = read_shared_key("alice").unwrap_err();
        assert_eq!(result.code, Status::InvalidConfig);
        assert_eq!(result.message, "boom");

        supports_keychain_for_tests(None);
        reset_security_command_runner_for_tests();
    }

    #[test]
    #[serial]
    fn security_runner_receives_unescaped_arguments() {
        supports_keychain_for_tests(Some(true));
        let args_seen = Arc::new(Mutex::new(Vec::<Vec<String>>::new()));

        let write_args = args_seen.clone();
        set_security_command_runner_for_tests(move |args| {
            let mut collected = write_args.lock().expect("security args lock");
            collected.push(args.iter().map(|value| value.to_string()).collect());
            Ok(SecurityResult {
                code: 0,
                stdout: String::new(),
                stderr: String::new(),
            })
        });

        let username = "alice'; echo pwned #".to_string();
        let key = "value with spaces".to_string();
        write_shared_key(&username, &key).unwrap();
        delete_shared_key(&username).unwrap();

        let captured = args_seen.lock().expect("security args lock");
        assert_eq!(captured.len(), 2);

        let write_command = captured
            .iter()
            .find(|entry| entry.first() == Some(&"add-generic-password".to_string()))
            .expect("write invocation recorded");
        assert!(write_command.contains(&"-a".to_string()));
        assert!(write_command.contains(&username));
        assert!(write_command.contains(&key));

        let delete_command = captured
            .iter()
            .find(|entry| entry.first() == Some(&"delete-generic-password".to_string()))
            .expect("delete invocation recorded");
        assert!(delete_command.contains(&"-a".to_string()));
        assert!(delete_command.contains(&username));
        assert!(!delete_command.contains(&key));

        supports_keychain_for_tests(None);
        reset_security_command_runner_for_tests();
    }
}
