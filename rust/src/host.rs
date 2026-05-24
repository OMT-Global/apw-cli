use crate::error::{APWError, Result};
use crate::types::{RuntimeMode, Status};
use serde_json::{json, Value};
use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const NATIVE_HOST_LABEL: &str = "dev.omt.apw.nativehost";
const NATIVE_HOST_BUNDLE_NAME: &str = "APWNativeHost.app";
const NATIVE_HOST_EXECUTABLE_NAME: &str = "APWNativeHost";
const HELPER_BINARY_PATH: &str = "/System/Cryptexes/App/System/Library/CoreServices/PasswordManagerBrowserExtensionHelper.app/Contents/MacOS/PasswordManagerBrowserExtensionHelper";

fn home_dir() -> PathBuf {
    PathBuf::from(
        env::var("HOME")
            .unwrap_or_else(|_| env::var("USERPROFILE").unwrap_or_else(|_| ".".to_string())),
    )
}

fn set_permissions(path: &Path, mode: u32) {
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode));
}

fn native_host_supported_platform() -> bool {
    #[cfg(test)]
    if let Some(value) = test_support::supported_platform_override() {
        return value;
    }

    cfg!(target_os = "macos")
}

pub fn native_host_runtime_supported() -> bool {
    native_host_supported_platform()
}

#[cfg(test)]
pub fn set_native_host_preflight_overrides_for_tests(
    supported_platform: Option<bool>,
    launch_agent_loaded: Option<bool>,
    helper_executable: Option<bool>,
    macos_major_version: Option<u32>,
) {
    test_support::mutate(|state| {
        state.supported_platform = supported_platform;
        state.launch_agent_loaded = launch_agent_loaded;
        state.helper_executable = helper_executable;
        state.macos_major_version = macos_major_version;
    });
}

#[cfg(test)]
pub fn clear_native_host_test_overrides() {
    test_support::reset();
}

pub fn native_host_run_dir() -> PathBuf {
    home_dir().join(".apw").join("run")
}

pub fn native_host_socket_path() -> PathBuf {
    native_host_run_dir().join("native-host.sock")
}

pub fn native_host_install_dir() -> PathBuf {
    home_dir()
        .join("Library")
        .join("Application Support")
        .join("APW")
        .join("NativeHost")
}

pub fn native_host_bundle_install_path() -> PathBuf {
    native_host_install_dir().join(NATIVE_HOST_BUNDLE_NAME)
}

pub fn native_host_launch_agents_dir() -> PathBuf {
    home_dir().join("Library").join("LaunchAgents")
}

pub fn native_host_launch_agent_path() -> PathBuf {
    native_host_launch_agents_dir().join(format!("{NATIVE_HOST_LABEL}.plist"))
}

pub fn native_host_helper_path() -> PathBuf {
    PathBuf::from(HELPER_BINARY_PATH)
}

pub fn native_host_executable_in_bundle(bundle_path: &Path) -> PathBuf {
    bundle_path
        .join("Contents")
        .join("MacOS")
        .join(NATIVE_HOST_EXECUTABLE_NAME)
}

pub fn ensure_native_host_runtime_dir() -> Result<()> {
    let run_dir = native_host_run_dir();
    fs::create_dir_all(&run_dir).map_err(|error| {
        APWError::new(
            Status::InvalidConfig,
            format!("Failed to create native host runtime directory: {error}"),
        )
    })?;
    set_permissions(&run_dir, 0o700);
    Ok(())
}

fn current_uid() -> u32 {
    #[cfg(unix)]
    {
        unsafe { libc::geteuid() as u32 }
    }
    #[cfg(not(unix))]
    {
        0
    }
}

fn launchctl_domain() -> String {
    format!("gui/{}", current_uid())
}

fn launchctl_service_target() -> String {
    format!("{}/{}", launchctl_domain(), NATIVE_HOST_LABEL)
}

