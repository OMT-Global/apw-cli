use clap::Parser;

mod cli;
mod client;
mod daemon;
mod error;
mod host;
mod secrets;
mod srp;
mod types;
mod utils;

const APP_VERSION: &str = "1.2.0";

use cli::{run, Cli};
use client::ApplePasswordManager;
use std::env;
use std::process;

#[tokio::main]
async fn main() {
    if APP_VERSION != env!("CARGO_PKG_VERSION") {
        eprintln!(
            "Version mismatch; please run cargo fmt and verify version sync files before release."
        );
        process::exit(1);
    }

    let raw_args: Vec<String> = env::args().collect();
    let normalized_args = normalize_legacy_args(raw_args);
    let args = Cli::parse_from(normalized_args);
    let manager = ApplePasswordManager::new();
    let json_output = args.json;
    if let Err(error) = run(manager, args).await {
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
    use super::normalize_legacy_args;

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
}
