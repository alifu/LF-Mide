//! Working out which terminal LF-Mide is running in.

use lf_mide_core::TerminalKind;
use lf_mide_system::System;

/// The terminal LF-Mide is running in, from environment variables.
/// Inside tmux we always answer tmux: splitting its pane is what you want
/// whatever terminal hosts it.
pub fn detect(sys: &dyn System) -> Option<TerminalKind> {
    let var = |key: &str| sys.env(key).filter(|value| !value.is_empty());
    if var("TMUX").is_some() {
        return Some(TerminalKind::Tmux);
    }
    let by_program = match var("TERM_PROGRAM").as_deref() {
        Some("iTerm.app") => Some(TerminalKind::Iterm2),
        Some("ghostty") => Some(TerminalKind::Ghostty),
        Some("WezTerm") => Some(TerminalKind::Wezterm),
        Some("Apple_Terminal") => Some(TerminalKind::Terminal),
        _ => None,
    };
    by_program.or_else(|| {
        let is_kitty =
            var("KITTY_WINDOW_ID").is_some() || var("TERM").as_deref() == Some("xterm-kitty");
        is_kitty.then_some(TerminalKind::Kitty)
    })
}

/// `configured` unless it is `Auto`, which is [`detect`]ed.
pub fn resolve(configured: TerminalKind, sys: &dyn System) -> Option<TerminalKind> {
    match configured {
        TerminalKind::Auto => detect(sys),
        explicit => Some(explicit),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lf_mide_system::fake::FakeSystem;

    fn term(program: &str) -> FakeSystem {
        FakeSystem::new().env("TERM_PROGRAM", program)
    }

    #[test]
    fn recognises_terminals_by_term_program() {
        assert_eq!(detect(&term("iTerm.app")), Some(TerminalKind::Iterm2));
        assert_eq!(detect(&term("ghostty")), Some(TerminalKind::Ghostty));
        assert_eq!(detect(&term("WezTerm")), Some(TerminalKind::Wezterm));
        assert_eq!(
            detect(&term("Apple_Terminal")),
            Some(TerminalKind::Terminal)
        );
    }

    #[test]
    fn recognises_kitty_by_its_own_variables() {
        assert_eq!(
            detect(&FakeSystem::new().env("KITTY_WINDOW_ID", "3")),
            Some(TerminalKind::Kitty)
        );
        assert_eq!(
            detect(&FakeSystem::new().env("TERM", "xterm-kitty")),
            Some(TerminalKind::Kitty)
        );
    }

    #[test]
    fn tmux_wins_over_the_hosting_terminal() {
        let sys = term("iTerm.app").env("TMUX", "/tmp/tmux-501/default,1,0");

        assert_eq!(detect(&sys), Some(TerminalKind::Tmux));
    }

    #[test]
    fn unknown_or_missing_is_none() {
        assert_eq!(detect(&term("mystery")), None);
        assert_eq!(detect(&FakeSystem::new()), None);
        assert_eq!(
            detect(&FakeSystem::new().env("TMUX", "")),
            None,
            "empty means unset"
        );
    }

    #[test]
    fn resolve_detects_only_for_auto() {
        let sys = term("iTerm.app");

        assert_eq!(
            resolve(TerminalKind::Auto, &sys),
            Some(TerminalKind::Iterm2)
        );
        assert_eq!(
            resolve(TerminalKind::Ghostty, &sys),
            Some(TerminalKind::Ghostty)
        );
        assert_eq!(resolve(TerminalKind::Auto, &FakeSystem::new()), None);
    }
}