fn run_launchctl(args: &[&str], allow_failure: bool) -> Result<String> {
    #[cfg(test)]
    if let Some(result) = test_support::run_launchctl(args, allow_failure) {
        return result;
    }

    let output = Command::new("/bin/launchctl")
        .args(args)
        .output()
        .map_err(|error| {
            APWError::new(
                Status::ProcessNotRunning,
                format!("Failed to execute launchctl: {error}"),
            )
        })?;

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

    if output.status.success() || allow_failure {
        return Ok(if stdout.is_empty() { stderr } else { stdout });
    }

    Err(APWError::new(
        Status::ProcessNotRunning,
        if stderr.is_empty() {
            "launchctl command failed.".to_string()
        } else {
            stderr
        },
    ))
}

fn launch_agent_loaded() -> bool {
    #[cfg(test)]
    if let Some(value) = test_support::launch_agent_loaded_override() {
        return value;
    }

    Command::new("/bin/launchctl")
        .args(["print", &launchctl_service_target()])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn read_bundle_version(bundle_path: &Path) -> Option<String> {
    let info_plist = bundle_path.join("Contents").join("Info.plist");
    let content = fs::read_to_string(info_plist).ok()?;
    let marker = "<key>CFBundleShortVersionString</key>";
    let start = content.find(marker)?;
    let rest = &content[start + marker.len()..];
    let string_start = rest.find("<string>")?;
    let rest = &rest[string_start + "<string>".len()..];
    let string_end = rest.find("</string>")?;
    Some(rest[..string_end].trim().to_string())
}

fn helper_executable() -> bool {
    #[cfg(test)]
    if let Some(value) = test_support::helper_executable_override() {
        return value;
    }

    fs::metadata(native_host_helper_path())
        .map(|metadata| metadata.is_file() && (metadata.permissions().mode() & 0o111 != 0))
        .unwrap_or(false)
}

fn native_host_macos_major_version() -> Option<u32> {
    #[cfg(test)]
    if let Some(value) = test_support::macos_major_version_override() {
        return Some(value);
    }

    if !native_host_supported_platform() {
        return None;
    }

    let output = Command::new("sw_vers")
        .arg("-productVersion")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let raw = String::from_utf8(output.stdout).ok()?;
    raw.trim()
        .split('.')
        .next()
        .and_then(|segment| segment.parse::<u32>().ok())
}

fn copy_dir_recursive(source: &Path, destination: &Path) -> Result<()> {
    if destination.exists() {
        fs::remove_dir_all(destination).map_err(|error| {
            APWError::new(
                Status::InvalidConfig,
                format!("Failed to replace native host bundle: {error}"),
            )
        })?;
    }

    fs::create_dir_all(destination).map_err(|error| {
        APWError::new(
            Status::InvalidConfig,
            format!("Failed to create native host destination directory: {error}"),
        )
    })?;

    for entry in fs::read_dir(source).map_err(|error| {
        APWError::new(
            Status::InvalidConfig,
            format!("Failed to enumerate native host bundle: {error}"),
        )
    })? {
        let entry = entry.map_err(|error| {
            APWError::new(
                Status::InvalidConfig,
                format!("Failed to enumerate native host bundle entry: {error}"),
            )
        })?;
        let entry_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if entry_path.is_dir() {
            copy_dir_recursive(&entry_path, &destination_path)?;
        } else {
            fs::copy(&entry_path, &destination_path).map_err(|error| {
                APWError::new(
                    Status::InvalidConfig,
                    format!("Failed to copy native host bundle file: {error}"),
                )
            })?;
        }
    }

    Ok(())
}

fn resolve_packaged_native_host_bundle() -> Result<PathBuf> {
    #[cfg(test)]
    if let Some(path) = test_support::packaged_bundle_override() {
        return Ok(path);
    }

    let current_exe = env::current_exe().map_err(|error| {
        APWError::new(
            Status::InvalidConfig,
            format!("Unable to resolve current executable: {error}"),
        )
    })?;
    let executable_dir = current_exe.parent().ok_or_else(|| {
        APWError::new(
            Status::InvalidConfig,
            "Unable to resolve executable directory.",
        )
    })?;

    let mut candidates = vec![
        executable_dir
            .join("../libexec")
            .join(NATIVE_HOST_BUNDLE_NAME),
        executable_dir
            .join("../../libexec")
            .join(NATIVE_HOST_BUNDLE_NAME),
        executable_dir
            .join("../../../native-host/dist")
            .join(NATIVE_HOST_BUNDLE_NAME),
    ];

    if let Ok(cwd) = env::current_dir() {
        candidates.push(cwd.join("native-host/dist").join(NATIVE_HOST_BUNDLE_NAME));
    }

    for candidate in candidates {
        if candidate.exists() {
            return Ok(candidate);
        }
    }

    Err(APWError::new(
        Status::InvalidConfig,
        "Packaged native host bundle not found. Build it with `./scripts/build-native-host.sh` or install APW via Homebrew first.",
    ))
}

fn build_launch_agent_plist(bundle_path: &Path, socket_path: &Path) -> String {
    let executable = native_host_executable_in_bundle(bundle_path);
    let stdout_log = native_host_install_dir().join("native-host.stdout.log");
    let stderr_log = native_host_install_dir().join("native-host.stderr.log");
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{label}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{executable}</string>
    <string>--socket-path</string>
    <string>{socket_path}</string>
    <string>--helper-path</string>
    <string>{helper_path}</string>
  </array>
  <key>KeepAlive</key>
  <true/>
  <key>RunAtLoad</key>
  <true/>
  <key>ProcessType</key>
  <string>Background</string>
  <key>StandardOutPath</key>
  <string>{stdout_log}</string>
  <key>StandardErrorPath</key>
  <string>{stderr_log}</string>
</dict>
</plist>
"#,
        label = NATIVE_HOST_LABEL,
        executable = executable.display(),
        socket_path = socket_path.display(),
        helper_path = native_host_helper_path().display(),
        stdout_log = stdout_log.display(),
        stderr_log = stderr_log.display(),
    )
}

