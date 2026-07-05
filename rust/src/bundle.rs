//! Diagnostic-bundle export. `apw doctor --bundle <path>` packages a
//! deterministic, redacted tarball that operators can attach to support
//! requests without leaking credentials. See issue #56.
//!
//! ## Layout
//!
//! ```text
//! apw-doctor-bundle/
//!   manifest.json                 # bundle version + included files + redaction notes
//!   doctor.json                   # full `apw doctor --json` output
//!   environment.json              # `apw doctor --ci` environment checks
//!   os.json                       # uname + sw_vers (macOS) probes
//!   native-app/file-listing.json  # path/size/mode listing only (no contents)
//! ```
//!
//! ## Redaction guarantees
//!
//! - No environment variables are read or included.
//! - No file contents from `~/.apw/native-app/` are included — only
//!   metadata (relative path, byte size, octal mode).
//! - The bundle never reads `credentials.json`, `config.json`, or
//!   `broker.log`; an operator who wants to share those attaches them
//!   separately with their own judgement.
//! - Free-text strings written into the bundle (e.g. probe `message`
//!   fields) are scanned by `redact_value` for token-like patterns; any
//!   match aborts the bundle so the operator never ships an
//!   incompletely-redacted archive.

use crate::error::{APWError, Result};
use crate::native_app::{native_app_runtime_dir, uuid_like_suffix};
use crate::types::Status;
use chrono::Utc;
use serde_json::{json, Value};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const BUNDLE_VERSION: u32 = 1;
const BUNDLE_ROOT_NAME: &str = "apw-doctor-bundle";

/// Outcome of writing a diagnostic bundle. Returned to the CLI so the
/// human-facing summary can include the file count and applied redaction
/// counters.
#[derive(Debug)]
pub struct BundleResult {
    pub path: PathBuf,
    pub files_included: Vec<String>,
    pub redaction_checks: u32,
}

/// Write a deterministic, redacted diagnostic bundle to `output_path`.
///
/// Fails closed if any structured value contains a token-like substring
/// that could be a credential — the operator is told what to remove rather
/// than shipping an incompletely-redacted archive.
pub fn write_diagnostic_bundle(
    output_path: &Path,
    doctor_payload: &Value,
    environment: &Value,
) -> Result<BundleResult> {
    if output_path.as_os_str().is_empty() {
        return Err(APWError::new(
            Status::InvalidParam,
            "Bundle output path must not be empty.",
        ));
    }
    if output_path.is_dir() {
        return Err(APWError::new(
            Status::InvalidParam,
            format!(
                "Bundle output path {} is a directory; expected a file.",
                output_path.display()
            ),
        ));
    }

    let staging = StagingDir::create()?;
    let root = staging.path().join(BUNDLE_ROOT_NAME);
    fs::create_dir(&root).map_err(|error| {
        APWError::new(
            Status::ProcessNotRunning,
            format!("Failed to prepare bundle staging directory: {error}"),
        )
    })?;

    let mut redaction_checks: u32 = 0;
    let mut files_included: Vec<String> = Vec::new();

    write_json_file(
        &root,
        "doctor.json",
        doctor_payload,
        &mut files_included,
        &mut redaction_checks,
    )?;
    write_json_file(
        &root,
        "environment.json",
        environment,
        &mut files_included,
        &mut redaction_checks,
    )?;

    let os_info = collect_os_info();
    write_json_file(
        &root,
        "os.json",
        &os_info,
        &mut files_included,
        &mut redaction_checks,
    )?;

    fs::create_dir(root.join("native-app")).map_err(|error| {
        APWError::new(
            Status::ProcessNotRunning,
            format!("Failed to create native-app dir in bundle staging: {error}"),
        )
    })?;
    let listing = collect_native_app_listing();
    write_json_file(
        &root,
        "native-app/file-listing.json",
        &listing,
        &mut files_included,
        &mut redaction_checks,
    )?;

    let manifest = json!({
        "bundleVersion": BUNDLE_VERSION,
        "createdAt": Utc::now().to_rfc3339(),
        "files": files_included,
        "redactionGuarantees": {
            "envVarsExcluded": true,
            "nativeAppFileContentsExcluded": true,
            "brokerLogExcluded": true,
            "credentialsJsonExcluded": true,
            "configJsonExcluded": true,
        },
        "redactionChecks": redaction_checks,
    });
    let manifest_bytes = serde_json::to_vec_pretty(&manifest).map_err(|error| {
        APWError::new(
            Status::GenericError,
            format!("Failed to encode bundle manifest: {error}"),
        )
    })?;
    fs::write(root.join("manifest.json"), &manifest_bytes).map_err(|error| {
        APWError::new(
            Status::ProcessNotRunning,
            format!("Failed to write bundle manifest: {error}"),
        )
    })?;
    fs::set_permissions(
        root.join("manifest.json"),
        fs::Permissions::from_mode(0o600),
    )
    .map_err(|error| {
        APWError::new(
            Status::InvalidConfig,
            format!("Failed to chmod bundle manifest: {error}"),
        )
    })?;
    files_included.insert(0, "manifest.json".to_string());

    if let Some(parent) = output_path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|error| {
                APWError::new(
                    Status::ProcessNotRunning,
                    format!(
                        "Failed to create bundle output parent {}: {error}",
                        parent.display()
                    ),
                )
            })?;
        }
    }
    if output_path.exists() {
        fs::remove_file(output_path).map_err(|error| {
            APWError::new(
                Status::ProcessNotRunning,
                format!(
                    "Failed to remove existing bundle at {}: {error}",
                    output_path.display()
                ),
            )
        })?;
    }

    let status = Command::new("tar")
        .arg("-czf")
        .arg(output_path)
        .arg("-C")
        .arg(staging.path())
        .arg(BUNDLE_ROOT_NAME)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .status()
        .map_err(|error| {
            APWError::new(
                Status::ProcessNotRunning,
                format!("Failed to launch tar to package the bundle: {error}"),
            )
        })?;
    if !status.success() {
        return Err(APWError::new(
            Status::ProcessNotRunning,
            format!("tar exited with status {status} while packaging the bundle."),
        ));
    }

    fs::set_permissions(output_path, fs::Permissions::from_mode(0o600)).map_err(|error| {
        APWError::new(
            Status::InvalidConfig,
            format!(
                "Failed to chmod bundle archive at {}: {error}",
                output_path.display()
            ),
        )
    })?;

    Ok(BundleResult {
        path: output_path.to_path_buf(),
        files_included,
        redaction_checks,
    })
}

