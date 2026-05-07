//! Environment / toolchain checks surfaced by `apw doctor`. Each check is
//! cheap, side-effect-free, and yields a structured `DoctorCheck` so both
//! the human and JSON renderers can consume the same data. See issue #12.

use serde::Serialize;
use serde_json::{json, Value};
use std::path::Path;
use std::process::Command;
use std::time::Duration;

/// Bound on every external probe. A misconfigured shim that hangs must not
/// block `apw doctor`.
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Ok,
    Warn,
    Fail,
    Skip,
}

impl CheckStatus {
    pub fn as_label(self) -> &'static str {
        match self {
            Self::Ok => "OK",
            Self::Warn => "WARN",
            Self::Fail => "FAIL",
            Self::Skip => "SKIP",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DoctorCheck {
    pub name: &'static str,
    pub status: CheckStatus,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remediation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detected_version: Option<String>,
}

impl DoctorCheck {
    fn new(name: &'static str, status: CheckStatus, message: impl Into<String>) -> Self {
        Self {
            name,
            status,
            message: message.into(),
            remediation: None,
            detected_version: None,
        }
    }

    fn with_remediation(mut self, hint: impl Into<String>) -> Self {
        self.remediation = Some(hint.into());
        self
    }

    fn with_version(mut self, version: impl Into<String>) -> Self {
        self.detected_version = Some(version.into());
        self
    }
}

fn is_macos() -> bool {
    cfg!(target_os = "macos")
}

fn run_probe(program: &str, args: &[&str]) -> Option<String> {
    use std::sync::mpsc;

    let program = program.to_owned();
    let args: Vec<String> = args.iter().map(|s| (*s).to_owned()).collect();

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let result = Command::new(program).args(args).output();
        let _ = tx.send(result);
    });

