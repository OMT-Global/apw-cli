use crate::error::APWError;
use crate::logging::{self, LogLevel};
use crate::native_app::{
    native_app_doctor, native_app_fill, native_app_install, native_app_launch, native_app_login,
    native_app_status,
};
use crate::types::{Status, BUILD_DATE, BUILD_TARGET, GIT_SHA, RUST_VERSION, VERSION};
use clap::{Args, Parser, Subcommand};
use serde_json::json;

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

fn sanitize_native_credential_url(raw: &str) -> Result<String, APWError> {
    let candidate = sanitize_url(raw)?;
    let parsed = url::Url::parse(&candidate).map_err(|_| {
        APWError::new(
            Status::InvalidParam,
            format!("Invalid native credential URL: '{candidate}'"),
        )
    })?;
    if parsed.scheme() != "https" {
        return Err(APWError::new(
            Status::InvalidParam,
            "Native credential requests require an https URL.",
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

fn print_status(payload: serde_json::Value, json_output: bool) {
    print_output(&payload, Status::Success, json_output);
}

#[derive(Parser)]
#[command(name = "apw")]
#[command(version = env!("CARGO_PKG_VERSION"))]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
    #[arg(long = "json", global = true)]
    pub json: bool,
    #[arg(
        long = "log-level",
        global = true,
        env = "APW_LOG",
        default_value = "warn"
    )]
    pub log_level: LogLevel,
}

#[derive(Subcommand)]
pub enum Commands {
    App(AppCommand),
    Doctor(DoctorCommand),
    Fill(FillCommand),
    Login(LoginCommand),
    Status(StatusCommand),
    Version(VersionCommand),
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
pub struct DoctorCommand {
    /// Emit only the structured environment-check array. Useful for CI
    /// jobs that want to grep `[FAIL]` lines or parse the JSON shape.
    /// See issue #12.
    #[arg(long)]
    pub ci: bool,
}

#[derive(Args)]
pub struct LoginCommand {
    pub url: String,
    #[arg(
        long = "external-fallback",
        help = "Explicitly allow reduced-security external password-manager CLI fallback when the native broker is unavailable or returns no results."
    )]
    pub external_fallback: bool,
}

#[derive(Args)]
pub struct FillCommand {
    pub url: String,
}

#[derive(Args)]
pub struct StatusCommand {
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Default)]
pub struct VersionCommand {}

pub async fn run(cli: Cli) -> Result<(), APWError> {
    match cli.command {
        Commands::App(args) => run_app(args, cli.json),
        Commands::Doctor(args) => run_doctor(args, cli.json),
        Commands::Fill(args) => run_fill(args, cli.json),
        Commands::Login(args) => run_login(args, cli.json),
        Commands::Status(args) => run_status(args, cli.json),
        Commands::Version(args) => run_version(args, cli.json),
    }
}

fn run_app(args: AppCommand, cli_json: bool) -> Result<(), APWError> {
    let payload = match args.command {
        AppSubcommand::Install => {
            logging::info("app", "installing native app bundle");
            native_app_install()?
        }
        AppSubcommand::Launch => {
            logging::info("app", "launching native app broker");
            native_app_launch()?
        }
    };
    print_output(&payload, Status::Success, cli_json);
    Ok(())
}

fn run_doctor(args: DoctorCommand, cli_json: bool) -> Result<(), APWError> {
    logging::info("doctor", "collecting native app diagnostics");
    let environment = crate::doctor::run_environment_checks();
    let environment_json = crate::doctor::checks_to_json(&environment);

    if args.ci {
        // CI mode always emits the structured envelope so downstream
        // tooling can parse `[FAIL]` deterministically.
        print_output(&environment_json, Status::Success, true);
        return Ok(());
    }

    let mut payload = native_app_doctor()?;
    if let Some(object) = payload.as_object_mut() {
        object.insert("environment".to_string(), environment_json);
    }

    if !cli_json {
        // Surface the human-readable check lines on stderr so the
        // existing JSON-on-stdout payload stays parseable.
        for line in crate::doctor::render_check_lines(&environment) {
            eprintln!("{line}");
        }
    }

    print_output(&payload, Status::Success, cli_json);
    Ok(())
}