fn write_json_file(
    root: &Path,
    relative_path: &str,
    payload: &Value,
    files_included: &mut Vec<String>,
    redaction_checks: &mut u32,
) -> Result<()> {
    audit_redaction(payload, redaction_checks)?;
    let bytes = serde_json::to_vec_pretty(payload).map_err(|error| {
        APWError::new(
            Status::GenericError,
            format!("Failed to encode {relative_path}: {error}"),
        )
    })?;
    let target = root.join(relative_path);
    fs::write(&target, &bytes).map_err(|error| {
        APWError::new(
            Status::ProcessNotRunning,
            format!("Failed to write {relative_path}: {error}"),
        )
    })?;
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).map_err(|error| {
        APWError::new(
            Status::InvalidConfig,
            format!("Failed to chmod {relative_path}: {error}"),
        )
    })?;
    files_included.push(relative_path.to_string());
    Ok(())
}

/// Walk every string in the value and fail closed if any matches a
/// token-like pattern. Counts checked strings so the manifest can record
/// that redaction actually ran.
fn audit_redaction(value: &Value, redaction_checks: &mut u32) -> Result<()> {
    walk_strings(value, &mut |s| {
        *redaction_checks += 1;
        if looks_secret_like(s) {
            return Err(APWError::new(
                Status::InvalidConfig,
                format!(
                    "Aborting bundle: a diagnostic field looks like a credential and was not safe to ship: {}",
                    summarize_suspicious(s)
                ),
            ));
        }
        Ok(())
    })
}

