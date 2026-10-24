//! User configuration: a global file overridden by a per-project file.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

pub const DEFAULT_LEADER: &str = "C-Space";
pub const PROJECT_CONFIG_FILE: &str = ".lfmide.toml";

/// Where Neovim runs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EditorMode {
    /// Inside LF-Mide's editor panel.
    #[default]
    Embedded,
    /// In a split pane of the terminal, next to LF-Mide.
    Split,
}

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

impl TerminalKind {
    /// Every choice, in the order the picker lists them.
    pub const ALL: [TerminalKind; 7] = [
        TerminalKind::Auto,
        TerminalKind::Iterm2,
        TerminalKind::Ghostty,
        TerminalKind::Wezterm,
        TerminalKind::Kitty,
        TerminalKind::Terminal,
        TerminalKind::Tmux,
    ];

    /// Name for people.
    pub fn label(self) -> &'static str {
        match self {
            TerminalKind::Auto => "Auto-detect",
            TerminalKind::Iterm2 => "iTerm2",
            TerminalKind::Ghostty => "Ghostty",
            TerminalKind::Wezterm => "WezTerm",
            TerminalKind::Kitty => "kitty",
            TerminalKind::Terminal => "Terminal.app",
            TerminalKind::Tmux => "tmux",
        }
    }

    /// What to expect when LF-Mide opens Neovim next to itself in this
    /// terminal, if it is not a plain split.
    pub fn launch_note(self) -> Option<&'static str> {
        match self {
            TerminalKind::Ghostty => Some("you split manually"),
            TerminalKind::Terminal => Some("opens a new window"),
            _ => None,
        }
    }

    /// The value written in the config file (`terminal = "iterm2"`).
    pub fn config_name(self) -> &'static str {
        match self {
            TerminalKind::Auto => "auto",
            TerminalKind::Iterm2 => "iterm2",
            TerminalKind::Ghostty => "ghostty",
            TerminalKind::Wezterm => "wezterm",
            TerminalKind::Kitty => "kitty",
            TerminalKind::Terminal => "terminal",
            TerminalKind::Tmux => "tmux",
        }
    }
}

/// `text` (a config file) with its `terminal` setting set to `kind`: an
/// existing `terminal = ...` line is replaced in place, otherwise one is
/// appended. Everything else, comments included, is kept as is.
pub fn set_terminal_in_config(text: &str, kind: TerminalKind) -> String {
    let setting = format!("terminal = \"{}\"", kind.config_name());
    let is_terminal_line = |line: &str| {
        line.trim_start()
            .strip_prefix("terminal")
            .is_some_and(|rest| rest.trim_start().starts_with('='))
    };

    if text.lines().any(is_terminal_line) {
        let replaced: Vec<&str> = text
            .lines()
            .map(|line| {
                if is_terminal_line(line) {
                    setting.as_str()
                } else {
                    line
                }
            })
            .collect();
        return format!("{}\n", replaced.join("\n"));
    }
    let mut out = text.to_string();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&setting);
    out.push('\n');
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Prefix key for IDE commands, in Vim notation (e.g. `C-Space`).
    pub leader: String,
    pub editor: EditorMode,
    /// Set up language servers (completion) in the embedded Neovim.
    pub lsp: bool,
    pub terminal: TerminalKind,
    pub show_blame: bool,
    pub show_perf: bool,
    /// Seconds between performance samples.
    pub perf_interval_secs: u64,
    /// Swap use (MiB) at which the indicator turns yellow / red.
    pub swap_warn_mb: u64,
    pub swap_critical_mb: u64,
}

/// Allowed range for `perf_interval_secs`.
pub const PERF_INTERVAL_RANGE: std::ops::RangeInclusive<u64> = 1..=60;