fn native_host_preflight_state() -> (String, Option<String>, Option<String>) {
    if !native_host_supported_platform() {
        return (
            "unsupported_platform".to_string(),
            Some("APW native host is supported only on macOS.".to_string()),
            None,
        );
    }

    let bundle_path = native_host_bundle_install_path();
    let launch_agent_path = native_host_launch_agent_path();
    let bundle_version = read_bundle_version(&bundle_path);

    if !bundle_path.exists() {
        return (
            "app_missing".to_string(),
            Some("Native host app bundle is not installed. Run `apw host install`.".to_string()),
            bundle_version,
        );
    }

    if !launch_agent_path.exists() {
        return (
            "launch_agent_missing".to_string(),
            Some("Native host LaunchAgent is not installed. Run `apw host install`.".to_string()),
            bundle_version,
        );
    }

    if !launch_agent_loaded() {
        return (
            "launch_agent_unloaded".to_string(),
            Some("Native host LaunchAgent is not loaded. Run `apw host install` or `apw host doctor`.".to_string()),
            bundle_version,
        );
    }

    if !helper_executable() {
        return (
            "helper_missing".to_string(),
            Some(
                "Apple PasswordManagerBrowserExtensionHelper is not executable on this host."
                    .to_string(),
            ),
            bundle_version,
        );
    }

    ("ready".to_string(), None, bundle_version)
}

pub fn native_host_preflight_status(configured_mode: RuntimeMode) -> Value {
    let bundle_path = native_host_bundle_install_path();
    let launch_agent_path = native_host_launch_agent_path();
    let socket_path = native_host_socket_path();
    let helper_path = native_host_helper_path();
    let (status, error, bundle_version) = native_host_preflight_state();

    json!({
        "supported": native_host_supported_platform(),
        "platform": {
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "macosMajorVersion": native_host_macos_major_version(),
        },
        "configuredRuntimeMode": configured_mode,
        "resolvedRuntimeMode": RuntimeMode::Native,
        "launchStrategies": ["native_host"],
        "status": status,
        "socketPath": socket_path,
        "socketExists": socket_path.exists(),
        "launchAgent": {
            "loaded": launch_agent_loaded(),
            "path": launch_agent_path,
        },
        "appBundle": {
            "path": bundle_path,
            "version": bundle_version,
        },
        "helper": {
            "path": helper_path,
            "executable": helper_executable(),
        },
        "error": error,
    })
}

