use clap::Parser;

mod bundle;
mod cli;
mod doctor;
mod error;
mod logging;
mod native_app;
mod secrets;
mod types;
mod utils;

use cli::{run, Cli};
use logging::error as log_error;
use std::env;
use std::process;

#[tokio::main]
async fn main() {
    let args = Cli::parse_from(env::args());
    let json_output = args.json;
    logging::init(args.log_level, json_output);
    if let Err(error) = run(args).await {
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

#[cfg(test)]
mod tests {
    use super::should_emit_text_error_log;

    #[test]
    fn suppresses_text_error_logs_for_json_output() {
        assert!(!should_emit_text_error_log(true));
        assert!(should_emit_text_error_log(false));
    }
}
