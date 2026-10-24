//! The colours of the interface, for a dark or a light terminal background.
//! The `theme` setting picks one once, at startup.

use std::sync::OnceLock;

use lf_mide_core::ThemeKind;
use ratatui::style::Color;

#[derive(Debug, PartialEq, Eq)]
pub struct Theme {
    /// Focused panel titles, the editor and selection highlights.
    pub accent: Color,
    /// Secondary text: hints and empty states.
    pub muted: Color,
    /// Background of the header and status bars.
    pub bar_bg: Color,
    /// Text drawn on top of the accent colour.
    pub on_accent: Color,
    pub ok: Color,
    pub warn: Color,
    pub error: Color,
    /// Informational highlights: running builds, hunk headers, renames.
    pub info: Color,
    /// Conflicts and other changed-state highlights.
    pub changed: Color,
}

pub const DARK: Theme = Theme {
    accent: Color::Cyan,
    muted: Color::DarkGray,
    bar_bg: Color::Rgb(40, 44, 52),
    on_accent: Color::Black,
    ok: Color::Green,
    warn: Color::Yellow,
    error: Color::Red,
    info: Color::Cyan,
    changed: Color::Magenta,
};

/// Darker shades, so they read on a white or cream background.
pub const LIGHT: Theme = Theme {
    accent: Color::Rgb(0, 95, 135),
    muted: Color::Rgb(100, 100, 110),
    bar_bg: Color::Rgb(222, 226, 234),
    on_accent: Color::White,
    ok: Color::Rgb(0, 120, 40),
    warn: Color::Rgb(160, 95, 0),
    error: Color::Rgb(180, 0, 0),
    info: Color::Rgb(0, 95, 135),
    changed: Color::Rgb(130, 0, 150),
};

static CURRENT: OnceLock<&'static Theme> = OnceLock::new();

pub fn of(kind: ThemeKind) -> &'static Theme {
    match kind {
        ThemeKind::Dark => &DARK,
        ThemeKind::Light => &LIGHT,
    }
}

/// Chooses the theme for this run. Only the first choice counts.
pub fn use_theme(kind: ThemeKind) {
    let _ = CURRENT.set(of(kind));
}

/// The theme in use; dark until one is chosen.
pub fn current() -> &'static Theme {
    CURRENT.get().copied().unwrap_or(&DARK)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_setting_names_its_own_palette() {
        assert_eq!(of(ThemeKind::Dark), &DARK);
        assert_eq!(of(ThemeKind::Light), &LIGHT);
        assert_ne!(DARK.accent, LIGHT.accent);
    }

    #[test]
    fn without_a_choice_the_dark_palette_is_used() {
        assert_eq!(current(), &DARK);
    }
}
