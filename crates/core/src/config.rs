//! User configuration: a global file overridden by a per-project file.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

pub const DEFAULT_LEADER: &str = "C-Space";
pub const PROJECT_CONFIG_FILE: &str = ".lfmide.toml";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TerminalKind {
    Auto,
    Iterm2,
    Ghostty,
    Wezterm,
    Kitty,
    Terminal,
    Tmux,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Prefix key for IDE commands, in Vim notation (e.g. `C-Space`).
    pub leader: String,
    pub terminal: TerminalKind,
    pub show_blame: bool,
    pub show_perf: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            leader: DEFAULT_LEADER.to_string(),
            terminal: TerminalKind::Auto,
            show_blame: false,
            show_perf: true,
        }
    }
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid config in {path}: {message}")]
    Invalid { path: PathBuf, message: String },
}

/// Mirror of [`Config`] where every field is optional, so a file only needs
/// to set what it changes.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PartialConfig {
    leader: Option<String>,
    terminal: Option<TerminalKind>,
    show_blame: Option<bool>,
    show_perf: Option<bool>,
}

impl Config {
    /// Returns a new config with the fields set in `toml_text` applied on top.
    pub fn merged(&self, toml_text: &str, source: &Path) -> Result<Config, ConfigError> {
        let invalid = |message: String| ConfigError::Invalid {
            path: source.to_path_buf(),
            message,
        };
        let partial: PartialConfig =
            toml::from_str(toml_text).map_err(|err| invalid(err.to_string()))?;
        if partial
            .leader
            .as_deref()
            .is_some_and(|leader| leader.trim().is_empty())
        {
            return Err(invalid("leader must not be empty".to_string()));
        }

        Ok(Config {
            leader: partial.leader.unwrap_or_else(|| self.leader.clone()),
            terminal: partial.terminal.unwrap_or(self.terminal),
            show_blame: partial.show_blame.unwrap_or(self.show_blame),
            show_perf: partial.show_perf.unwrap_or(self.show_perf),
        })
    }

    /// Loads defaults, then the global file, then the project file. Missing
    /// files are skipped.
    pub fn load(global: Option<&Path>, project: Option<&Path>) -> Result<Config, ConfigError> {
        [global, project]
            .into_iter()
            .flatten()
            .try_fold(Config::default(), |config, path| {
                match read_optional(path)? {
                    Some(text) => config.merged(&text, path),
                    None => Ok(config),
                }
            })
    }
}

fn read_optional(path: &Path) -> Result<Option<String>, ConfigError> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(ConfigError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn merged_overrides_only_given_fields() {
        let base = Config::default();

        let merged = base
            .merged(
                "terminal = \"iterm2\"\nshow_blame = true\n",
                Path::new("x.toml"),
            )
            .unwrap();

        assert_eq!(merged.terminal, TerminalKind::Iterm2);
        assert!(merged.show_blame);
        assert_eq!(merged.leader, DEFAULT_LEADER);
        assert_eq!(base, Config::default(), "original must not change");
    }

    #[test]
    fn merged_rejects_unknown_keys() {
        let err = Config::default()
            .merged("colour = \"red\"\n", Path::new("x.toml"))
            .unwrap_err();

        assert!(matches!(err, ConfigError::Invalid { .. }));
    }

    #[test]
    fn merged_rejects_empty_leader() {
        let err = Config::default()
            .merged("leader = \"  \"\n", Path::new("x.toml"))
            .unwrap_err();

        assert!(err.to_string().contains("leader"));
    }

    #[test]
    fn load_applies_project_over_global_and_skips_missing() {
        let tmp = TempDir::new().unwrap();
        let global = tmp.path().join("global.toml");
        let project = tmp.path().join("project.toml");
        fs::write(&global, "terminal = \"ghostty\"\nshow_perf = false\n").unwrap();
        fs::write(&project, "terminal = \"tmux\"\n").unwrap();

        let config = Config::load(Some(&global), Some(&project)).unwrap();
        let only_defaults = Config::load(Some(&tmp.path().join("missing.toml")), None).unwrap();

        assert_eq!(config.terminal, TerminalKind::Tmux);
        assert!(!config.show_perf);
        assert_eq!(only_defaults, Config::default());
    }
}
