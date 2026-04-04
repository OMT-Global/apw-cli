use crate::client::ApplePasswordManager;
use crate::daemon::{start_daemon, DaemonOptions};
use crate::error::APWError;
use crate::host::{native_host_doctor, native_host_install, native_host_uninstall};
use crate::native_app::{
    native_app_doctor, native_app_install, native_app_launch, native_app_login,
};
use crate::types::{Payload, RuntimeMode, Status};
use crate::utils::{bigint_to_base64, read_bigint};
use clap::{Args, Parser, Subcommand};
use rpassword::prompt_password;
use serde_json::json;
use std::io::{self, Write};

fn read_prompt(prompt: &str) -> Result<String, APWError> {
    print!("{prompt}");
    io::stdout().flush().map_err(|error| {
        APWError::new(
            Status::GenericError,
            format!("Failed to print prompt: {error}"),
        )
    })?;

    let mut value = String::new();
    io::stdin().read_line(&mut value).map_err(|error| {
        APWError::new(
            Status::GenericError,
            format!("Failed to read input: {error}"),
        )
    })?;

    Ok(value.trim().to_string())
}

fn normalize_pin(value: String) -> Result<String, APWError> {
    if !value.chars().all(|c| c.is_ascii_digit()) || value.len() != 6 {
        return Err(APWError::new(
            Status::InvalidParam,
            "PIN must be exactly 6 digits.",
        ));
    }
    Ok(value)
}

fn is_valid_host(host: &str) -> bool {
    !host.trim().is_empty() && !host.contains('\0') && !host.contains(' ')
}

fn parse_host(raw: &str) -> Result<String, APWError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(APWError::new(Status::InvalidParam, "Missing host."));
    }
    if !is_valid_host(trimmed) {
        return Err(APWError::new(Status::InvalidParam, "Invalid host."));
    }
    Ok(trimmed.to_string())
}

fn parse_host_arg(raw: &str) -> std::result::Result<String, String> {
    parse_host(raw).map_err(|error| error.message)
}

fn sanitize_url(raw: &str) -> Result<String, APWError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(APWError::new(
            Status::InvalidParam,
            "Missing or invalid URL.",
        ));
    }
    let candidate = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    };

    let parsed = url::Url::parse(&candidate)
        .map_err(|_| APWError::new(Status::GenericError, format!("Invalid URL: '{candidate}'")))?;
    if parsed.host_str().is_none() {
        return Err(APWError::new(
            Status::GenericError,
            format!("Invalid URL: '{candidate}'"),
        ));
    }

    Ok(candidate)
}

fn print_output(payload: &serde_json::Value, status: Status, json_output: bool) {
    if json_output {
        println!(
            "{}",
            serde_json::json!({
              "ok": status == Status::Success,
              "code": status,
              "payload": payload,
            })
        );
        return;
    }

    match payload {
        serde_json::Value::String(text) => println!("{text}"),
        _ => println!("{}", payload),
    }
}

fn print_entries(payload: &Payload, json_output: bool) -> Result<(), APWError> {
    if payload.status != Status::Success {
        return Err(APWError::new(
            payload.status,
            crate::types::status_text(payload.status),
        ));
    }

    let entries = payload
        .entries
        .iter()
        .filter_map(|entry| {
            if let Some(username) = entry.get("USR").and_then(serde_json::Value::as_str) {
                let domain = entry
                    .get("sites")
                    .and_then(serde_json::Value::as_array)
                    .and_then(|sites| sites.first())
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                let password = entry
                    .get("PWD")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("Not Included");
                return Some(serde_json::json!({
                  "username": username,
                  "domain": domain,
                  "password": password,
                }));
            }

            if let Some(username) = entry.get("username").and_then(serde_json::Value::as_str) {
                let code = entry
                    .get("code")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("Not Included");
                let domain = entry
                    .get("domain")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                return Some(serde_json::json!({
                  "username": username,
                  "domain": domain,
                  "code": code,
                }));
            }

            None
        })
        .collect::<Vec<_>>();

    let mapped = json!({
      "results": entries,
      "status": "ok",
    });
    print_output(&mapped, Status::Success, json_output);
    Ok(())
}

