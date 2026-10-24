//! macOS settings that decide whether LF-Mide's keys reach it: the system's
//! Ctrl-Space shortcut, the terminal's Cmd-Shift-O mapping, and permission
//! to control the terminal. None of these change anything.

use std::path::PathBuf;

use lf_mide_core::config::DEFAULT_LEADER;
use lf_mide_core::{Category, CheckResult, Status};

use super::{CheckFn, Ctx};

pub const CHECKS: &[CheckFn] = &[input_source_shortcut, iterm_find_file_key, automation];

const DEFAULTS: &str = "defaults";
const HOTKEYS_DOMAIN: &str = "com.apple.symbolichotkeys";
const HOTKEYS_KEY: &str = "AppleSymbolicHotKeys";
/// "Select the previous input source": Ctrl-Space.
const PREVIOUS_INPUT_SOURCE: &str = "60";
/// Parameters of a standard shortcut for Ctrl-Space: space (32), key code
/// 49, Control (262144).
const CTRL_SPACE: [&str; 3] = ["32", "49", "262144"];
const ITERM_DOMAIN: &str = "com.googlecode.iterm2";
/// What iTerm2 must send for Command-Shift-O (Super+Shift+o).
const FIND_FILE_SEQUENCE: &str = "111;10u";

/// The leader or editor mode written in the user's config file, if any.
fn config_value(ctx: &Ctx, key: &str) -> Option<String> {
    let dir = match ctx.sys.env("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        Some(xdg) => PathBuf::from(xdg),
        None => ctx.sys.home()?.join(".config"),
    };
    let text = ctx.sys.read_to_string(&dir.join("lf-mide/config.toml"))?;
    text.lines().find_map(|line| {
        let (name, value) = line.split_once('=')?;
        (name.trim() == key).then(|| value.trim().trim_matches('"').to_string())
    })
}

fn leader(ctx: &Ctx) -> String {
    config_value(ctx, "leader").unwrap_or_else(|| DEFAULT_LEADER.to_string())
}

/// Whether the "previous input source" shortcut is on, from the output of
/// `defaults read com.apple.symbolichotkeys AppleSymbolicHotKeys`.
fn input_source_shortcut_is_on(defaults_output: &str) -> bool {
    let mut lines = defaults_output.lines().map(str::trim);
    let header = format!("{PREVIOUS_INPUT_SOURCE} =");
    let Some(_) = lines.find(|line| line.starts_with(&header)) else {
        return false;
    };
    let mut enabled = false;
    let mut numbers = Vec::new();
    let mut in_parameters = false;
    for line in lines {
        if line == "};" && !in_parameters && (enabled || !numbers.is_empty()) {
            break;
        }
        if line.starts_with("enabled") {
            enabled = line.contains('1');
        } else if line.starts_with("parameters") {
            in_parameters = true;
        } else if in_parameters && line.starts_with(')') {
            in_parameters = false;
        } else if in_parameters {
            numbers.push(line.trim_end_matches(',').to_string());
        }
        // The next shortcut starts: this one is done.
        if line.ends_with("= {") && !line.starts_with("value") {
            break;
        }
    }
    enabled && numbers == CTRL_SPACE
}

fn input_source_shortcut(ctx: &Ctx) -> Vec<CheckResult> {
    const NAME: &str = "Ctrl-Space shortcut";
    if !is_ctrl_space(&leader(ctx)) {
        return Vec::new();
    }
    let Ok(output) = ctx
        .sys
        .run(DEFAULTS, &["read", HOTKEYS_DOMAIN, HOTKEYS_KEY])
    else {
        return Vec::new();
    };
    if !output.success {
        return Vec::new();
    }
    let result = if input_source_shortcut_is_on(&output.stdout) {
        CheckResult::new(
            Category::Terminal,
            NAME,
            Status::Warn,
            "macOS keeps Ctrl-Space (select previous input source), so the leader never arrives",
        )
        .with_hint(
            "Set `leader = \"F12\"` in ~/.config/lf-mide/config.toml, or turn the shortcut off in \
             System Settings → Keyboard → Keyboard Shortcuts → Input Sources",
        )
    } else {
        CheckResult::new(Category::Terminal, NAME, Status::Ok, "free for the leader")
    };
    vec![result]
}

fn is_ctrl_space(leader: &str) -> bool {
    let spelled = leader.trim().trim_matches(['<', '>']).to_ascii_lowercase();
    matches!(spelled.as_str(), "c-space" | "ctrl-space")
}

fn iterm_find_file_key(ctx: &Ctx) -> Vec<CheckResult> {
    const NAME: &str = "iTerm2 Cmd-Shift-O";
    if ctx.sys.env("TERM_PROGRAM").as_deref() != Some("iTerm.app") {
        return Vec::new();
    }
    let Ok(output) = ctx.sys.run(DEFAULTS, &["read", ITERM_DOMAIN]) else {
        return Vec::new();
    };
    if !output.success {
        return Vec::new();
    }
    let result = if output.stdout.contains(FIND_FILE_SEQUENCE) {
        CheckResult::new(
            Category::Terminal,
            NAME,
            Status::Ok,
            "sends the key code LF-Mide's finder listens for",
        )
    } else {
        CheckResult::new(
            Category::Terminal,
            NAME,
            Status::Warn,
            "no key mapping sends Cmd-Shift-O (iTerm2 may read its settings from a custom folder)",
        )
        .with_hint(
            "Settings → Profiles → Keys → Key Mappings → +, shortcut Cmd-Shift-O, action Send \
             Escape Sequence, value `[111;10u`; or use the leader then `f`",
        )
    };
    vec![result]
}

