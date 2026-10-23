//! Where LF-Mide keeps its files.

use std::path::{Path, PathBuf};

use directories::{BaseDirs, ProjectDirs};
use lf_mide_core::config::PROJECT_CONFIG_FILE;

const APP_NAME: &str = "lf-mide";
const CONFIG_FILE: &str = "config.toml";

/// `$XDG_CONFIG_HOME/lf-mide/config.toml`, else `~/.config/lf-mide/config.toml`.
/// Terminal tools on macOS conventionally use `~/.config`, so we do too.
pub fn global_config() -> Option<PathBuf> {
    global_config_from(
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
        BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf()),
    )
}

fn global_config_from(xdg_config_home: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    let base = xdg_config_home
        .filter(|dir| dir.is_absolute())
        .or_else(|| home.map(|home| home.join(".config")))?;
    Some(base.join(APP_NAME).join(CONFIG_FILE))
}

pub fn project_config(root: &Path) -> PathBuf {
    root.join(PROJECT_CONFIG_FILE)
}

/// `~/Library/Caches/lf-mide` on macOS. Logs go here, never to the terminal.
pub fn log_dir() -> Option<PathBuf> {
    ProjectDirs::from("", "", APP_NAME).map(|dirs| dirs.cache_dir().to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_config_prefers_absolute_xdg_config_home() {
        let path = global_config_from(Some("/xdg".into()), Some("/home/u".into()));

        assert_eq!(path, Some(PathBuf::from("/xdg/lf-mide/config.toml")));
    }

    #[test]
    fn global_config_falls_back_to_home_dot_config() {
        let unset = global_config_from(None, Some("/home/u".into()));
        // The XDG spec says relative values must be ignored.
        let relative = global_config_from(Some("rel".into()), Some("/home/u".into()));

        let expected = Some(PathBuf::from("/home/u/.config/lf-mide/config.toml"));
        assert_eq!(unset, expected);
        assert_eq!(relative, expected);
    }

    #[test]
    fn global_config_is_none_without_any_base() {
        assert_eq!(global_config_from(None, None), None);
    }

    #[test]
    fn project_config_lives_in_project_root() {
        assert_eq!(
            project_config(Path::new("/p")),
            PathBuf::from("/p/.lfmide.toml")
        );
    }
}