fn run_fill(args: FillCommand, cli_json: bool) -> Result<(), APWError> {
    logging::info(
        "fill",
        format!("requesting fill credential for {}", args.url),
    );
    let payload = native_app_fill(&sanitize_native_credential_url(&args.url)?)?;
    print_output(&payload, Status::Success, cli_json);
    Ok(())
}

fn run_login(args: LoginCommand, cli_json: bool) -> Result<(), APWError> {
    logging::info("login", format!("requesting credential for {}", args.url));
    let payload = native_app_login(
        &sanitize_native_credential_url(&args.url)?,
        args.external_fallback,
    )?;
    print_output(&payload, Status::Success, cli_json);
    Ok(())
}

fn run_status(args: StatusCommand, cli_json: bool) -> Result<(), APWError> {
    logging::debug("status", "collecting native app status");
    let payload = native_app_status();
    print_status(payload, args.json || cli_json);
    Ok(())
}

fn run_version(_args: VersionCommand, cli_json: bool) -> Result<(), APWError> {
    if cli_json {
        print_output(&version_payload()?, Status::Success, true);
        return Ok(());
    }

    print_output(
        &serde_json::Value::String(format!("apw {}", VERSION)),
        Status::Success,
        false,
    );
    Ok(())
}

fn version_payload() -> Result<serde_json::Value, APWError> {
    Ok(json!({
      "version": VERSION,
      "semver": parse_semver(VERSION)?,
      "build_date": BUILD_DATE,
      "git_sha": GIT_SHA,
      "rust_version": RUST_VERSION,
      "target": BUILD_TARGET,
    }))
}

fn parse_semver(version: &str) -> Result<serde_json::Value, APWError> {
    let invalid = || APWError::new(Status::GenericError, "Invalid semantic version.");
    let (core_and_prerelease, build) = match version.split_once('+') {
        Some((core_and_prerelease, build)) if !build.is_empty() && !build.contains('+') => {
            (core_and_prerelease, Some(build))
        }
        Some(_) => return Err(invalid()),
        None => (version, None),
    };
    let (core, prerelease) = match core_and_prerelease.split_once('-') {
        Some((core, prerelease)) if !prerelease.is_empty() => (core, Some(prerelease)),
        Some(_) => return Err(invalid()),
        None => (core_and_prerelease, None),
    };

    let mut parts = core.split('.');
    let major = parse_semver_number(parts.next())?;
    let minor = parse_semver_number(parts.next())?;
    let patch = parse_semver_number(parts.next())?;

    if parts.next().is_some() {
        return Err(invalid());
    }

    if let Some(prerelease) = prerelease {
        validate_semver_identifiers(prerelease, true)?;
    }

    if let Some(build) = build {
        validate_semver_identifiers(build, false)?;
    }

    Ok(json!({
      "major": major,
      "minor": minor,
      "patch": patch,
    }))
}

fn parse_semver_number(part: Option<&str>) -> Result<u64, APWError> {
    let invalid = || APWError::new(Status::GenericError, "Invalid semantic version.");
    let part = part.ok_or_else(invalid)?;
    if part.is_empty()
        || !part.chars().all(|value| value.is_ascii_digit())
        || (part.len() > 1 && part.starts_with('0'))
    {
        return Err(invalid());
    }

    part.parse::<u64>().map_err(|_| invalid())
}

