//! Which terminal LF-Mide is running in, and the splitting fallbacks.

use lf_mide_core::{Category, CheckResult, Status};

use super::{CheckFn, Ctx, ToolSpec, tool};

const ITERM_APP: &str = "/Applications/iTerm.app";

pub const CHECKS: &[CheckFn] = &[current_terminal, iterm2_installed, tmux];

/// Friendly name for a `$TERM_PROGRAM` value, if we know it.
fn terminal_name(term_program: &str) -> Option<&'static str> {
    match term_program {
        "iTerm.app" => Some("iTerm2"),
        "ghostty" => Some("Ghostty"),
        "WezTerm" => Some("WezTerm"),
        "Apple_Terminal" => Some("Terminal.app"),
        _ => None,
    }
}

fn current_terminal(ctx: &Ctx) -> Vec<CheckResult> {
    const NAME: &str = "Current terminal";
    const HINT: &str = "Set `terminal = \"iterm2\"` (or another) in ~/.config/lf-mide/config.toml";
    let result = match ctx.sys.env("TERM_PROGRAM") {
        None => {
            CheckResult::new(Category::Terminal, NAME, Status::Warn, "not detected").with_hint(HINT)
        }
        Some(program) => match terminal_name(&program) {
            Some(name) => {
                let in_tmux = ctx.sys.env("TMUX").is_some();
                let detail = if in_tmux {
                    format!("{name} (inside tmux)")
                } else {
                    name.to_string()
                };
                CheckResult::new(Category::Terminal, NAME, Status::Ok, detail)
            }
            None => CheckResult::new(
                Category::Terminal,
                NAME,
                Status::Warn,
                format!("unknown (TERM_PROGRAM={program})"),
            )
            .with_hint(HINT),
        },
    };
    vec![result]
}

fn iterm2_installed(ctx: &Ctx) -> Vec<CheckResult> {
    let result = if ctx.sys.is_dir(std::path::Path::new(ITERM_APP)) {
        CheckResult::new(Category::Terminal, "iTerm2", Status::Ok, "installed").with_path(ITERM_APP)
    } else {
        CheckResult::new(Category::Terminal, "iTerm2", Status::Warn, "not installed")
            .with_hint("brew install --cask iterm2")
    };
    vec![result]
}

fn tmux(ctx: &Ctx) -> Vec<CheckResult> {
    let spec = ToolSpec {
        category: Category::Terminal,
        name: "tmux",
        binary: "tmux",
        version_args: &["-V"],
        min: None,
        required: false,
        hint: "brew install tmux",
    };
    vec![tool(ctx, &spec, None)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeSystem;

    fn one(check: CheckFn, sys: &FakeSystem) -> CheckResult {
        let mut results = check(&Ctx { sys, project: None });
        assert_eq!(results.len(), 1);
        results.remove(0)
    }

    #[test]
    fn maps_known_term_programs() {
        assert_eq!(terminal_name("iTerm.app"), Some("iTerm2"));
        assert_eq!(terminal_name("ghostty"), Some("Ghostty"));
        assert_eq!(terminal_name("WezTerm"), Some("WezTerm"));
        assert_eq!(terminal_name("Apple_Terminal"), Some("Terminal.app"));
        assert_eq!(terminal_name("mystery"), None);
    }

    #[test]
    fn detects_iterm2() {
        let r = one(
            current_terminal,
            &FakeSystem::new().env("TERM_PROGRAM", "iTerm.app"),
        );

        assert_eq!((r.status, r.detail.as_str()), (Status::Ok, "iTerm2"));
    }

    #[test]
    fn notes_when_inside_tmux() {
        let sys = FakeSystem::new()
            .env("TERM_PROGRAM", "iTerm.app")
            .env("TMUX", "/tmp/tmux-501/default,1,0");

        assert_eq!(one(current_terminal, &sys).detail, "iTerm2 (inside tmux)");
    }

    #[test]
    fn unknown_and_undetected_terminals_warn() {
        let unknown = one(
            current_terminal,
            &FakeSystem::new().env("TERM_PROGRAM", "mystery"),
        );
        let none = one(current_terminal, &FakeSystem::new());

        assert_eq!(
            (unknown.status, unknown.detail.as_str()),
            (Status::Warn, "unknown (TERM_PROGRAM=mystery)")
        );
        assert_eq!(none.status, Status::Warn);
        assert!(none.hint.is_some());
    }

    #[test]
    fn iterm2_app_is_optional() {
        let installed = one(iterm2_installed, &FakeSystem::new().dir(ITERM_APP, &[]));
        let absent = one(iterm2_installed, &FakeSystem::new());

        assert_eq!(installed.status, Status::Ok);
        assert_eq!(absent.status, Status::Warn);
        assert_eq!(absent.hint.as_deref(), Some("brew install --cask iterm2"));
    }

    #[test]
    fn tmux_is_an_optional_fallback() {
        let r = one(tmux, &FakeSystem::new().env("PATH", "/opt/bin"));

        assert_eq!(r.status, Status::Warn);
        assert_eq!(r.hint.as_deref(), Some("brew install tmux"));
    }
}