fn print_status(payload: serde_json::Value, json_output: bool) {
    print_output(&payload, Status::Success, json_output);
}

fn parse_pin_prompt(optional: Option<String>) -> Result<String, APWError> {
    if let Some(pin) = optional {
        return normalize_pin(pin);
    }
    normalize_pin(prompt_password("Enter PIN: ").map_err(|error| {
        APWError::new(Status::GenericError, format!("Failed to read PIN: {error}"))
    })?)
}

fn ask_pw_action() -> Result<PwAction, APWError> {
    let selected = read_prompt("Choose action:\n  1) list accounts\n  2) get password\n> ")?;
    let lowered = selected.trim().to_lowercase();
    if lowered == "1" || lowered == "list" || lowered == "list accounts" {
        Ok(PwAction::List { url: String::new() })
    } else if lowered == "2" || lowered == "get" || lowered == "get password" {
        Ok(PwAction::Get {
            url: String::new(),
            username: None,
        })
    } else {
        Err(APWError::new(Status::InvalidParam, "Invalid action."))
    }
}

fn ask_otp_action() -> Result<OtpAction, APWError> {
    let selected = read_prompt("Choose action:\n  1) list OTPs\n  2) get OTP\n> ")?;
    let lowered = selected.trim().to_lowercase();
    if lowered == "1" || lowered == "list" || lowered == "list otps" {
        Ok(OtpAction::List { url: String::new() })
    } else if lowered == "2" || lowered == "get" || lowered == "get otp" {
        Ok(OtpAction::Get { url: String::new() })
    } else {
        Err(APWError::new(Status::InvalidParam, "Invalid action."))
    }
}

#[derive(Parser)]
#[command(name = "apw")]
#[command(version = env!("CARGO_PKG_VERSION"))]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
    #[arg(long = "json", global = true)]
    pub json: bool,
}

#[derive(Subcommand)]
pub enum Commands {
    App(AppCommand),
    Auth(AuthCommand),
    Doctor(DoctorCommand),
    Host(HostCommand),
    Login(LoginCommand),
    Pw(PwCommand),
    Otp(OtpCommand),
    Start(StartCommand),
    Status(StatusCommand),
}

#[derive(Args)]
pub struct AppCommand {
    #[command(subcommand)]
    pub command: AppSubcommand,
}

#[derive(Subcommand)]
pub enum AppSubcommand {
    Install,
    Launch,
}

#[derive(Args, Default)]
pub struct DoctorCommand {}

#[derive(Args)]
pub struct LoginCommand {
    pub url: String,
}

#[derive(Args)]
pub struct AuthCommand {
    #[command(subcommand)]
    pub command: Option<AuthSubcommand>,
    #[arg(short, long)]
    pub pin: Option<String>,
}

#[derive(Subcommand)]
pub enum AuthSubcommand {
    Logout,
    Request,
    Response(AuthResponseArgs),
}

#[derive(Args)]
pub struct AuthResponseArgs {
    #[arg(short, long)]
    pub pin: String,
    #[arg(short, long)]
    pub salt: String,
    #[arg(long = "server_key", alias = "serverKey")]
    pub server_key: String,
    #[arg(long = "client_key", short, alias = "clientKey")]
    pub client_key: String,
    #[arg(short, long)]
    pub username: String,
}

#[derive(Args)]
pub struct HostCommand {
    #[command(subcommand)]
    pub command: HostSubcommand,
}

#[derive(Subcommand)]
pub enum HostSubcommand {
    Install,
    Doctor(HostDoctorArgs),
    Uninstall,
}

