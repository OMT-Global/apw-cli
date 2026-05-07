use base64::engine::general_purpose;
use base64::Engine as _;
use chrono::{Duration, Utc};
use openssl::symm::{Cipher, Crypter, Mode};
use rand::RngCore;
use serde_json::Value;
use serial_test::serial;
use std::env;
use std::fs;
use std::net::UdpSocket;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::thread;
use std::time::Duration as StdDuration;
use tempfile::TempDir;

const MAX_MESSAGE_BYTES: usize = 16 * 1024;
const DEFAULT_SHARED_KEY_BYTES: [u8; 16] = [0x10; 16];

#[derive(Debug)]
struct CommandOutput {
    status: i32,
    stdout: String,
    stderr: String,
}

fn has_deno() -> bool {
    Command::new("deno")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn run_command(program: &Path, args: &[&str], home: &Path) -> CommandOutput {
    let mut command = Command::new(program);
    command.env("HOME", home).args(args).env("NO_COLOR", "1");

    let output: Output = command.output().expect("failed to run command");

    CommandOutput {
        status: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).trim().to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
    }
}

fn run_rust_cli(home: &Path, args: &[&str]) -> CommandOutput {
    let path = PathBuf::from(env!("CARGO_BIN_EXE_apw"));
    run_command(&path, args, home)
}

fn run_deno_cli(home: &Path, args: &[&str]) -> CommandOutput {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let cli = workspace.join("../legacy/deno/src/cli.ts");
    let mut cmd = Command::new("deno");
    let output = cmd
        .current_dir(&workspace)
        .env("HOME", home)
        .env("NO_COLOR", "1")
        .arg("run")
        .arg("--quiet")
        .arg("--allow-read")
        .arg("--allow-write")
        .arg("--allow-env")
        .arg("--allow-net")
        .arg(cli)
        .args(args)
        .output()
        .expect("failed to run legacy deno cli");

    CommandOutput {
        status: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).trim().to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
    }
}

fn write_session_config(home: &Path, port: u16, shared_key: &[u8], authenticated: bool) -> Value {
    let created_at = if authenticated {
        Utc::now().to_rfc3339()
    } else {
        (Utc::now() - Duration::days(1)).to_rfc3339()
    };

    let config = serde_json::json!({
        "schema": 1,
        "port": port,
        "host": "127.0.0.1",
        "username": "alice",
        "sharedKey": general_purpose::STANDARD.encode(shared_key),
        "createdAt": created_at,
        "secretSource": "file",
    });

    fs::create_dir_all(home.join(".apw")).expect("failed to create config dir");
    fs::write(
        home.join(".apw/config.json"),
        serde_json::to_string(&config).expect("failed to serialize config"),
    )
    .expect("failed to write config");

    config
}

fn encrypt_payload(shared_key: &[u8], payload: &Value) -> String {
    let key = shared_key;
    assert!(key.len() >= 16);
    let nonce_seed = {
        let mut bytes = [0_u8; 16];
        let mut random = rand::thread_rng();
        random.fill_bytes(&mut bytes);
        bytes
    };
    let mut cipher = Crypter::new(
        Cipher::aes_128_gcm(),
        Mode::Encrypt,
        &key[..16],
        Some(&nonce_seed),
    )
    .expect("valid aes key slice");
    let plain = serde_json::to_vec(payload).expect("failed to serialize payload");
    let mut encrypted = vec![0_u8; plain.len() + 16];
    let count = cipher
        .update(&plain, &mut encrypted)
        .expect("payload encryption failed");
    let finalize_count = cipher
        .finalize(&mut encrypted[count..])
        .expect("payload encryption failed");
    encrypted.truncate(count + finalize_count);
    let mut tag = [0_u8; 16];
    cipher.get_tag(&mut tag).expect("payload encryption failed");
    encrypted.extend_from_slice(&tag);

    let mut output = nonce_seed.to_vec();
    output.extend_from_slice(&encrypted);
    general_purpose::STANDARD.encode(output)
}