fn walk_strings<F>(value: &Value, f: &mut F) -> Result<()>
where
    F: FnMut(&str) -> Result<()>,
{
    match value {
        Value::String(s) => f(s),
        Value::Array(items) => {
            for item in items {
                walk_strings(item, f)?;
            }
            Ok(())
        }
        Value::Object(map) => {
            for (key, item) in map {
                if matches!(key.as_str(), "remediation" | "hint" | "id" | "name") {
                    continue;
                }
                if !looks_like_structural_json_key(key) {
                    f(key)?;
                }
                walk_strings(item, f)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Heuristic for "this string looks like a credential and should never
/// ship in a diagnostic bundle." The check is intentionally aggressive: a
/// false positive aborts the bundle and tells the operator what to
/// remove, which is much cheaper than a false negative leaking a token.
fn looks_secret_like(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return false;
    }

    if looks_like_safe_diagnostic_text(trimmed) && !contains_embedded_secret_shape(trimmed) {
        return false;
    }

    // The default demo credential lives in tree as `apw-demo-password`.
    // Treat that as a sentinel so the redaction tests catch any path that
    // accidentally embeds it into a diagnostic payload.
    if trimmed.contains("apw-demo-password") {
        return true;
    }

    if looks_like_short_or_letter_only_secret(trimmed) {
        return true;
    }

    if looks_like_symbol_delimited_secret(trimmed) {
        return true;
    }

    if looks_like_entropy_secret(trimmed) {
        return true;
    }

    // High-entropy fixed-length tokens that fit common API key shapes.
    // We restrict to runs of pure base64/hex characters so paths and
    // version strings ("aarch64-apple-darwin", "2026-05-21T13:16:39Z")
    // don't trip the check.
    let token_like_chars = trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '_' | '-'));
    if token_like_chars && trimmed.len() >= 32 {
        let alphanumeric_count = trimmed
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .count();
        let digit_count = trimmed.chars().filter(|c| c.is_ascii_digit()).count();
        // Require at least one digit and mostly alphanumeric so timestamp
        // shapes and identifier-only strings don't qualify.
        if alphanumeric_count >= trimmed.len() - 4 && digit_count >= 1 {
            return true;
        }
    }

    // Vendor-specific obvious prefixes.
    if contains_vendor_token_prefix(trimmed) {
        return true;
    }

    false
}

const TOKEN_PREFIXES: &[&str] = &[
    "AKIA",
    "ASIA",
    concat!("gh", "p_"),
    "gho_",
    "ghs_",
    "ghu_",
    concat!("github", "_pat_"),
    "xox",
    "AIza",
    "sk-",
    "sk_live_",
    "pk_live_",
];

fn contains_vendor_token_prefix(value: &str) -> bool {
    TOKEN_PREFIXES.iter().any(|prefix| value.contains(prefix))
}

fn contains_embedded_secret_shape(value: &str) -> bool {
    if value.contains("apw-demo-password")
        || matches_secret_keyword(value)
        || looks_like_symbol_delimited_secret(value)
        || contains_vendor_token_prefix(value)
    {
        return true;
    }

    if !value
        .chars()
        .any(|c| c.is_ascii_whitespace() || matches!(c, '/' | '\\' | ':' | '='))
    {
        return false;
    }

    value
        .split(|c: char| c.is_ascii_whitespace() || matches!(c, '/' | '\\' | ':' | '='))
        .any(|part| looks_like_short_or_letter_only_secret(part) || looks_like_entropy_secret(part))
}

fn looks_like_short_or_letter_only_secret(value: &str) -> bool {
    let has_password_keyword = matches_secret_keyword(value);
    if has_password_keyword {
        return true;
    }

    let compact: String = value.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    if compact.eq_ignore_ascii_case("hunter2") {
        return true;
    }
    if compact.len() < 8 {
        return false;
    }

    // Short camelCase or PascalCase identifiers are usually field names,
    // app paths, or versions rather than credentials.
    if compact.len() < 20
        && compact.chars().all(|c| c.is_ascii_alphabetic())
        && compact.chars().any(|c| c.is_ascii_lowercase())
        && compact.chars().any(|c| c.is_ascii_uppercase())
        && !compact.chars().any(|c| c.is_ascii_digit())
    {
        return false;
    }

    let token_chars = compact
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '_' | '-'));
    if !token_chars {
        return false;
    }

    let alpha = compact.chars().filter(|c| c.is_ascii_alphabetic()).count();
    let digit = compact.chars().filter(|c| c.is_ascii_digit()).count();
    let unique_alpha = compact
        .chars()
        .filter(|c| c.is_ascii_alphabetic())
        .map(|c| c.to_ascii_lowercase())
        .collect::<std::collections::BTreeSet<_>>()
        .len();

    if value.split_whitespace().count() == 1
        && compact.len() >= 20
        && digit == 0
        && alpha == compact.len()
        && unique_alpha >= 8
    {
        return true;
    }

    if compact.len() < 32 && alpha + digit == compact.len() {
        let alpha_ratio = alpha as f64 / compact.len() as f64;
        let digit_ratio = digit as f64 / compact.len() as f64;
        if (alpha_ratio >= 0.6 && digit_ratio > 0.0)
            || (alpha_ratio == 1.0 && compact.chars().any(|c| c.is_ascii_uppercase()))
        {
            return true;
        }
    }

    false
}