impl Default for Config {
    fn default() -> Self {
        Self {
            leader: DEFAULT_LEADER.to_string(),
            editor: EditorMode::Embedded,
            lsp: true,
            terminal: TerminalKind::Auto,
            show_blame: false,
            show_perf: true,
            perf_interval_secs: 2,
            swap_warn_mb: 2048,
            swap_critical_mb: 4096,
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
    editor: Option<EditorMode>,
    lsp: Option<bool>,
    terminal: Option<TerminalKind>,
    show_blame: Option<bool>,
    show_perf: Option<bool>,
    perf_interval_secs: Option<u64>,
    swap_warn_mb: Option<u64>,
    swap_critical_mb: Option<u64>,
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

        let merged = Config {
            leader: partial.leader.unwrap_or_else(|| self.leader.clone()),
            editor: partial.editor.unwrap_or(self.editor),
            lsp: partial.lsp.unwrap_or(self.lsp),
            terminal: partial.terminal.unwrap_or(self.terminal),
            show_blame: partial.show_blame.unwrap_or(self.show_blame),
            show_perf: partial.show_perf.unwrap_or(self.show_perf),
            perf_interval_secs: partial
                .perf_interval_secs
                .unwrap_or(self.perf_interval_secs),
            swap_warn_mb: partial.swap_warn_mb.unwrap_or(self.swap_warn_mb),
            swap_critical_mb: partial.swap_critical_mb.unwrap_or(self.swap_critical_mb),
        };
        if !PERF_INTERVAL_RANGE.contains(&merged.perf_interval_secs) {
            return Err(invalid(format!(
                "perf_interval_secs must be between {} and {}",
                PERF_INTERVAL_RANGE.start(),
                PERF_INTERVAL_RANGE.end()
            )));
        }
        if merged.swap_warn_mb > merged.swap_critical_mb {
            return Err(invalid(
                "swap_warn_mb must not be larger than swap_critical_mb".to_string(),
            ));
        }
        Ok(merged)
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
    fn the_editor_is_embedded_with_language_servers_unless_set_otherwise() {
        let source = Path::new("/c.toml");

        let split = Config::default()
            .merged("editor = \"split\"\nlsp = false\n", source)
            .unwrap();

        assert_eq!(Config::default().editor, EditorMode::Embedded);
        assert!(Config::default().lsp);
        assert_eq!((split.editor, split.lsp), (EditorMode::Split, false));
        assert!(
            Config::default()
                .merged("editor = \"vim\"", source)
                .is_err()
        );
    }

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

    #[test]
    fn config_names_round_trip_through_the_parser() {
        for kind in TerminalKind::ALL {
            let toml = format!("terminal = \"{}\"\n", kind.config_name());

            let parsed = Config::default()
                .merged(&toml, Path::new("x.toml"))
                .unwrap();

            assert_eq!(parsed.terminal, kind, "{kind:?}");
        }
    }

    #[test]
    fn launch_notes_flag_terminals_that_do_not_split() {
        assert_eq!(
            TerminalKind::Ghostty.launch_note(),
            Some("you split manually")
        );
        assert_eq!(
            TerminalKind::Terminal.launch_note(),
            Some("opens a new window")
        );
        for kind in [
            TerminalKind::Iterm2,
            TerminalKind::Tmux,
            TerminalKind::Wezterm,
            TerminalKind::Kitty,
            TerminalKind::Auto,
        ] {
            assert_eq!(kind.launch_note(), None, "{kind:?}");
        }
    }

    #[test]
    fn labels_are_for_people() {
        assert_eq!(TerminalKind::Iterm2.label(), "iTerm2");
        assert_eq!(TerminalKind::Terminal.label(), "Terminal.app");
        assert_eq!(TerminalKind::Auto.label(), "Auto-detect");
        assert_eq!(TerminalKind::ALL.len(), 7);
    }

    #[test]
    fn perf_settings_can_be_changed_and_are_checked() {
        let base = Config::default();

        let ok = base
            .merged(
                "perf_interval_secs = 5\nswap_warn_mb = 1000\nswap_critical_mb = 3000\n",
                Path::new("x.toml"),
            )
            .unwrap();
        let slow = base.merged("perf_interval_secs = 0\n", Path::new("x.toml"));
        let silly = base.merged("perf_interval_secs = 100000\n", Path::new("x.toml"));
        let backwards = base.merged("swap_warn_mb = 9000\n", Path::new("x.toml"));

        assert_eq!(
            (ok.perf_interval_secs, ok.swap_warn_mb, ok.swap_critical_mb),
            (5, 1000, 3000)
        );
        for bad in [slow, silly] {
            assert!(bad.unwrap_err().to_string().contains("perf_interval_secs"));
        }
        assert!(backwards.unwrap_err().to_string().contains("swap_warn_mb"));
        assert_eq!(base.perf_interval_secs, 2);
    }

    #[test]
    fn set_terminal_replaces_the_existing_line_in_place() {
        let text = "# my settings\nleader = \"C-a\"\nterminal = \"ghostty\"\nshow_perf = false\n";

        let updated = set_terminal_in_config(text, TerminalKind::Iterm2);

        assert_eq!(
            updated,
            "# my settings\nleader = \"C-a\"\nterminal = \"iterm2\"\nshow_perf = false\n"
        );
    }

    #[test]
    fn set_terminal_appends_when_missing_and_handles_empty_files() {
        assert_eq!(
            set_terminal_in_config("", TerminalKind::Tmux),
            "terminal = \"tmux\"\n"
        );
        assert_eq!(
            set_terminal_in_config("show_blame = true", TerminalKind::Tmux),
            "show_blame = true\nterminal = \"tmux\"\n"
        );
        assert_eq!(
            set_terminal_in_config("show_blame = true\n", TerminalKind::Kitty),
            "show_blame = true\nterminal = \"kitty\"\n"
        );
    }

    #[test]
    fn set_terminal_ignores_comments_and_similar_keys() {
        let text = "# terminal = \"kitty\"\nterminal_font = \"x\"\n";

        let updated = set_terminal_in_config(text, TerminalKind::Wezterm);

        assert!(updated.starts_with("# terminal = \"kitty\"\nterminal_font = \"x\"\n"));
        assert!(updated.ends_with("terminal = \"wezterm\"\n"));
    }

    #[test]
    fn set_terminal_accepts_tight_spacing_and_the_result_parses() {
        let updated = set_terminal_in_config("terminal=\"auto\"\n", TerminalKind::Terminal);

        assert_eq!(updated, "terminal = \"terminal\"\n");
        let parsed = Config::default()
            .merged(&updated, Path::new("x.toml"))
            .unwrap();
        assert_eq!(parsed.terminal, TerminalKind::Terminal);
    }
}