fn spawn_fake_daemon<F>(max_messages: usize, handler: F) -> (u16, thread::JoinHandle<()>)
where
    F: Fn(&Value, usize) -> Vec<u8> + Send + 'static,
{
    let socket = UdpSocket::bind("127.0.0.1:0").expect("failed to bind daemon socket");
    let port = socket
        .local_addr()
        .expect("failed to query daemon socket")
        .port();
    socket
        .set_read_timeout(Some(StdDuration::from_millis(60_000)))
        .expect("failed to set socket timeout");

    let join = thread::spawn(move || {
        let mut step = 0usize;
        let mut buffer = vec![0_u8; MAX_MESSAGE_BYTES];

        while step < max_messages {
            let (size, peer) = match socket.recv_from(&mut buffer) {
                Ok((size, peer)) => (size, peer),
                Err(error) if error.kind() == std::io::ErrorKind::TimedOut => break,
                Err(_) => break,
            };

            let request = serde_json::from_slice::<Value>(&buffer[..size]).unwrap_or(Value::Null);
            let response = handler(&request, step);
            let _ = socket.send_to(&response, peer);
            step = step.saturating_add(1);
        }
    });

    (port, join)
}

fn parse_json_output(output: &CommandOutput) -> Value {
    serde_json::from_str(&output.stdout)
        .unwrap_or_else(|_| panic!("command stdout was not JSON: {:?}", output.stdout))
}

fn parse_json_from_output(output: &CommandOutput) -> Value {
    let source = if output.stdout.trim().is_empty() {
        &output.stderr
    } else {
        &output.stdout
    };
    serde_json::from_str(source)
        .unwrap_or_else(|_| panic!("command output was not JSON: {:?}", source))
}

fn run_with_temp_home<F, R>(run: F) -> R
where
    F: FnOnce(&Path) -> R,
{
    let temp = TempDir::new().expect("failed to create temp home");
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

#[derive(Clone)]
struct CommandCase {
    name: &'static str,
    rust_args: &'static [&'static str],
    deno_args: &'static [&'static str],
    require_session: bool,
    expect_code: i32,
}

#[test]
#[serial]
fn parity_status_output_with_no_session_is_shape_compatible() {
    if !has_deno() {
        return;
    }

    run_with_temp_home(|home| {
        let rust = run_rust_cli(home, &["status", "--json"]);
        let deno = run_deno_cli(home, &["status", "--json"]);

        assert_eq!(rust.status, 0, "rust auth request failed: {rust:#?}");
        assert_eq!(deno.status, 0, "deno auth request failed: {deno:#?}");

        let rust_payload = parse_json_output(&rust);
        let deno_payload = parse_json_output(&deno);

        assert_eq!(rust_payload["ok"], deno_payload["ok"]);
        assert_eq!(rust_payload["code"], deno_payload["code"]);
        assert_eq!(
            rust_payload["payload"]["daemon"]["host"],
            deno_payload["payload"]["daemon"]["host"]
        );
        assert_eq!(rust_payload["payload"]["session"]["authenticated"], false);
        assert_eq!(deno_payload["payload"]["session"]["authenticated"], false);
    });
}

#[test]
fn legacy_daemon_commands_emit_deprecation_warning() {
    run_with_temp_home(|home| {
        for args in [
            &["auth", "logout"][..],
            &["pw", "list", "bad host"][..],
            &["otp", "list", "bad host"][..],
            &["start", "--dry-run"][..],
        ] {
            let output = run_rust_cli(home, args);
            assert!(
                output.stderr.contains("legacy daemon path"),
                "{args:?} did not emit deprecation warning: {:?}",
                output.stderr
            );
            assert!(
                output.stderr.contains("v2.1.0"),
                "{args:?} did not include removal milestone: {:?}",
                output.stderr
            );
        }
    });
}