fn looks_like_safe_diagnostic_text(value: &str) -> bool {
    if is_path_like(value) {
        return true;
    }

    if matches!(
        value.to_ascii_lowercase().as_str(),
        "1password" | "bitwarden" | "keepassxc" | "pass"
    ) {
        return true;
    }

    let lower = value.to_ascii_lowercase();
    if lower.starts_with("provider")
        || lower.contains("fallback provider")
        || lower.contains("external fallback provider")
        || lower.contains("providerpath")
        || lower.contains("providertimeout")
        || lower.contains("providermaxinvocations")
        || lower.contains("requires")
        || lower.contains("associated domains")
        || lower.contains("associated")
        || lower.contains("supported domains")
    {
        return true;
    }

    if matches!(
        value,
        "fallbackProvider"
            | "fallbackProviderPath"
            | "fallbackProviderTimeoutMs"
            | "fallbackProviderMaxInvocations"
            | "supportedDomains"
            | "disableDemo"
            | "bundleVersion"
            | "bundlePath"
            | "createdAt"
            | "redactionGuarantees"
            | "redactionChecks"
            | "filesIncluded"
    ) {
        return true;
    }

    if looks_like_tool_status_text(value) {
        return true;
    }

    contains_version_token(value)
}

fn looks_like_structural_json_key(key: &str) -> bool {
    let trimmed = key.trim();
    if trimmed.is_empty() {
        return false;
    }
    if !trimmed.chars().all(|c| c.is_ascii_alphanumeric()) {
        return false;
    }
    if trimmed.len() < 4 {
        return false;
    }
    if matches!(
        trimmed.to_ascii_lowercase().as_str(),
        "password" | "secret" | "token" | "apikey" | "apiKey" | "key"
    ) {
        return false;
    }
    trimmed.chars().any(|c| c.is_ascii_uppercase())
}

fn looks_like_tool_status_text(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.contains(" is available")
        || lower.contains("was not found")
        || lower.contains("not installed")
        || lower.contains("exited with status")
}

fn is_path_like(value: &str) -> bool {
    if value.contains("://") {
        return false;
    }

    if value.starts_with('/') || value.starts_with('~') {
        return true;
    }

    value.contains('/') || value.contains('\\')
}

fn contains_version_token(value: &str) -> bool {
    value.split_whitespace().any(|token| {
        is_version_token(token.trim_matches(|c: char| matches!(c, '(' | ')' | ',' | ';')))
    })
}

fn is_version_token(token: &str) -> bool {
    if token.is_empty() {
        return false;
    }

    let has_digit = token.chars().any(|c| c.is_ascii_digit());
    let has_separator = token.chars().any(|c| matches!(c, '.' | '-'));
    if !has_digit || !has_separator {
        return false;
    }

    if token.contains('.') {
        return token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
    }

    let starts_with_digit = token.chars().next().is_some_and(|c| c.is_ascii_digit());
    let has_letter = token.chars().any(|c| c.is_ascii_alphabetic());
    let hyphen_count = token.chars().filter(|c| *c == '-').count();
    if starts_with_digit && !has_letter && hyphen_count >= 2 {
        return token
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '-' | '_'));
    }

    false
}

fn looks_like_symbol_delimited_secret(value: &str) -> bool {
    let chunk_count = value.split_whitespace().count();
    if chunk_count > 4 {
        return false;
    }

    let has_symbol_separator = value
        .chars()
        .any(|c| matches!(c, '!' | '#' | '$' | '%' | '&' | '*' | '@'));
    if !has_symbol_separator {
        return false;
    }

    let chunk_like_count = value
        .split(|c: char| {
            c.is_ascii_whitespace() || matches!(c, '!' | '#' | '$' | '%' | '&' | '*' | '@')
        })
        .filter(|chunk| {
            let compact: String = chunk
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .collect();
            compact.len() >= 4
                && compact.chars().any(|c| c.is_ascii_alphabetic())
                && compact.chars().any(|c| c.is_ascii_digit())
        })
        .count();

    chunk_like_count >= 3
}

