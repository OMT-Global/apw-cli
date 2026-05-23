use serde_json::Value;
use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

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

fn parse_json_output(output: &CommandOutput) -> Value {
    serde_json::from_str(&output.stdout)
        .unwrap_or_else(|_| panic!("command stdout was not JSON: {:?}", output.stdout))
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

#[test]
fn removed_legacy_daemon_commands_are_not_active_contract() {
    run_with_temp_home(|home| {
        for args in [
            &["auth"][..],
            &["pw", "list", "example.com"][..],
            &["otp", "list", "example.com"][..],
            &["start"][..],
        ] {
            let output = run_rust_cli(home, args);
            assert_ne!(
                output.status, 0,
                "removed legacy command unexpectedly succeeded: {args:?}"
            );
            assert!(
                output.stderr.contains("unrecognized subcommand"),
                "removed legacy command should be rejected by clap: {args:?}, stderr={:?}",
                output.stderr
            );
        }
    });
}

#[test]
fn parity_status_output_with_no_session_is_shape_compatible() {
    if !has_deno() {
        return;
    }

    run_with_temp_home(|home| {
        let rust = run_rust_cli(home, &["status", "--json"]);
        let deno = run_deno_cli(home, &["status", "--json"]);

        assert_eq!(rust.status, 0, "rust status failed: {rust:#?}");
        if deno.status != 0 && deno.stderr.contains("JSR package manifest") {
            return;
        }
        assert_eq!(deno.status, 0, "deno status failed: {deno:#?}");

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