pub fn native_host_status_note() -> String {
    let preflight = native_host_preflight_status(RuntimeMode::Native);
    let status = preflight
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    format!(
        "Run `apw host doctor` or `apw status --json` and inspect `daemon.preflight`; current `daemon.preflight.status={status}`."
    )
}

pub fn native_host_failure_message(base_message: &str) -> String {
    let preflight = native_host_preflight_status(RuntimeMode::Native);
    let status = preflight
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("unknown");

    let guidance = match status {
        "app_missing" | "launch_agent_missing" | "launch_agent_unloaded" => {
            "Run `apw host install`, then `apw host doctor`; use `apw app launch` for the supported v2 broker."
        }
        "helper_missing" => {
            "The Apple helper is unavailable on this host; run `apw host doctor` for details."
        }
        "ready" => {
            "Run `apw host doctor`; use `apw app launch` for the supported v2 broker."
        }
        _ => "Run `apw host doctor` for native host diagnostics.",
    };

    format!("{base_message} {guidance} Current `daemon.preflight.status={status}`.")
}

pub fn native_host_doctor() -> Result<Value> {
    Ok(native_host_preflight_status(RuntimeMode::Native))
}

pub fn native_host_install() -> Result<Value> {
    if !native_host_supported_platform() {
        return Err(APWError::new(
            Status::ProcessNotRunning,
            "APW native host is supported only on macOS.",
        ));
    }

    ensure_native_host_runtime_dir()?;

    let source_bundle = resolve_packaged_native_host_bundle()?;
    let install_dir = native_host_install_dir();
    fs::create_dir_all(&install_dir).map_err(|error| {
        APWError::new(
            Status::InvalidConfig,
            format!("Failed to create native host install directory: {error}"),
        )
    })?;
    set_permissions(&install_dir, 0o700);

    let launch_agents_dir = native_host_launch_agents_dir();
    fs::create_dir_all(&launch_agents_dir).map_err(|error| {
        APWError::new(
            Status::InvalidConfig,
            format!("Failed to create LaunchAgents directory: {error}"),
        )
    })?;

    let installed_bundle = native_host_bundle_install_path();
    copy_dir_recursive(&source_bundle, &installed_bundle)?;

    let plist = build_launch_agent_plist(&installed_bundle, &native_host_socket_path());
    fs::write(native_host_launch_agent_path(), plist).map_err(|error| {
        APWError::new(
            Status::InvalidConfig,
            format!("Failed to write LaunchAgent plist: {error}"),
        )
    })?;
    set_permissions(&native_host_launch_agent_path(), 0o644);

    let launch_agent_path = native_host_launch_agent_path();
    let launch_agent_path_text = launch_agent_path.to_string_lossy().to_string();
    let domain = launchctl_domain();
    let _ = run_launchctl(&["bootout", &domain, &launch_agent_path_text], true);
    run_launchctl(&["bootstrap", &domain, &launch_agent_path_text], false)?;
    let _ = run_launchctl(&["kickstart", "-k", &launchctl_service_target()], true);

    Ok(json!({
        "status": "installed",
        "paths": {
            "bundle": installed_bundle,
            "launchAgent": native_host_launch_agent_path(),
            "socket": native_host_socket_path(),
        },
        "preflight": native_host_preflight_status(RuntimeMode::Native),
    }))
}