#[derive(Args)]
pub struct HostDoctorArgs {
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct PwCommand {
    #[command(subcommand)]
    pub action: Option<PwAction>,
}

#[derive(Subcommand)]
pub enum PwAction {
    Get {
        #[arg(value_name = "url")]
        url: String,
        username: Option<String>,
    },
    List {
        url: String,
    },
}

#[derive(Args)]
pub struct OtpCommand {
    #[command(subcommand)]
    pub action: Option<OtpAction>,
}

#[derive(Subcommand)]
pub enum OtpAction {
    Get { url: String },
    List { url: String },
}

#[derive(Args)]
pub struct StartCommand {
    #[arg(short, long, default_value_t = 0)]
    pub port: u16,
    #[arg(
        short,
        long,
        default_value = "127.0.0.1",
        value_parser = parse_host_arg
    )]
    pub bind: String,
    #[arg(short = 'm', long, default_value = "auto", value_parser = parse_runtime_mode)]
    pub runtime_mode: RuntimeMode,
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Args)]
pub struct StatusCommand {
    #[arg(long)]
    pub json: bool,
}

pub async fn run(mut manager: ApplePasswordManager, cli: Cli) -> Result<(), APWError> {
    match cli.command {
        Commands::App(args) => run_app(args, cli.json),
        Commands::Auth(args) => run_auth(&mut manager, args, cli.json),
        Commands::Doctor(args) => run_doctor(args, cli.json),
        Commands::Host(args) => run_host(args, cli.json),
        Commands::Login(args) => run_login(args, cli.json),
        Commands::Pw(args) => run_pw(&mut manager, args, cli.json),
        Commands::Otp(args) => run_otp(&mut manager, args, cli.json),
        Commands::Start(args) => run_start(args).await,
        Commands::Status(args) => run_status(&mut manager, args, cli.json),
    }
}

fn run_app(args: AppCommand, cli_json: bool) -> Result<(), APWError> {
    let payload = match args.command {
        AppSubcommand::Install => native_app_install()?,
        AppSubcommand::Launch => native_app_launch()?,
    };
    print_output(&payload, Status::Success, cli_json);
    Ok(())
}

fn run_doctor(_args: DoctorCommand, cli_json: bool) -> Result<(), APWError> {
    let payload = native_app_doctor()?;
    print_output(&payload, Status::Success, cli_json);
    Ok(())
}

fn run_login(args: LoginCommand, cli_json: bool) -> Result<(), APWError> {
    let payload = native_app_login(&sanitize_url(&args.url)?)?;
    print_output(&payload, Status::Success, cli_json);
    Ok(())
}

fn run_status(
    manager: &mut ApplePasswordManager,
    args: StatusCommand,
    cli_json: bool,
) -> Result<(), APWError> {
    let payload = manager.status();
    print_status(payload, args.json || cli_json);
    Ok(())
}

fn run_auth(
    manager: &mut ApplePasswordManager,
    args: AuthCommand,
    cli_json: bool,
) -> Result<(), APWError> {
    let result = match args.command {
        Some(AuthSubcommand::Logout) => {
            manager.logout()?;
            serde_json::json!({"status": "logged out"})
        }
        Some(AuthSubcommand::Request) => {
            manager.request_challenge()?;
            let values = manager.session.return_values();
            serde_json::json!({
              "salt": bigint_to_base64(&values.salt.unwrap_or_default()),
              "serverKey": bigint_to_base64(&values.server_public_key.unwrap_or_default()),
              "username": values.username.unwrap_or_default(),
              "clientKey": bigint_to_base64(&values.client_private_key.unwrap_or_default()),
            })
        }
        Some(AuthSubcommand::Response(options)) => {
            let salt = read_bigint(&options.salt)?;
            let server_key = read_bigint(&options.server_key)?;
            let client_key = read_bigint(&options.client_key)?;
            manager.set_session_for_response(options.username, client_key, server_key, salt);
            let pin = normalize_pin(options.pin)?;
            manager.verify_challenge(pin)?;
            serde_json::json!({"status": "ok"})
        }
        None => {
            let pin = parse_pin_prompt(args.pin)?;
            manager.request_challenge()?;
            manager.verify_challenge(pin)?;
            serde_json::json!({"status": "ok"})
        }
    };

    print_output(&result, Status::Success, cli_json);
    Ok(())
}

