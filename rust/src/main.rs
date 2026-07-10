use clap::Parser;

mod bundle;
mod cli;
mod doctor;
mod error;
mod logging;
mod native_app;
mod secrets;
mod state_root;
mod types;
mod utils;

use cli::{run, Cli, Commands};
use logging::error as log_error;
use std::env;
use std::process;

#[tokio::main]
async fn main() {
    let args = Cli::parse_from(env::args());
    let json_output = args.json;
    logging::init(args.log_level, json_output);
    let result = if command_requires_state(&args.command) {
        match state_root::apw_state_root() {
            Ok(_) => run(args).await,
            Err(error) => Err(error),
        }
    } else {
        run(args).await
    };

    if let Err(error) = result {
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

fn command_requires_state(command: &Commands) -> bool {
    !matches!(command, Commands::Version(_))
}

fn should_emit_text_error_log(json_output: bool) -> bool {
    !json_output
}

#[cfg(test)]
mod tests {
    use super::{command_requires_state, should_emit_text_error_log};
    use crate::cli::{Cli, Commands};
    use clap::Parser;

    #[test]
    fn suppresses_text_error_logs_for_json_output() {
        assert!(!should_emit_text_error_log(true));
        assert!(should_emit_text_error_log(false));
    }

    #[test]
    fn version_is_the_only_state_free_subcommand() {
        let version = Cli::parse_from(["apw", "version"]);
        assert!(!command_requires_state(&version.command));

        let status = Cli::parse_from(["apw", "status"]);
        assert!(matches!(status.command, Commands::Status(_)));
        assert!(command_requires_state(&status.command));
    }
}