#[test]
#[serial]
fn parity_auth_request_shape_matches_legacy() {
    if !has_deno() {
        return;
    }

    let (port, handle) = spawn_fake_daemon(2, |_request, _step| {
        let msg = _request
            .get("msg")
            .and_then(|value| value.get("PAKE"))
            .and_then(Value::as_str);
        let raw = msg
            .and_then(|candidate| general_purpose::STANDARD.decode(candidate).ok())
            .and_then(|payload| serde_json::from_slice::<Value>(&payload).ok());
        let tid = raw
            .as_ref()
            .and_then(|value| value.get("TID"))
            .and_then(Value::as_str)
            .unwrap_or("alice");

        let response = serde_json::json!({
            "TID": tid,
            "MSG": 1,
            "A": "AQ==",
            "s": "AQ==",
            "B": "AQ==",
            "PROTO": [1],
            "VER": "1.0",
            "ErrCode": 0,
        });

        serde_json::to_vec(&serde_json::json!({
            "ok": true,
            "code": 0,
            "payload": {
                "PAKE": general_purpose::STANDARD.encode(serde_json::to_vec(&response).unwrap())
            },
        }))
        .expect("failed to encode auth response")
    });

    run_with_temp_home(|home| {
        let _ = write_session_config(home, port, DEFAULT_SHARED_KEY_BYTES.as_slice(), true);
        let rust = run_rust_cli(home, &["--json", "auth", "request"]);
        let deno = run_deno_cli(home, &["--json", "auth", "request"]);

        assert_eq!(rust.status, 0);
        assert_eq!(deno.status, 0);
        let rust_payload = parse_json_output(&rust);
        let deno_payload = parse_json_output(&deno);

        assert_eq!(rust_payload["code"], deno_payload["code"]);
        assert!(rust_payload["payload"]["salt"].is_string());
        assert!(rust_payload["payload"]["serverKey"].is_string());
        assert!(rust_payload["payload"]["clientKey"].is_string());
        assert!(rust_payload["payload"]["username"].is_string());
    });

    handle.join().expect("daemon failed");
}

#[test]
#[serial]
fn parity_auth_response_pin_mismatch_maps_to_invalid_session() {
    if !has_deno() {
        return;
    }

    let (port, handle) = spawn_fake_daemon(2, |request, _step| {
        let msg = request
            .get("msg")
            .and_then(|value| value.get("PAKE"))
            .and_then(Value::as_str);
        let raw = msg
            .and_then(|candidate| general_purpose::STANDARD.decode(candidate).ok())
            .and_then(|payload| serde_json::from_slice::<Value>(&payload).ok());
        let tid = raw
            .as_ref()
            .and_then(|value| value.get("TID"))
            .and_then(Value::as_str)
            .unwrap_or("alice");

        let response = serde_json::json!({
            "TID": tid,
            "MSG": 3,
            "A": "AQ==",
            "s": "AQ==",
            "B": "AQ==",
            "PROTO": [1],
            "VER": "1.0",
            "ErrCode": 1,
        });

        serde_json::to_vec(&serde_json::json!({
            "ok": true,
            "code": 0,
            "payload": {
                "PAKE": general_purpose::STANDARD.encode(serde_json::to_vec(&response).unwrap())
            },
        }))
        .expect("failed to encode pin mismatch response")
    });

    run_with_temp_home(|home| {
        let _ = write_session_config(home, port, DEFAULT_SHARED_KEY_BYTES.as_slice(), true);
        let rust = run_rust_cli(
            home,
            &[
                "--json",
                "auth",
                "response",
                "--pin",
                "123456",
                "--salt",
                "AQ==",
                "--server_key",
                "AQ==",
                "--client_key",
                "AQ==",
                "--username",
                "alice",
            ],
        );
        let deno = run_deno_cli(
            home,
            &[
                "--json",
                "auth",
                "response",
                "--pin",
                "123456",
                "--salt",
                "AQ==",
                "--serverKey",
                "AQ==",
                "--clientKey",
                "AQ==",
                "--username",
                "alice",
            ],
        );

        assert_eq!(rust.status, 9, "rust auth response failed: {rust:#?}");
        assert_eq!(deno.status, 9);
        assert!(rust.stderr.contains("Incorrect"));
        assert!(deno.stderr.contains("Incorrect"));
    });

    handle.join().expect("daemon failed");
}