fn validate_semver_identifiers(
    value: &str,
    reject_numeric_leading_zero: bool,
) -> Result<(), APWError> {
    let invalid = || APWError::new(Status::GenericError, "Invalid semantic version.");
    for part in value.split('.') {
        if part.is_empty()
            || !part
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
        {
            return Err(invalid());
        }
        if reject_numeric_leading_zero
            && part.len() > 1
            && part.chars().all(|ch| ch.is_ascii_digit())
            && part.starts_with('0')
        {
            return Err(invalid());
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{CommandFactory, Parser};
    use rand::{thread_rng, Rng};

    #[test]
    fn parse_url_is_optional_https_default() {
        assert_eq!(sanitize_url("example.com").unwrap(), "https://example.com");
        assert!(sanitize_url("not a url").is_err());
    }

    #[test]
    fn native_credential_urls_must_be_https() {
        assert_eq!(
            sanitize_native_credential_url("example.com").unwrap(),
            "https://example.com"
        );
        assert_eq!(
            sanitize_native_credential_url("https://example.com/login").unwrap(),
            "https://example.com/login"
        );
        assert!(sanitize_native_credential_url("http://example.com").is_err());
        assert!(sanitize_native_credential_url("ftp://example.com").is_err());
    }

    #[test]
    fn parse_url_rejects_nulls_and_missing_host() {
        assert!(sanitize_url("   ").is_err());
        assert!(sanitize_url("http://\0evil").is_err());
        assert!(sanitize_url("://bad").is_err());
    }

    #[test]
    fn fill_subcommand_is_parsed() {
        let cli = Cli::parse_from(["apw", "fill", "example.com"]);
        assert!(matches!(cli.command, Commands::Fill(_)));
    }

    #[test]
    fn legacy_daemon_commands_are_removed_from_help() {
        let mut command = Cli::command();
        for name in ["auth", "pw", "otp", "start"] {
            assert!(
                command.find_subcommand_mut(name).is_none(),
                "removed legacy subcommand still appears in help: {name}"
            );
        }
    }

    #[test]
    fn version_subcommand_is_parsed() {
        let cli = Cli::parse_from(["apw", "version"]);
        assert!(matches!(cli.command, Commands::Version(_)));
    }

    #[test]
    fn version_payload_includes_expected_metadata() {
        let payload = version_payload().unwrap();
        assert_eq!(payload["version"], VERSION);
        assert_eq!(payload["semver"], parse_semver(VERSION).unwrap());
        assert_eq!(payload["build_date"], BUILD_DATE);
        assert_eq!(payload["git_sha"], GIT_SHA);
        assert_eq!(payload["rust_version"], RUST_VERSION);
        assert_eq!(payload["target"], BUILD_TARGET);
    }

    #[test]
    fn parse_semver_accepts_prerelease_and_build_metadata() {
        let prerelease = parse_semver("2.1.0-rc.1").unwrap();
        assert_eq!(prerelease["major"], 2);
        assert_eq!(prerelease["minor"], 1);
        assert_eq!(prerelease["patch"], 0);

        let build = parse_semver("2.1.0+build.7").unwrap();
        assert_eq!(build["major"], 2);
        assert_eq!(build["minor"], 1);
        assert_eq!(build["patch"], 0);

        let combined = parse_semver("2.1.0-rc.1+build.7").unwrap();
        assert_eq!(combined["major"], 2);
        assert_eq!(combined["minor"], 1);
        assert_eq!(combined["patch"], 0);
    }

    #[test]
    fn parse_semver_rejects_invalid_shapes() {
        assert!(parse_semver("1.2").is_err());
        assert!(parse_semver("1.2.3.4").is_err());
        assert!(parse_semver("one.two.three").is_err());
        assert!(parse_semver("1.2.3-01").is_err());
        assert!(parse_semver("1.2.3-").is_err());
        assert!(parse_semver("1.2.3+").is_err());
    }

    #[test]
    fn log_level_can_be_loaded_from_env() {
        std::env::set_var("APW_LOG", "debug");
        let cli = Cli::parse_from(["apw", "status"]);
        std::env::remove_var("APW_LOG");
        assert_eq!(cli.log_level, LogLevel::Debug);
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
    fn parse_status_global_json_defaults_to_status_json() {
        let parsed = Cli::try_parse_from(["apw", "--json", "status"]).unwrap();
        assert!(parsed.json);
    }

    #[test]
    fn legacy_daemon_commands_are_rejected() {
        for args in [
            &["apw", "auth"][..],
            &["apw", "host", "install"][..],
            &["apw", "pw", "list", "example.com"][..],
            &["apw", "otp", "list", "example.com"][..],
            &["apw", "start"][..],
        ] {
            assert!(
                Cli::try_parse_from(args).is_err(),
                "removed legacy command unexpectedly parsed: {args:?}"
            );
        }
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