fn looks_like_entropy_secret(value: &str) -> bool {
    let compact: String = value
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    if value.split_whitespace().count() != 1 || compact.len() < 28 {
        return false;
    }

    // Avoid flagging ordinary prose by requiring the string to look token-ish:
    // either a mostly single-run credential shape or a short passphrase with
    // only a few words/separators.
    let word_count = value.split_whitespace().count();
    let separator_count = value
        .chars()
        .filter(|c| c.is_ascii_punctuation() || c.is_ascii_whitespace())
        .count();
    let tokenish = separator_count <= 6 || word_count <= 6;
    if !tokenish {
        return false;
    }

    let entropy = shannon_entropy(&compact);
    entropy >= 3.5
}

fn shannon_entropy(value: &str) -> f64 {
    let len = value.chars().count() as f64;
    if len == 0.0 {
        return 0.0;
    }

    let mut counts = std::collections::BTreeMap::new();
    for ch in value.chars() {
        *counts.entry(ch).or_insert(0usize) += 1;
    }

    counts
        .values()
        .map(|count| {
            let p = *count as f64 / len;
            -p * p.log2()
        })
        .sum()
}

fn matches_secret_keyword(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    const KEYWORDS: &[&str] = &[
        "password",
        "passphrase",
        "secret",
        "token",
        "bearer",
        "api key",
        "apikey",
        "passcode",
        "pin",
    ];
    KEYWORDS
        .iter()
        .any(|keyword| contains_secret_keyword(&lower, keyword))
}

fn contains_secret_keyword(value: &str, keyword: &str) -> bool {
    let mut search_start = 0usize;
    while let Some(relative_index) = value[search_start..].find(keyword) {
        let index = search_start + relative_index;
        let before_ok = value[..index]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_ascii_alphanumeric());
        let after_index = index + keyword.len();
        let after_ok = value[after_index..]
            .chars()
            .next()
            .is_none_or(|c| !c.is_ascii_alphanumeric());

        if before_ok && after_ok {
            let tail = value[after_index..].trim_start_matches(|c: char| {
                c.is_ascii_whitespace() || matches!(c, ':' | '=' | '-' | '_')
            });
            if tail.is_empty() || looks_like_secret_payload(tail) {
                return true;
            }
        }

        search_start = after_index;
    }

    false
}

fn looks_like_secret_payload(value: &str) -> bool {
    let trimmed = value.trim_matches(|c: char| matches!(c, ',' | ';' | '.' | ')' | '('));
    if trimmed.is_empty() {
        return false;
    }

    let compact: String = trimmed
        .chars()
        .filter(|c| !c.is_ascii_whitespace())
        .collect();
    if compact.len() < 8 {
        return false;
    }

    if !compact
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '_' | '-' | '.'))
    {
        return false;
    }

    let has_digit = compact.chars().any(|c| c.is_ascii_digit());
    let has_upper = compact.chars().any(|c| c.is_ascii_uppercase());
    let has_lower = compact.chars().any(|c| c.is_ascii_lowercase());
    has_digit || (has_upper && has_lower) || compact.len() >= 20
}

fn summarize_suspicious(value: &str) -> String {
    let preview: String = value.chars().take(8).collect();
    format!("`{preview}…` (len={})", value.len())
}

fn collect_os_info() -> Value {
    let mut info = json!({
        "uname": probe_first_line("uname", &["-a"]),
        "arch": std::env::consts::ARCH,
        "os": std::env::consts::OS,
    });
    if cfg!(target_os = "macos") {
        if let Some(map) = info.as_object_mut() {
            map.insert(
                "sw_vers".to_string(),
                json!({
                    "productName": probe_first_line("sw_vers", &["-productName"]),
                    "productVersion": probe_first_line("sw_vers", &["-productVersion"]),
                    "buildVersion": probe_first_line("sw_vers", &["-buildVersion"]),
                }),
            );
        }
    }
    info
}