pub fn native_host_uninstall() -> Result<Value> {
    if !native_host_supported_platform() {
        return Err(APWError::new(
            Status::ProcessNotRunning,
            "APW native host is supported only on macOS.",
        ));
    }

    let launch_agent_path = native_host_launch_agent_path();
    let launch_agent_path_text = launch_agent_path.to_string_lossy().to_string();
    let domain = launchctl_domain();
    let _ = run_launchctl(&["bootout", &domain, &launch_agent_path_text], true);

    if launch_agent_path.exists() {
        let _ = fs::remove_file(&launch_agent_path);
    }
    let bundle_path = native_host_bundle_install_path();
    if bundle_path.exists() {
        let _ = fs::remove_dir_all(&bundle_path);
    }
    let socket_path = native_host_socket_path();
    if socket_path.exists() {
        let _ = fs::remove_file(&socket_path);
    }

    Ok(json!({
        "status": "uninstalled",
        "paths": {
            "bundle": bundle_path,
            "launchAgent": launch_agent_path,
            "socket": socket_path,
        }
    }))
}

#[cfg(test)]
mod test_support {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    #[derive(Clone, Default)]
    pub struct State {
        pub supported_platform: Option<bool>,
        pub packaged_bundle: Option<PathBuf>,
        pub launch_agent_loaded: Option<bool>,
        pub helper_executable: Option<bool>,
        pub macos_major_version: Option<u32>,
        pub stub_launchctl: bool,
        pub launchctl_error: Option<String>,
        pub launchctl_calls: Vec<Vec<String>>,
    }

    static STATE: OnceLock<Mutex<State>> = OnceLock::new();

