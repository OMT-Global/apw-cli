use crate::error::{APWError, Result};
use crate::types::Status;
use std::env;
use std::ffi::OsString;
use std::path::PathBuf;

fn resolve_home(home: Option<OsString>, user_profile: Option<OsString>) -> Result<PathBuf> {
    let (variable, value) = match home {
        Some(value) => ("HOME", value),
        None => match user_profile {
            Some(value) => ("USERPROFILE", value),
            None => {
                return Err(APWError::new(
                    Status::InvalidConfig,
                    "HOME and USERPROFILE are not set; APW cannot resolve its state directory.",
                ));
            }
        },
    };

    if value.is_empty() {
        return Err(APWError::new(
            Status::InvalidConfig,
            format!("{variable} is empty; APW cannot resolve its state directory."),
        ));
    }

    let home = PathBuf::from(value);
    if !home.is_absolute() {
        return Err(APWError::new(
            Status::InvalidConfig,
            format!("{variable} must be an absolute path for APW state."),
        ));
    }

    Ok(home)
}

pub fn home_dir() -> Result<PathBuf> {
    resolve_home(env::var_os("HOME"), env::var_os("USERPROFILE"))
}

pub fn apw_state_root() -> Result<PathBuf> {
    Ok(home_dir()?.join(".apw"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_home_over_userprofile() {
        let resolved = resolve_home(
            Some(OsString::from("/home/primary")),
            Some(OsString::from("/home/fallback")),
        )
        .unwrap();

        assert_eq!(resolved, PathBuf::from("/home/primary"));
    }

    #[test]
    fn uses_userprofile_when_home_is_absent() {
        let resolved = resolve_home(None, Some(OsString::from("/home/fallback"))).unwrap();

        assert_eq!(resolved, PathBuf::from("/home/fallback"));
    }

    #[test]
    fn fails_when_home_variables_are_absent() {
        let error = resolve_home(None, None).unwrap_err();

        assert_eq!(error.code, Status::InvalidConfig);
        assert!(error.message.contains("HOME and USERPROFILE are not set"));
    }

    #[test]
    fn rejects_relative_home_paths() {
        let error = resolve_home(Some(OsString::from("relative/home")), None).unwrap_err();

        assert_eq!(error.code, Status::InvalidConfig);
        assert!(error.message.contains("must be an absolute path"));
    }
}