    let output = rx.recv_timeout(PROBE_TIMEOUT).ok()?.ok()?;
    if !output.status.success() {
        return None;
    }
    let combined = if !output.stdout.is_empty() {
        output.stdout
    } else {
        output.stderr
    };
    let trimmed = String::from_utf8_lossy(&combined).trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

fn check_xcodebuild() -> DoctorCheck {
    if !is_macos() {
        return DoctorCheck::new(
            "xcodebuild",
            CheckStatus::Skip,
            "xcodebuild is only required on macOS for building the native app bundle.",
        );
    }
    match run_probe("xcodebuild", &["-version"]) {
        Some(version) => {
            DoctorCheck::new("xcodebuild", CheckStatus::Ok, "xcodebuild is available.")
                .with_version(version.lines().next().unwrap_or("").to_string())
        }
        None => DoctorCheck::new(
            "xcodebuild",
            CheckStatus::Fail,
            "xcodebuild not found or not callable.",
        )
        .with_remediation("Install Xcode from the App Store, then run `xcode-select --install`."),
    }
}

fn check_rust_toolchain() -> DoctorCheck {
    match run_probe("rustc", &["--version"]) {
        Some(version) => DoctorCheck::new(
            "rust-toolchain",
            CheckStatus::Ok,
            "Rust toolchain is available.",
        )
        .with_version(version),
        None => DoctorCheck::new("rust-toolchain", CheckStatus::Fail, "rustc not found.")
            .with_remediation("Install via https://rustup.rs/."),
    }
}

fn check_detect_secrets() -> DoctorCheck {
    match run_probe("detect-secrets", &["--version"]) {
        Some(version) => DoctorCheck::new(
            "detect-secrets",
            CheckStatus::Ok,
            "detect-secrets is available.",
        )
        .with_version(version),
        None => DoctorCheck::new(
            "detect-secrets",
            CheckStatus::Warn,
            "detect-secrets not installed; pre-commit secrets scan will be skipped.",
        )
        .with_remediation(
            "`brew install detect-secrets` (macOS) or `pipx install detect-secrets`.",
        ),
    }
}

fn check_signing_identity() -> DoctorCheck {
    if !is_macos() {
        return DoctorCheck::new(
            "code-signing",
            CheckStatus::Skip,
            "Apple code-signing identities only apply on macOS.",
        );
    }
    match run_probe("security", &["find-identity", "-v", "-p", "codesigning"]) {
        Some(output) if output.contains("Developer ID Application") => DoctorCheck::new(
            "code-signing",
            CheckStatus::Ok,
            "At least one Developer ID Application certificate is available.",
        ),
        Some(_) => DoctorCheck::new(
            "code-signing",
            CheckStatus::Warn,
            "No `Developer ID Application` certificate found in the keychain. Release builds will fail to sign.",
        )
        .with_remediation(
            "Download an Apple Developer ID Application certificate and import it into your login keychain.",
        ),
        None => DoctorCheck::new(
            "code-signing",
            CheckStatus::Fail,
            "`security find-identity` is not callable.",
        ),
    }
}

fn check_runner_labels() -> Option<DoctorCheck> {
    if std::env::var("CI").as_deref() != Ok("true") {
        return None;
    }
    let labels = std::env::var("RUNNER_LABELS").ok();
    let mut check = match labels.as_deref() {
        Some(value) if !value.is_empty() => DoctorCheck::new(
            "ci-runner-labels",
            CheckStatus::Ok,
            "Runner labels exposed via RUNNER_LABELS.",
        )
        .with_version(value.to_string()),
        _ => DoctorCheck::new(
            "ci-runner-labels",
            CheckStatus::Warn,
            "Running in CI but RUNNER_LABELS is not set; cannot verify runner pool selection.",
        )
        .with_remediation("Export RUNNER_LABELS in the workflow step env, e.g. `RUNNER_LABELS: ${{ join(runner.labels, ',') }}`."),
    };
    if check.status == CheckStatus::Ok {
        let value = check.detected_version.clone().unwrap_or_default();
        if value.contains("self-hosted") && !value.contains("public") {
            check = DoctorCheck::new(
                "ci-runner-labels",
                CheckStatus::Warn,
                format!(
                    "Self-hosted runner labels `{value}` do not include the `public` tag expected for the open-source CI lane."
                ),
            );
        }
    }
    Some(check)
}

fn check_native_app_bundle() -> DoctorCheck {
    let bundle = crate::native_app::native_app_bundle_install_path();
    if bundle.exists() {
        return DoctorCheck::new(
            "native-app-bundle",
            CheckStatus::Ok,
            format!("APW.app installed at {}.", bundle.display()),
        );
    }
    let source_candidates = [
        Path::new("native-app/dist/APW.app"),
        Path::new("../native-app/dist/APW.app"),
    ];
    if source_candidates.iter().any(|candidate| candidate.exists()) {
        return DoctorCheck::new(
            "native-app-bundle",
            CheckStatus::Warn,
            "Source-built APW.app exists but has not been installed.",
        )
        .with_remediation(
            "Run `apw app install` to copy the bundle into ~/.apw/native-app/installed/.",
        );
    }
    DoctorCheck::new(
        "native-app-bundle",
        CheckStatus::Warn,
        "APW.app bundle is not built.",
    )
    .with_remediation("Run `./scripts/build-native-app.sh`, then `apw app install`.")
}

/// Probe each configured associated domain for a reachable AASA file.
/// Domains are read from `APW_AASA_DOMAINS` (comma-separated) so this can
/// be wired ahead of the `supportedDomains` config field landing. See
/// issue #8.
fn check_associated_domains() -> Option<DoctorCheck> {
    let raw = std::env::var("APW_AASA_DOMAINS").ok()?;
    let domains: Vec<&str> = raw
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if domains.is_empty() {
        return None;
    }

    let mut failures: Vec<String> = Vec::new();
    for domain in &domains {
        let url = format!("https://{domain}/.well-known/apple-app-site-association");
        // `curl -fsI` is a small dependency footprint — most macOS / Linux
        // hosts have it available, and we just need a HEAD probe.
        let probe = run_probe("curl", &["-fsI", "--max-time", "5", &url]);
        if probe.is_none() {
            failures.push(domain.to_string());
        }
    }

    if failures.is_empty() {
        Some(
            DoctorCheck::new(
                "associated-domains",
                CheckStatus::Ok,
                format!(
                    "AASA files reachable for {} configured domain(s).",
                    domains.len()
                ),
            )
            .with_version(domains.join(",")),
        )
    } else {
        Some(
            DoctorCheck::new(
                "associated-domains",
                CheckStatus::Fail,
                format!(
                    "AASA file unreachable for: {}",
                    failures.join(", ")
                ),
            )
            .with_remediation(
                "Each domain must serve application/json at /.well-known/apple-app-site-association without redirects. See docs/DOMAIN_EXPANSION.md.",
            ),
        )
    }
}

pub fn run_environment_checks() -> Vec<DoctorCheck> {
    let mut checks = vec![
        check_xcodebuild(),
        check_rust_toolchain(),
        check_detect_secrets(),
        check_signing_identity(),
        check_native_app_bundle(),
    ];
    if let Some(runner) = check_runner_labels() {
        checks.push(runner);
    }
    if let Some(aasa) = check_associated_domains() {
        checks.push(aasa);
    }
    checks
}

pub fn render_check_lines(checks: &[DoctorCheck]) -> Vec<String> {
    checks
        .iter()
        .map(|check| {
            let mut line = format!(
                "[{}] {}: {}",
                check.status.as_label(),
                check.name,
                check.message
            );
            if let Some(version) = &check.detected_version {
                line.push_str(&format!(" (detected: {version})"));
            }
            if let Some(hint) = &check.remediation {
                line.push_str(&format!("\n      → {hint}"));
            }
            line
        })
        .collect()
}

pub fn checks_to_json(checks: &[DoctorCheck]) -> Value {
    json!(checks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_status_label_is_uppercase() {
        assert_eq!(CheckStatus::Ok.as_label(), "OK");
        assert_eq!(CheckStatus::Warn.as_label(), "WARN");
        assert_eq!(CheckStatus::Fail.as_label(), "FAIL");
        assert_eq!(CheckStatus::Skip.as_label(), "SKIP");
    }

    #[test]
    fn rust_toolchain_check_succeeds_in_test_env() {
        let check = check_rust_toolchain();
        // The test runs under cargo, so rustc must be reachable.
        assert_eq!(check.status, CheckStatus::Ok);
        assert!(check.detected_version.is_some());
    }

    #[test]
    fn xcodebuild_check_skips_on_non_macos() {
        let check = check_xcodebuild();
        if !cfg!(target_os = "macos") {
            assert_eq!(check.status, CheckStatus::Skip);
        }
    }

    #[test]
    fn signing_identity_skips_on_non_macos() {
        let check = check_signing_identity();
        if !cfg!(target_os = "macos") {
            assert_eq!(check.status, CheckStatus::Skip);
        }
    }

    #[test]
    fn run_environment_checks_returns_at_least_the_core_set() {
        let checks = run_environment_checks();
        let names: Vec<_> = checks.iter().map(|c| c.name).collect();
        assert!(names.contains(&"xcodebuild"));
        assert!(names.contains(&"rust-toolchain"));
        assert!(names.contains(&"detect-secrets"));
        assert!(names.contains(&"code-signing"));
        assert!(names.contains(&"native-app-bundle"));
    }

    #[test]
    fn json_render_is_a_valid_array() {
        let checks = run_environment_checks();
        let value = checks_to_json(&checks);
        assert!(value.is_array());
        assert!(!value.as_array().unwrap().is_empty());
    }

    #[test]
    fn human_render_includes_status_label() {
        let checks = run_environment_checks();
        let lines = render_check_lines(&checks);
        assert!(lines.iter().any(|line| line.starts_with('[')));
    }

    #[test]
    fn associated_domains_check_skipped_when_env_unset() {
        std::env::remove_var("APW_AASA_DOMAINS");
        assert!(check_associated_domains().is_none());
    }

    #[test]
    fn associated_domains_check_reports_failure_for_unreachable_host() {
        // Use a guaranteed-unreachable .invalid TLD (RFC 2606). curl will
        // exit non-zero so the probe returns None and the check fails.
        std::env::set_var("APW_AASA_DOMAINS", "definitely-not-a-real-host.invalid");
        let check = check_associated_domains().expect("expected an AASA check");
        std::env::remove_var("APW_AASA_DOMAINS");
        assert_eq!(check.status, CheckStatus::Fail);
        assert!(check.message.contains("unreachable"));
    }
}