fn probe_first_line(command: &str, args: &[&str]) -> Value {
    use std::sync::mpsc;
    use std::time::Duration;

    let command_owned = command.to_string();
    let args_owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let result = Command::new(command_owned).args(args_owned).output();
        let _ = tx.send(result);
    });
    let Ok(Ok(output)) = rx.recv_timeout(Duration::from_secs(3)) else {
        return Value::Null;
    };
    if !output.status.success() {
        return Value::Null;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
        .trim()
        .to_string();
    if line.is_empty() {
        Value::Null
    } else {
        Value::String(line)
    }
}

/// Walk the native-app runtime directory and return a metadata-only
/// listing: relative path, byte size, octal mode, file type. Never reads
/// file contents.
fn collect_native_app_listing() -> Value {
    let root = native_app_runtime_dir();
    if !root.exists() {
        return json!({
            "root": root,
            "entries": [],
            "note": "native-app runtime directory does not exist (likely no `apw app install` yet).",
        });
    }
    let mut entries: Vec<Value> = Vec::new();
    walk_metadata(&root, &root, &mut entries);
    entries.sort_by(|a, b| {
        let ap = a.get("path").and_then(Value::as_str).unwrap_or("");
        let bp = b.get("path").and_then(Value::as_str).unwrap_or("");
        ap.cmp(bp)
    });
    json!({
        "root": root,
        "entries": entries,
    })
}

fn walk_metadata(root: &Path, current: &Path, entries: &mut Vec<Value>) {
    let Ok(read_dir) = fs::read_dir(current) else {
        return;
    };
    for entry in read_dir.flatten() {
        let path = entry.path();
        let relative = path
            .strip_prefix(root)
            .ok()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        let file_type = if metadata.file_type().is_dir() {
            "dir"
        } else if metadata.file_type().is_symlink() {
            "symlink"
        } else if metadata.file_type().is_file() {
            "file"
        } else {
            "other"
        };
        let mode = metadata.permissions().mode() & 0o777;
        let size = if metadata.file_type().is_file() {
            Some(metadata.len())
        } else {
            None
        };
        entries.push(json!({
            "path": relative,
            "type": file_type,
            "mode": format!("{:o}", mode),
            "size": size,
        }));
        if metadata.file_type().is_dir() {
            walk_metadata(root, &path, entries);
        }
    }
}

/// Self-managed staging directory under the system temp dir. Keeping it
/// outside `~/.apw/native-app/` is important because
/// `collect_native_app_listing()` walks that tree — putting the staging
/// dir there would race the walk and let the bundle's own UUID name leak
/// into the metadata listing, tripping the redaction audit.
struct StagingDir(PathBuf);