#[test]
#[serial]
fn parity_data_plane_queries_match_legacy() {
    if !has_deno() {
        return;
    }

    let shared_key = DEFAULT_SHARED_KEY_BYTES.to_vec();
    let (port, handle) = spawn_fake_daemon(16, move |request, _step| {
        let command = request.get("cmd").and_then(Value::as_i64).unwrap_or(-1);
        let response_payload = match command {
            4 => serde_json::json!({
                "STATUS": 0,
                "Entries": [{
                    "USR": "alice",
                    "sites": ["https://example.com/"],
                    "PWD": "secret",
                }],
            }),
            5 => serde_json::json!({
                "STATUS": 0,
                "Entries": [{
                    "USR": "alice",
                    "sites": ["https://example.com/"],
                    "PWD": "hunter2",
                }],
            }),
            15 | 16 => serde_json::json!({
                "STATUS": 0,
                "Entries": [{
                    "code": "111111",
                    "username": "alice",
                    "source": "totp",
                    "domain": "example.com",
                }],
            }),
            14 => {
                return serde_json::to_vec(&serde_json::json!({
                    "ok": true,
                    "code": 0,
                    "payload": {
                        "canFillOneTimeCodes": true,
                        "scanForOTPURI": false,
                    },
                }))
                .expect("failed to encode capabilities response")
            }
            _ => serde_json::json!({
                "STATUS": 3,
                "Entries": [],
            }),
        };

        let encrypted = encrypt_payload(&shared_key, &response_payload);
        serde_json::to_vec(&serde_json::json!({
            "ok": true,
            "code": 0,
            "payload": {
                "SMSG": {
                    "TID": "alice",
                    "SDATA": encrypted,
                },
            },
        }))
        .expect("failed to encode response")
    });

    run_with_temp_home(|home| {
        let _ = write_session_config(home, port, DEFAULT_SHARED_KEY_BYTES.as_slice(), true);

        let rust_pw = run_rust_cli(home, &["--json", "pw", "get", "example.com", "alice"]);
        let deno_pw = run_deno_cli(home, &["--json", "pw", "get", "example.com", "alice"]);
        let rust_otp = run_rust_cli(home, &["--json", "otp", "list", "example.com"]);
        let deno_otp = run_deno_cli(home, &["--json", "otp", "list", "example.com"]);

        assert_eq!(rust_pw.status, 0, "rust pw failed: {rust_pw:#?}");
        assert_eq!(deno_pw.status, 0, "deno pw failed: {deno_pw:#?}");
        assert_eq!(rust_otp.status, 0, "rust otp failed: {rust_otp:#?}");
        assert_eq!(deno_otp.status, 0, "deno otp failed: {deno_otp:#?}");

        let rust_pw_payload = parse_json_output(&rust_pw);
        let deno_pw_payload = parse_json_output(&deno_pw);
        let rust_otp_payload = parse_json_output(&rust_otp);
        let deno_otp_payload = parse_json_output(&deno_otp);

        assert_eq!(rust_pw_payload["payload"], deno_pw_payload["payload"]);
        assert_eq!(rust_otp_payload["payload"], deno_otp_payload["payload"]);
    });

    handle.join().expect("daemon failed");
}

