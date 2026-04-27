use clap::Parser;

mod cli;
mod client;
mod daemon;
mod doctor;
mod error;
mod host;
mod logging;
mod native_app;
mod secrets;
mod srp;
mod types;
mod utils;

use cli::{run, Cli};
use client::ApplePasswordManager;
use logging::error as log_error;
use std::env;
use std::process;

#[tokio::main]
async fn main() {
    let raw_args: Vec<String> = env::args().collect();
    let normalized_args = normalize_legacy_args(raw_args);
    let args = Cli::parse_from(normalized_args);
    let json_output = args.json;
    logging::init(args.log_level, json_output);
    let manager = ApplePasswordManager::new();
    if let Err(error) = run(manager, args).await {
        if should_emit_text_error_log(json_output) {
            log_error("cli", &error.message);
        }
        if json_output {
            eprintln!(
                "{}",
                serde_json::json!({
                  "ok": false,
                  "code": error.code,
                  "error": error.message,
                })
            );
            process::exit(i32::from(error.code));
        }
        eprintln!("{}", error.message);
        process::exit(i32::from(error.code));
    }
}

fn should_emit_text_error_log(json_output: bool) -> bool {
    !json_output
}

fn normalize_legacy_args(raw: Vec<String>) -> Vec<String> {
    raw.into_iter()
        .map(|arg| match arg.as_str() {
            "-sk" => "--serverKey".to_string(),
            "-ck" => "--clientKey".to_string(),
            other => other.to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{normalize_legacy_args, should_emit_text_error_log};

    #[test]
    fn normalizes_legacy_auth_short_flags() {
        let args = vec![
            "apw".to_string(),
            "-sk".to_string(),
            "server".to_string(),
            "-ck".to_string(),
            "client".to_string(),
        ];
        assert_eq!(
            normalize_legacy_args(args),
            vec![
                "apw".to_string(),
                "--serverKey".to_string(),
                "server".to_string(),
                "--clientKey".to_string(),
                "client".to_string(),
            ]
        );
    }

    #[test]
    fn suppresses_text_error_logs_for_json_output() {
        assert!(!should_emit_text_error_log(true));
        assert!(should_emit_text_error_log(false));
    }
}