/// Only for `editor = "split"`, the one mode that asks the terminal to open
/// a pane. The first call can make macOS ask for permission.
fn automation(ctx: &Ctx) -> Vec<CheckResult> {
    const NAME: &str = "Automation permission";
    if config_value(ctx, "editor").as_deref() != Some("split") {
        return Vec::new();
    }
    let app = match ctx.sys.env("TERM_PROGRAM").as_deref() {
        Some("iTerm.app") => "iTerm",
        Some("Apple_Terminal") => "Terminal",
        _ => return Vec::new(),
    };
    let script = format!("tell application \"{app}\" to get name");
    let Ok(output) = ctx.sys.run("osascript", &["-e", &script]) else {
        return Vec::new();
    };
    let denied = output.stderr.contains("-1743") || output.stderr.contains("Not authorized");
    let result = if output.success {
        CheckResult::new(
            Category::Terminal,
            NAME,
            Status::Ok,
            format!("allowed to control {app}"),
        )
    } else if denied {
        CheckResult::new(
            Category::Terminal,
            NAME,
            Status::Warn,
            format!("not allowed to control {app}, so the Neovim split cannot open"),
        )
        .with_hint(
            "System Settings → Privacy & Security → Automation: allow your terminal to control it",
        )
    } else {
        return Vec::new();
    };
    vec![result]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeSystem;

    const HOTKEYS: &str = "defaults read com.apple.symbolichotkeys AppleSymbolicHotKeys";

    fn one(check: CheckFn, sys: &FakeSystem) -> Option<CheckResult> {
        let mut results = check(&Ctx { sys, project: None });
        assert!(results.len() <= 1);
        results.pop()
    }

    fn hotkeys(enabled: u8, modifiers: u32) -> String {
        format!(
            "{{\n    59 =     {{\n        enabled = 1;\n        value =         {{\n            parameters =             (\n                65535,\n                96,\n                8650752\n            );\n            type = standard;\n        }};\n    }};\n    60 =     {{\n        enabled = {enabled};\n        value =         {{\n            parameters =             (\n                32,\n                49,\n                {modifiers}\n            );\n            type = standard;\n        }};\n    }};\n    61 =     {{\n        enabled = 1;\n        value =         {{\n            parameters =             (\n                32,\n                49,\n                786432\n            );\n            type = standard;\n        }};\n    }};\n}}"
        )
    }

    fn home_with(config: &str) -> FakeSystem {
        FakeSystem::new()
            .home("/h")
            .file("/h/.config/lf-mide/config.toml", config)
    }

    #[test]
    fn an_enabled_ctrl_space_shortcut_warns_when_the_leader_is_ctrl_space() {
        let sys = FakeSystem::new().ok(HOTKEYS, &hotkeys(1, 262144));

        let result = one(input_source_shortcut, &sys).unwrap();

        assert_eq!(result.status, Status::Warn);
        assert!(result.hint.unwrap().contains("F12"));
    }

    #[test]
    fn a_disabled_shortcut_or_another_key_combination_is_fine() {
        let off = FakeSystem::new().ok(HOTKEYS, &hotkeys(0, 262144));
        let other = FakeSystem::new().ok(HOTKEYS, &hotkeys(1, 786432));

        assert_eq!(one(input_source_shortcut, &off).unwrap().status, Status::Ok);
        assert_eq!(
            one(input_source_shortcut, &other).unwrap().status,
            Status::Ok
        );
    }

    #[test]
    fn another_leader_or_no_defaults_command_means_no_result() {
        let f12 = home_with("leader = \"F12\"\n").ok(HOTKEYS, &hotkeys(1, 262144));

        assert!(one(input_source_shortcut, &f12).is_none());
        assert!(one(input_source_shortcut, &FakeSystem::new()).is_none());
    }

    #[test]
    fn iterm2_needs_a_mapping_that_sends_the_finder_sequence() {
        let cmd = "defaults read com.googlecode.iterm2";
        let iterm = |output: &str| {
            FakeSystem::new()
                .env("TERM_PROGRAM", "iTerm.app")
                .ok(cmd, output)
        };

        let mapped = one(iterm_find_file_key, &iterm("Text = \"\\U001b[111;10u\";")).unwrap();
        let missing = one(iterm_find_file_key, &iterm("Keyboard Map = {};")).unwrap();
        let elsewhere = one(
            iterm_find_file_key,
            &FakeSystem::new()
                .env("TERM_PROGRAM", "ghostty")
                .ok(cmd, "111;10u"),
        );

        assert_eq!(mapped.status, Status::Ok);
        assert_eq!(missing.status, Status::Warn);
        assert!(missing.hint.unwrap().contains("[111;10u"));
        assert!(elsewhere.is_none());
    }

    #[test]
    fn automation_is_probed_only_for_the_split_mode() {
        let script = "osascript -e tell application \"iTerm\" to get name";
        let base = |config: &str| home_with(config).env("TERM_PROGRAM", "iTerm.app");

        let embedded = base("").ok(script, "iTerm2");
        let allowed = base("editor = \"split\"\n").ok(script, "iTerm2");
        let denied = base("editor = \"split\"\n").fails(
            script,
            "execution error: Not authorized to send Apple events (-1743)",
        );

        assert!(one(automation, &embedded).is_none());
        assert_eq!(one(automation, &allowed).unwrap().status, Status::Ok);
        let denied = one(automation, &denied).unwrap();
        assert_eq!(denied.status, Status::Warn);
        assert!(denied.hint.unwrap().contains("Automation"));
    }
}