fn run_host(args: HostCommand, cli_json: bool) -> Result<(), APWError> {
    match args.command {
        HostSubcommand::Install => {
            let payload = native_host_install()?;
            print_output(&payload, Status::Success, cli_json);
        }
        HostSubcommand::Doctor(options) => {
            let payload = native_host_doctor()?;
            print_output(&payload, Status::Success, options.json || cli_json);
        }
        HostSubcommand::Uninstall => {
            let payload = native_host_uninstall()?;
            print_output(&payload, Status::Success, cli_json);
        }
    }

    Ok(())
}

fn run_pw(
    manager: &mut ApplePasswordManager,
    args: PwCommand,
    cli_json: bool,
) -> Result<(), APWError> {
    match args.action {
        Some(PwAction::Get { url, username }) => {
            let payload = manager.get_password_for_url(
                &sanitize_url(&url)?,
                username.unwrap_or_default().as_str(),
            )?;
            print_entries(&payload, cli_json)
        }
        Some(PwAction::List { url }) => {
            let payload = manager.get_login_names_for_url(&sanitize_url(&url)?)?;
            print_entries(&payload, cli_json)
        }
        None => {
            let action = ask_pw_action()?;
            let url = sanitize_url(&read_prompt("Enter URL: ")?)?;
            match action {
                PwAction::Get { .. } => {
                    let username = read_prompt("Enter username (optional): ")?;
                    let payload = manager.get_password_for_url(&url, username.as_str())?;
                    print_entries(&payload, cli_json)
                }
                PwAction::List { .. } => {
                    let payload = manager.get_login_names_for_url(&url)?;
                    print_entries(&payload, cli_json)
                }
            }
        }
    }
}

fn run_otp(
    manager: &mut ApplePasswordManager,
    args: OtpCommand,
    cli_json: bool,
) -> Result<(), APWError> {
    match args.action {
        Some(OtpAction::Get { url }) => {
            let payload = manager.get_otp_for_url(&sanitize_url(&url)?)?;
            print_entries(&payload, cli_json)
        }
        Some(OtpAction::List { url }) => {
            let payload = manager.list_otp_for_url(&sanitize_url(&url)?)?;
            print_entries(&payload, cli_json)
        }
        None => {
            let action = ask_otp_action()?;
            let url = sanitize_url(&read_prompt("Enter URL: ")?)?;
            match action {
                OtpAction::Get { .. } => {
                    let payload = manager.get_otp_for_url(&url)?;
                    print_entries(&payload, cli_json)
                }
                OtpAction::List { .. } => {
                    let payload = manager.list_otp_for_url(&url)?;
                    print_entries(&payload, cli_json)
                }
            }
        }
    }
}

async fn run_start(args: StartCommand) -> Result<(), APWError> {
    let host = parse_host(&args.bind)?;
    let port = args.port;
    start_daemon(DaemonOptions {
        port,
        host,
        runtime_mode: args.runtime_mode,
        dry_run: args.dry_run,
    })
    .await
}