#[test]
#[serial]
fn parity_command_matrix_matches_legacy() {
    if !has_deno() {
        return;
    }

    let shared_key = DEFAULT_SHARED_KEY_BYTES.to_vec();
    let (port, handle) = spawn_fake_daemon(16, move |request, _step| {
        let command = request.get("cmd").and_then(Value::as_i64).unwrap_or(-1);
        if command == 2 {
            let message = request
                .get("msg")
                .and_then(|value| value.get("PAKE"))
                .and_then(Value::as_str)
                .and_then(|candidate| general_purpose::STANDARD.decode(candidate).ok())
                .and_then(|payload| serde_json::from_slice::<Value>(&payload).ok());
            let request_msg = message
                .as_ref()
                .and_then(|value| value.get("MSG"))
                .and_then(Value::as_i64)
                .unwrap_or_default();

            let response_payload = if request_msg == 2 {
                serde_json::json!({
                "TID": "alice",
                "MSG": 3,
                "A": "AQ==",
                "s": "AQ==",
                "B": "AQ==",
                "PROTO": [1],
                "VER": "1.0",
                "ErrCode": 1,
                "HAMK": "AQ==",
                })
            } else {
                serde_json::json!({
                "TID": "alice",
                "MSG": 1,
                "A": "AQ==",
                "s": "AQ==",
                "B": "AQ==",
                "PROTO": [1],
                "VER": "1.0",
                "ErrCode": 0,
                })
            };

            return serde_json::to_vec(&serde_json::json!({
                "ok": true,
                "code": 0,
                "payload": {
                    "PAKE": general_purpose::STANDARD.encode(serde_json::to_vec(&response_payload).unwrap()),
                },
            }))
            .expect("failed to encode auth response");
        }

        let response_payload = match command {
            4 => serde_json::json!({
                "STATUS": 0,
                "Entries": [{
                    "USR": "alice",
                    "sites": ["https://example.com/"],
                    "PWD": "secret",
                }],
            }),
            5 => serde_json::json!({
                "STATUS": 0,
                "Entries": [{
                    "USR": "alice",
                    "sites": ["https://example.com/"],
                    "PWD": "hunter2",
                }],
            }),
            16 => serde_json::json!({
                "STATUS": 0,
                "Entries": [{
                    "code": "111111",
                    "username": "alice",
                    "source": "totp",
                    "domain": "example.com",
                }],
            }),
            17 => serde_json::json!({
                "STATUS": 0,
                "Entries": [{
                    "code": "222222",
                    "username": "alice",
                    "source": "totp",
                    "domain": "example.com",
                }],
            }),
            14 => {
                return serde_json::to_vec(&serde_json::json!({
                    "ok": true,
                    "code": 0,
                    "payload": {
                        "canFillOneTimeCodes": true,
                        "scanForOTPURI": false,
                    },
                }))
                .expect("failed to encode capabilities response")
            }
            _ => serde_json::json!({
                "STATUS": 3,
                "Entries": [],
            }),
        };

        let encrypted = encrypt_payload(&shared_key, &response_payload);
        serde_json::to_vec(&serde_json::json!({
            "ok": true,
            "code": 0,
            "payload": {
                "SMSG": {
                    "TID": "alice",
                    "SDATA": encrypted,
                },
            },
        }))
        .expect("failed to encode response")
    });

    let cases: &[CommandCase] = &[
        CommandCase {
            name: "status-without-session",
            rust_args: &["--json", "status"],
            deno_args: &["--json", "status"],
            require_session: false,
            expect_code: 0,
        },
        CommandCase {
            name: "auth-request",
            rust_args: &["--json", "auth", "request"],
            deno_args: &["--json", "auth", "request"],
            require_session: true,
            expect_code: 0,
        },
        CommandCase {
            name: "auth-response-pin-mismatch",
            rust_args: &[
                "--json",
                "auth",
                "response",
                "--pin",
                "123456",
                "--salt",
                "AQ==",
                "--server_key",
                "AQ==",
                "--client_key",
                "AQ==",
                "--username",
                "alice",
            ],
            deno_args: &[
                "--json",
                "auth",
                "response",
                "--pin",
                "123456",
                "--salt",
                "AQ==",
                "--serverKey",
                "AQ==",
                "--clientKey",
                "AQ==",
                "--username",
                "alice",
            ],
            require_session: true,
            expect_code: 9,
        },
        CommandCase {
            name: "auth-response-pin-mismatch-short-flags",
            rust_args: &[
                "--json",
                "auth",
                "response",
                "-p",
                "123456",
                "-s",
                "AQ==",
                "--serverKey",
                "AQ==",
                "--clientKey",
                "AQ==",
                "-u",
                "alice",
            ],
            deno_args: &[
                "--json",
                "auth",
                "response",
                "--pin",
                "123456",
                "--salt",
                "AQ==",
                "--serverKey",
                "AQ==",
                "--clientKey",
                "AQ==",
                "--username",
                "alice",
            ],
            require_session: true,
            expect_code: 9,
        },
        CommandCase {
            name: "pw-list",
            rust_args: &["--json", "pw", "list", "example.com"],
            deno_args: &["--json", "pw", "list", "example.com"],
            require_session: true,
            expect_code: 0,
        },
        CommandCase {
            name: "pw-get",
            rust_args: &["--json", "pw", "get", "example.com", "alice"],
            deno_args: &["--json", "pw", "get", "example.com", "alice"],
            require_session: true,
            expect_code: 0,
        },
        CommandCase {
            name: "otp-list",
            rust_args: &["--json", "otp", "list", "example.com"],
            deno_args: &["--json", "otp", "list", "example.com"],
            require_session: true,
            expect_code: 0,
        },
        CommandCase {
            name: "otp-get",
            rust_args: &["--json", "otp", "get", "example.com"],
            deno_args: &["--json", "otp", "get", "example.com"],
            require_session: true,
            expect_code: 0,
        },
        CommandCase {
            name: "invalid-url-blocked-before-daemon",
            rust_args: &["--json", "pw", "list", "bad host"],
            deno_args: &["--json", "pw", "list", "bad host"],
            require_session: false,
            expect_code: 1,
        },
        CommandCase {
            name: "auth-logout",
            rust_args: &["--json", "auth", "logout"],
            deno_args: &["--json", "auth", "logout"],
            require_session: true,
            expect_code: 0,
        },
    ];

    run_with_temp_home(|home| {
        let mut session_configured = false;

        for case in cases {
            if case.require_session && !session_configured {
                let _ = write_session_config(home, port, DEFAULT_SHARED_KEY_BYTES.as_slice(), true);
                session_configured = true;
            }

            let rust = run_rust_cli(home, case.rust_args);
            let deno = run_deno_cli(home, case.deno_args);

            assert_eq!(rust.status, deno.status, "{} status mismatch", case.name);
            assert_eq!(rust.status, case.expect_code, "{} code mismatch", case.name);

            let rust_payload = parse_json_from_output(&rust);
            let deno_payload = parse_json_from_output(&deno);

            assert_eq!(
                rust_payload["code"], deno_payload["code"],
                "{} code envelope mismatch",
                case.name
            );
            assert_eq!(
                rust_payload["ok"].as_bool(),
                deno_payload["ok"].as_bool(),
                "{} ok mismatch",
                case.name
            );
            assert_eq!(
                rust_payload["code"], case.expect_code,
                "{} expected code mismatch",
                case.name
            );

            match case.name {
                "status-without-session" => {
                    assert_eq!(rust_payload["payload"]["daemon"]["host"], "127.0.0.1");
                    assert_eq!(
                        rust_payload["payload"]["daemon"]["host"],
                        deno_payload["payload"]["daemon"]["host"]
                    );
                    assert_eq!(rust_payload["payload"]["session"]["authenticated"], false);
                }
                "auth-request" => {
                    assert!(rust_payload["payload"]["salt"].is_string());
                    assert!(rust_payload["payload"]["serverKey"].is_string());
                    assert!(rust_payload["payload"]["clientKey"].is_string());
                    assert!(rust_payload["payload"]["username"].is_string());
                }
                "auth-response-pin-mismatch" => {
                    assert_eq!(rust_payload["code"], deno_payload["code"]);
                    let rust_error = rust_payload["error"].as_str().unwrap_or("").to_string();
                    let deno_error = deno_payload["error"].as_str().unwrap_or("").to_string();
                    assert!(rust_error.contains("Incorrect"), "{}", case.name);
                    assert!(deno_error.contains("Incorrect"), "{}", case.name);
                }
                "auth-response-pin-mismatch-short-flags" => {
                    assert_eq!(rust_payload["code"], deno_payload["code"]);
                    let rust_error = rust_payload["error"].as_str().unwrap_or("").to_string();
                    let deno_error = deno_payload["error"].as_str().unwrap_or("").to_string();
                    assert!(rust_error.contains("Incorrect"), "{}", case.name);
                    assert!(deno_error.contains("Incorrect"), "{}", case.name);
                }
                "pw-list" | "pw-get" | "otp-list" | "otp-get" => {
                    assert_eq!(
                        rust_payload["payload"], deno_payload["payload"],
                        "{} payload mismatch",
                        case.name
                    );
                }
                "invalid-url-blocked-before-daemon" => {
                    let rust_error = rust_payload["error"].as_str().unwrap_or("").to_string();
                    let deno_error = deno_payload["error"].as_str().unwrap_or("").to_string();
                    assert!(rust_error.contains("Invalid URL"), "{}", case.name);
                    assert!(deno_error.contains("Invalid URL"), "{}", case.name);
                }
                "auth-logout" => {
                    assert_eq!(
                        rust_payload["payload"]["status"], deno_payload["payload"]["status"],
                        "{} payload mismatch",
                        case.name
                    );
                    assert_eq!(rust_payload["payload"]["status"], "logged out");
                }
                _ => {}
            }
        }
    });

    handle.join().expect("daemon failed");
}
