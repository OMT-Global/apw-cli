use chrono::Utc;
use clap::ValueEnum;
use std::sync::OnceLock;

#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, ValueEnum)]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

static LOG_LEVEL: OnceLock<LogLevel> = OnceLock::new();
static MACHINE_READABLE_OUTPUT: OnceLock<bool> = OnceLock::new();

pub fn init(level: LogLevel, machine_readable_output: bool) {
    let _ = LOG_LEVEL.set(level);
    let _ = MACHINE_READABLE_OUTPUT.set(machine_readable_output);
}

fn enabled(level: LogLevel) -> bool {
    level >= *LOG_LEVEL.get().unwrap_or(&LogLevel::Warn)
}

pub fn machine_readable_output() -> bool {
    *MACHINE_READABLE_OUTPUT.get().unwrap_or(&false)
}

fn emit(level: LogLevel, component: &str, message: impl AsRef<str>) {
    if machine_readable_output() || !enabled(level) {
        return;
    }

    let level_name = match level {
        LogLevel::Debug => "DEBUG",
        LogLevel::Info => "INFO",
        LogLevel::Warn => "WARN",
        LogLevel::Error => "ERROR",
    };
    eprintln!(
        "[{level_name}] {} {component}: {}",
        Utc::now().to_rfc3339(),
        message.as_ref()
    );
}

pub fn debug(component: &str, message: impl AsRef<str>) {
    emit(LogLevel::Debug, component, message);
}

pub fn info(component: &str, message: impl AsRef<str>) {
    emit(LogLevel::Info, component, message);
}

pub fn warn(component: &str, message: impl AsRef<str>) {
    emit(LogLevel::Warn, component, message);
}

pub fn error(component: &str, message: impl AsRef<str>) {
    emit(LogLevel::Error, component, message);
}

#[cfg(test)]
mod tests {
    use super::LogLevel;

    #[test]
    fn log_levels_order_from_most_verbose_to_least() {
        assert!(LogLevel::Debug < LogLevel::Info);
        assert!(LogLevel::Info < LogLevel::Warn);
        assert!(LogLevel::Warn < LogLevel::Error);
    }
}