fn parse_runtime_mode(raw: &str) -> std::result::Result<RuntimeMode, String> {
    let normalized = raw.trim().to_lowercase();
    Ok(match normalized.as_str() {
        "auto" => RuntimeMode::Auto,
        "native" => RuntimeMode::Native,
        "browser" => RuntimeMode::Browser,
        "direct" => RuntimeMode::Direct,
        "launchd" => RuntimeMode::Launchd,
        "disabled" => RuntimeMode::Disabled,
        _ => {
            return Err(
                "runtime mode must be one of auto|native|browser|direct|launchd|disabled."
                    .to_string(),
            );
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use rand::{thread_rng, Rng};

    #[test]
    fn host_validation_rejects_spaces() {
        assert!(is_valid_host("localhost"));
        assert!(!is_valid_host("local host"));
        assert!(!is_valid_host("   "));
        assert!(!is_valid_host("a\0b"));
    }

    #[test]
    fn parse_host_requires_value() {
        assert!(parse_host("127.0.0.1").is_ok());
        assert!(parse_host("   ").is_err());
        assert!(parse_host("bad host").is_err());
    }

    #[test]
    fn pin_normalization_is_strict() {
        assert_eq!(normalize_pin("123456".to_string()).unwrap(), "123456");
        assert!(normalize_pin("12345".to_string()).is_err());
        assert!(normalize_pin("12ab56".to_string()).is_err());
        assert!(normalize_pin(" 123456 ".to_string()).is_err());
    }

    #[test]
    fn parse_url_is_optional_https_default() {
        assert_eq!(sanitize_url("example.com").unwrap(), "https://example.com");
        assert!(sanitize_url("not a url").is_err());
    }

    #[test]
    fn parse_url_rejects_nulls_and_missing_host() {
        assert!(sanitize_url("   ").is_err());
        assert!(sanitize_url("http://\0evil").is_err());
        assert!(sanitize_url("://bad").is_err());
    }

    #[test]
    fn sanitize_url_fuzzed_inputs_stay_defensive() {
        let mut rng = thread_rng();
        for _ in 0..2048 {
            let len = rng.gen_range(0..256usize);
            let mut raw = vec![0_u8; len];
            rng.fill(raw.as_mut_slice());
            let candidate = String::from_utf8_lossy(&raw).to_string();
            match sanitize_url(&candidate) {
                Ok(value) => {
                    let parsed = if value.contains("://") {
                        value.to_string()
                    } else {
                        format!("https://{value}")
                    };

                    let parsed = url::Url::parse(&parsed).expect("sanitized URL must parse");
                    assert!(parsed.host_str().is_some());
                }
                Err(error) => {
                    assert!(
                        error.code == Status::GenericError || error.code == Status::InvalidParam
                    );
                }
            }
        }
    }

    #[test]
    fn status_json_aliases_global_flag() {
        let parsed = Cli::try_parse_from(["apw", "--json", "status", "--json"]).unwrap();
        assert!(parsed.json);
        match parsed.command {
            Commands::Status(_) => {}
            _ => panic!("expected status command"),
        }
    }

    #[test]
    fn start_command_rejects_invalid_bind_host() {
        assert!(
            Cli::try_parse_from(["apw", "start", "--bind", "bad host", "--port", "5000"]).is_err()
        );
    }

    #[test]
    fn start_command_rejects_invalid_port() {
        assert!(
            Cli::try_parse_from(["apw", "start", "--bind", "127.0.0.1", "--port", "bad"]).is_err()
        );
    }

    #[test]
    fn parse_status_global_json_defaults_to_status_json() {
        let parsed = Cli::try_parse_from(["apw", "--json", "status"]).unwrap();
        assert!(parsed.json);
    }

    #[test]
    fn auth_response_command_requires_expected_fields() {
        let parsed = Cli::try_parse_from([
            "apw",
            "auth",
            "response",
            "--pin",
            "123456",
            "--salt",
            "AQ==",
            "--server_key",
            "Ag==",
            "--client_key",
            "Aw==",
            "--username",
            "alice",
        ])
        .unwrap();
        match parsed.command {
            Commands::Auth(auth) => match auth.command {
                Some(AuthSubcommand::Response(_)) => {}
                _ => panic!("expected auth response command"),
            },
            _ => panic!("expected auth command"),
        }
    }

    #[test]
    fn auth_response_command_accepts_camel_case_keys() {
        let parsed = Cli::try_parse_from([
            "apw",
            "auth",
            "response",
            "--pin",
            "123456",
            "--salt",
            "AQ==",
            "--serverKey",
            "Ag==",
            "--clientKey",
            "Aw==",
            "--username",
            "alice",
        ])
        .unwrap();
        match parsed.command {
            Commands::Auth(auth) => match auth.command {
                Some(AuthSubcommand::Response(response)) => {
                    assert_eq!(response.server_key, "Ag==");
                    assert_eq!(response.client_key, "Aw==");
                }
                _ => panic!("expected auth response command"),
            },
            _ => panic!("expected auth command"),
        }
    }

    #[test]
    fn auth_response_command_accepts_legacy_short_flags() {
        let parsed = Cli::try_parse_from([
            "apw",
            "auth",
            "response",
            "-p",
            "123456",
            "-s",
            "AQ==",
            "--serverKey",
            "Ag==",
            "-c",
            "Aw==",
            "-u",
            "alice",
        ])
        .unwrap();
        match parsed.command {
            Commands::Auth(auth) => match auth.command {
                Some(AuthSubcommand::Response(response)) => {
                    assert_eq!(response.pin, "123456");
                    assert_eq!(response.salt, "AQ==");
                    assert_eq!(response.server_key, "Ag==");
                    assert_eq!(response.client_key, "Aw==");
                    assert_eq!(response.username, "alice");
                }
                _ => panic!("expected auth response command"),
            },
            _ => panic!("expected auth command"),
        }
    }

    #[test]
    fn start_command_defaults_match_legacy() {
        let parsed = Cli::try_parse_from(["apw", "start"]).unwrap();
        match parsed.command {
            Commands::Start(start) => {
                assert_eq!(start.port, 0);
                assert_eq!(start.bind, "127.0.0.1");
            }
            _ => panic!("expected start command"),
        }
    }

    #[test]
    fn start_command_accepts_browser_runtime_mode() {
        let parsed = Cli::try_parse_from(["apw", "start", "--runtime-mode", "browser"]).unwrap();
        match parsed.command {
            Commands::Start(start) => {
                assert_eq!(start.runtime_mode, RuntimeMode::Browser);
            }
            _ => panic!("expected start command"),
        }
    }

    #[test]
    fn host_install_command_parses() {
        let parsed = Cli::try_parse_from(["apw", "host", "install"]).unwrap();
        match parsed.command {
            Commands::Host(host) => match host.command {
                HostSubcommand::Install => {}
                _ => panic!("expected host install command"),
            },
            _ => panic!("expected host command"),
        }
    }

    #[test]
    fn host_doctor_command_accepts_json_flag() {
        let parsed = Cli::try_parse_from(["apw", "host", "doctor", "--json"]).unwrap();
        match parsed.command {
            Commands::Host(host) => match host.command {
                HostSubcommand::Doctor(options) => {
                    assert!(options.json);
                }
                _ => panic!("expected host doctor command"),
            },
            _ => panic!("expected host command"),
        }
    }

    #[test]
    fn host_uninstall_command_parses() {
        let parsed = Cli::try_parse_from(["apw", "host", "uninstall"]).unwrap();
        match parsed.command {
            Commands::Host(host) => match host.command {
                HostSubcommand::Uninstall => {}
                _ => panic!("expected host uninstall command"),
            },
            _ => panic!("expected host command"),
        }
    }

    #[test]
    fn print_entries_rejects_errors() {
        let payload = Payload {
            status: Status::NoResults,
            entries: Vec::new(),
        };
        let result = print_entries(&payload, false);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, Status::NoResults);
    }

    #[test]
    fn app_install_command_parses() {
        let parsed = Cli::try_parse_from(["apw", "app", "install"]).unwrap();
        match parsed.command {
            Commands::App(app) => match app.command {
                AppSubcommand::Install => {}
                _ => panic!("expected app install command"),
            },
            _ => panic!("expected app command"),
        }
    }

    #[test]
    fn doctor_command_parses() {
        let parsed = Cli::try_parse_from(["apw", "doctor"]).unwrap();
        match parsed.command {
            Commands::Doctor(_) => {}
            _ => panic!("expected doctor command"),
        }
    }

    #[test]
    fn login_command_parses() {
        let parsed = Cli::try_parse_from(["apw", "login", "https://example.com"]).unwrap();
        match parsed.command {
            Commands::Login(login) => {
                assert_eq!(login.url, "https://example.com");
            }
            _ => panic!("expected login command"),
        }
    }
}