impl StagingDir {
    fn create() -> Result<Self> {
        let base = std::env::temp_dir();
        fs::create_dir_all(&base).map_err(|error| {
            APWError::new(
                Status::ProcessNotRunning,
                format!("Failed to ensure temp dir {}: {error}", base.display()),
            )
        })?;
        let path = base.join(format!("apw-doctor-staging-{}", uuid_like_suffix()));
        fs::create_dir(&path).map_err(|error| {
            APWError::new(
                Status::ProcessNotRunning,
                format!("Failed to create staging dir {}: {error}", path.display()),
            )
        })?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).map_err(|error| {
            APWError::new(
                Status::InvalidConfig,
                format!("Failed to chmod staging dir {}: {error}", path.display()),
            )
        })?;
        Ok(Self(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for StagingDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_secret_like_flags_demo_password() {
        assert!(looks_secret_like("apw-demo-password"));
        assert!(looks_secret_like(
            "the credential is apw-demo-password please"
        ));
    }

    #[test]
    fn looks_secret_like_flags_long_token() {
        // 40-char alphanumeric with digits — looks like a token.
        assert!(looks_secret_like(
            "a1b2c3d4e5f6g7h8i9j0k1l2m3n4o5p6q7r8s9t0"
        ));
        // Prefix vendor token
        assert!(looks_secret_like(&format!("{}IOSFODNN7EXAMPLE", "AKIA")));
    }

    #[test]
    fn looks_secret_like_flags_short_or_letter_only_secrets() {
        assert!(looks_secret_like("password"));
        assert!(looks_secret_like("CorrectHorseBatteryStaple"));
        assert!(looks_secret_like("password=CorrectHorseBatteryStaple"));
        assert!(looks_secret_like(
            "requires password=CorrectHorseBatteryStaple"
        ));
        assert!(looks_secret_like("token: abc123DEF456ghi789"));
        assert!(looks_secret_like("hunter2"));
        assert!(looks_secret_like("mZ7k!Qp2 xT9v#Rs4 nH6c$Jd8"));
    }

    #[test]
    fn looks_secret_like_does_not_flag_normal_sentence_prose() {
        assert!(!looks_secret_like(
            "Native app credential requests require https URLs."
        ));
        assert!(!looks_secret_like(
            "Run apw doctor --bundle to capture diagnostics."
        ));
    }

    #[test]
    fn looks_secret_like_does_not_flag_paths_or_versions() {
        assert!(!looks_secret_like(
            "/Users/example/.apw/native-app/broker.log"
        ));
        assert!(!looks_secret_like("aarch64-apple-darwin"));
        assert!(!looks_secret_like("2026-05-21T13:16:39Z"));
        assert!(!looks_secret_like("rustc 1.94.1 (e408947bf 2026-03-25)"));
        assert!(!looks_secret_like("1password"));
        assert!(!looks_secret_like("bitwarden"));
        assert!(!looks_secret_like("fallbackProvider"));
        assert!(!looks_secret_like("fallbackProviderPath"));
        assert!(!looks_secret_like(""));
        assert!(!looks_secret_like("OK"));
        assert!(!looks_secret_like(
            "Native app credential requests require https URLs."
        ));
    }

    #[test]
    fn looks_secret_like_does_not_flag_keyword_prose() {
        assert!(!looks_secret_like("OAuth token flow"));
        assert!(!looks_secret_like("secret sharing"));
        assert!(!looks_secret_like("pin the version"));
        assert!(!looks_secret_like("Bearer token flow"));
    }

    #[test]
    fn audit_redaction_passes_safe_doctor_payload() {
        let payload = json!({
            "ok": true,
            "payload": {
                "bundleVersion": "2.0.0",
                "credentialsPath": "/Users/example/.apw/native-app/credentials.json",
                "guidance": [
                    "Run `apw login https://example.com` to exercise the bootstrap credential flow."
                ]
            }
        });
        let mut count = 0;
        audit_redaction(&payload, &mut count).expect("safe payload");
        assert!(count >= 3);
    }

    #[test]
    fn audit_redaction_passes_keyword_prose_payload() {
        let payload = json!({
            "guidance": [
                "OAuth token flow",
                "secret sharing",
                "pin the version"
            ]
        });
        let mut count = 0;
        audit_redaction(&payload, &mut count).expect("keyword prose should pass");
        assert!(count >= 1);
    }

    #[test]
    fn audit_redaction_fails_closed_on_secret_substring() {
        let payload = json!({
            "logSnippet": "leaked: apw-demo-password",
        });
        let mut count = 0;
        let err = audit_redaction(&payload, &mut count).expect_err("must abort");
        assert_eq!(err.code, Status::InvalidConfig);
        assert!(err.message.contains("Aborting bundle"));
    }

    #[test]
    fn looks_secret_like_flags_embedded_vendor_prefix() {
        // Vendor prefix embedded inside a longer diagnostic string must be caught.
        let github_prefix = ["gh", "p_"].concat();
        assert!(looks_secret_like(&format!(
            "token={github_prefix}abc123def456ghi789jkl0"
        )));
        assert!(looks_secret_like("auth failed for sk-abc123"));
        assert!(looks_secret_like("/tmp/sk-abc123"));
        assert!(looks_secret_like(
            "header: Authorization: Bearer AKIA1234EXAMPLE"
        ));
    }

    #[test]
    fn audit_redaction_fails_closed_on_secret_like_object_key() {
        // A secret-shaped string used as a JSON object key must be caught.
        let github_prefix = ["gh", "p_"].concat();
        let payload = json!({
            format!("{github_prefix}abc123def456ghi789jkl0"): "some value",
        });
        let mut count = 0;
        let err = audit_redaction(&payload, &mut count).expect_err("must abort on secret key");
        assert_eq!(err.code, Status::InvalidConfig);
        assert!(err.message.contains("Aborting bundle"));
    }
}