    fn state() -> &'static Mutex<State> {
        STATE.get_or_init(|| Mutex::new(State::default()))
    }

    pub fn mutate<F>(mutator: F)
    where
        F: FnOnce(&mut State),
    {
        let mut guard = state().lock().unwrap();
        mutator(&mut guard);
    }

    pub fn replace(new_state: State) {
        *state().lock().unwrap() = new_state;
    }

    pub fn reset() {
        replace(State::default());
    }

    pub fn supported_platform_override() -> Option<bool> {
        state().lock().unwrap().supported_platform
    }

    pub fn packaged_bundle_override() -> Option<PathBuf> {
        state().lock().unwrap().packaged_bundle.clone()
    }

    pub fn launch_agent_loaded_override() -> Option<bool> {
        state().lock().unwrap().launch_agent_loaded
    }

    pub fn helper_executable_override() -> Option<bool> {
        state().lock().unwrap().helper_executable
    }

    pub fn macos_major_version_override() -> Option<u32> {
        state().lock().unwrap().macos_major_version
    }

    pub fn launchctl_calls() -> Vec<Vec<String>> {
        state().lock().unwrap().launchctl_calls.clone()
    }

    pub fn run_launchctl(args: &[&str], allow_failure: bool) -> Option<Result<String>> {
        let mut guard = state().lock().unwrap();
        if !guard.stub_launchctl {
            return None;
        }

        guard
            .launchctl_calls
            .push(args.iter().map(|value| value.to_string()).collect());

        if let Some(error) = guard.launchctl_error.clone() {
            if allow_failure {
                return Some(Ok(error));
            }
            return Some(Err(APWError::new(Status::ProcessNotRunning, error)));
        }

        Some(Ok(String::new()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::sync::Mutex;
    use tempfile::tempdir;

    static TEST_HOME_LOCK: Mutex<()> = Mutex::new(());

    fn with_temp_home<F, R>(run: F) -> R
    where
        F: FnOnce(&Path) -> R,
    {
        let _guard = TEST_HOME_LOCK.lock().unwrap();
        let temp = tempdir().unwrap();
        let previous_home = env::var("HOME").ok();

        unsafe {
            env::set_var("HOME", temp.path());
        }
        test_support::reset();

        let result = run(temp.path());

        test_support::reset();
        if let Some(value) = previous_home {
            unsafe {
                env::set_var("HOME", value);
            }
        } else {
            unsafe {
                env::remove_var("HOME");
            }
        }

        result
    }

    fn write_file(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    fn make_fake_bundle(root: &Path) -> PathBuf {
        let bundle = root.join("packaged").join(NATIVE_HOST_BUNDLE_NAME);
        let executable = native_host_executable_in_bundle(&bundle);
        write_file(
            &bundle.join("Contents").join("Info.plist"),
            r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
  <key>CFBundleShortVersionString</key>
  <string>2.0.0</string>
</dict>
</plist>
"#,
        );
        write_file(&executable, "#!/bin/sh\nexit 0\n");
        set_permissions(&executable, 0o755);
        bundle
    }

    #[test]
    #[serial]
    fn native_host_doctor_reports_unsupported_platform() {
        with_temp_home(|_| {
            test_support::replace(test_support::State {
                supported_platform: Some(false),
                ..Default::default()
            });

            let payload = native_host_doctor().unwrap();
            assert_eq!(payload["status"], "unsupported_platform");
            assert_eq!(
                payload["error"],
                "APW native host is supported only on macOS."
            );
            assert_eq!(payload["supported"], false);
        });
    }

    #[test]
    #[serial]
    fn native_host_install_copies_bundle_and_writes_launch_agent() {
        with_temp_home(|home| {
            let source_bundle = make_fake_bundle(home);
            test_support::replace(test_support::State {
                supported_platform: Some(true),
                packaged_bundle: Some(source_bundle),
                launch_agent_loaded: Some(true),
                helper_executable: Some(true),
                macos_major_version: Some(26),
                stub_launchctl: true,
                ..Default::default()
            });

            let payload = native_host_install().unwrap();
            let installed_bundle = native_host_bundle_install_path();
            let launch_agent = native_host_launch_agent_path();
            let socket_path = native_host_socket_path();

            assert_eq!(payload["status"], "installed");
            assert_eq!(payload["preflight"]["status"], "ready");
            assert_eq!(payload["preflight"]["appBundle"]["version"], "2.0.0");
            assert!(installed_bundle.exists());
            assert!(native_host_executable_in_bundle(&installed_bundle).exists());
            assert!(launch_agent.exists());
            assert_eq!(
                fs::metadata(native_host_install_dir())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(&launch_agent).unwrap().permissions().mode() & 0o777,
                0o644
            );

            let plist = fs::read_to_string(&launch_agent).unwrap();
            assert!(plist.contains("--socket-path"));
            assert!(plist.contains(socket_path.to_string_lossy().as_ref()));
            assert!(plist.contains(NATIVE_HOST_LABEL));

            let launchctl_calls = test_support::launchctl_calls();
            assert!(launchctl_calls
                .iter()
                .any(|args| args.first().map(String::as_str) == Some("bootstrap")));
            assert!(launchctl_calls
                .iter()
                .any(|args| args.first().map(String::as_str) == Some("kickstart")));
        });
    }

    #[test]
    #[serial]
    fn native_host_uninstall_removes_installed_artifacts() {
        with_temp_home(|home| {
            let bundle = native_host_bundle_install_path();
            let executable = native_host_executable_in_bundle(&bundle);
            let launch_agent = native_host_launch_agent_path();
            let socket_path = native_host_socket_path();

            write_file(
                &bundle.join("Contents").join("Info.plist"),
                "<plist version=\"1.0\"></plist>\n",
            );
            write_file(&executable, "#!/bin/sh\nexit 0\n");
            write_file(&launch_agent, "<plist version=\"1.0\"></plist>\n");
            write_file(&socket_path, "socket");
            assert!(home.exists());

            test_support::replace(test_support::State {
                supported_platform: Some(true),
                stub_launchctl: true,
                ..Default::default()
            });

            let payload = native_host_uninstall().unwrap();
            assert_eq!(payload["status"], "uninstalled");
            assert!(!bundle.exists());
            assert!(!launch_agent.exists());
            assert!(!socket_path.exists());
            assert!(test_support::launchctl_calls()
                .iter()
                .any(|args| args.first().map(String::as_str) == Some("bootout")));
        });
    }

    #[test]
    #[serial]
    fn native_host_failure_message_guides_install_for_missing_bundle() {
        with_temp_home(|_| {
            test_support::replace(test_support::State {
                supported_platform: Some(true),
                stub_launchctl: true,
                ..Default::default()
            });

            let message = native_host_failure_message("Base failure.");
            assert!(message
                .contains("Run `apw host install`, then `apw host doctor`; use `apw app launch`"));
            assert!(message.contains("daemon.preflight.status=app_missing"));
        });
    }
}
