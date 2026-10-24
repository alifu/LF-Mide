//! Terminal keys in Neovim's key notation (`<C-a>`, `<CR>`, `<lt>`), for
//! `nvim_input`.
//!
//! Modifiers are always written in the same order (`C-`, `M-`, `S-`, `D-`)
//! so a key and the configured leader can be compared as text.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// `key` as Neovim input, or `None` for keys Neovim has no name for.
pub fn notation(key: KeyEvent) -> Option<String> {
    let mods = key.modifiers;
    let (name, mods) = match key.code {
        KeyCode::Char(ch) => char_name(ch, mods)?,
        KeyCode::Enter => ("CR".to_string(), mods),
        KeyCode::Esc => ("Esc".to_string(), mods),
        KeyCode::Backspace => ("BS".to_string(), mods),
        KeyCode::Tab => ("Tab".to_string(), mods),
        KeyCode::BackTab => ("Tab".to_string(), mods | KeyModifiers::SHIFT),
        KeyCode::Delete => ("Del".to_string(), mods),
        KeyCode::Insert => ("Insert".to_string(), mods),
        KeyCode::Home => ("Home".to_string(), mods),
        KeyCode::End => ("End".to_string(), mods),
        KeyCode::PageUp => ("PageUp".to_string(), mods),
        KeyCode::PageDown => ("PageDown".to_string(), mods),
        KeyCode::Up => ("Up".to_string(), mods),
        KeyCode::Down => ("Down".to_string(), mods),
        KeyCode::Left => ("Left".to_string(), mods),
        KeyCode::Right => ("Right".to_string(), mods),
        KeyCode::F(n) => (format!("F{n}"), mods),
        _ => return None,
    };
    Some(wrap(&name, mods))
}

/// The leader as written in the config (`C-Space`, `C-\`, `F12`, `,`) in
/// the form [`notation`] produces, or `None` if it is not a key.
pub fn leader(text: &str) -> Option<String> {
    let text = text.trim();
    let inner = text
        .strip_prefix('<')
        .and_then(|rest| rest.strip_suffix('>'))
        .unwrap_or(text);
    if inner.chars().count() == 1 {
        let ch = inner.chars().next()?;
        return (!ch.is_control()).then(|| plain_char(ch));
    }
    let mut mods = KeyModifiers::NONE;
    let mut rest = inner;
    while let Some((prefix, tail)) = split_modifier(rest) {
        mods |= prefix;
        rest = tail;
    }
    let name = canonical_name(rest)?;
    Some(wrap(&name, mods))
}

fn split_modifier(text: &str) -> Option<(KeyModifiers, &str)> {
    let (head, tail) = text.split_at_checked(2)?;
    if tail.is_empty() {
        return None;
    }
    let modifier = match head.to_ascii_uppercase().as_str() {
        "C-" => KeyModifiers::CONTROL,
        "M-" | "A-" => KeyModifiers::ALT,
        "S-" => KeyModifiers::SHIFT,
        "D-" => KeyModifiers::SUPER,
        _ => return None,
    };
    Some((modifier, tail))
}

/// A key name in the spelling [`notation`] uses.
fn canonical_name(name: &str) -> Option<String> {
    const NAMES: [&str; 14] = [
        "Space", "CR", "Esc", "BS", "Tab", "Del", "Insert", "Home", "End", "PageUp", "PageDown",
        "Up", "Down", "Left",
    ];
    if name.chars().count() == 1 {
        let ch = name.chars().next()?;
        return (!ch.is_control()).then(|| char_text(ch));
    }
    if let Some(known) = NAMES
        .iter()
        .chain(["Right", "lt", "Bslash", "Bar"].iter())
        .find(|known| known.eq_ignore_ascii_case(name))
    {
        return Some(match *known {
            "Bslash" => "\\".to_string(),
            "Bar" => "|".to_string(),
            other => other.to_string(),
        });
    }
    if name.eq_ignore_ascii_case("Enter") || name.eq_ignore_ascii_case("Return") {
        return Some("CR".to_string());
    }
    let number = name.strip_prefix(['F', 'f'])?.parse::<u8>().ok()?;
    (1..=24).contains(&number).then(|| format!("F{number}"))
}

/// The name and the modifiers still to write for a character key.
fn char_name(ch: char, mods: KeyModifiers) -> Option<(String, KeyModifiers)> {
    if mods.contains(KeyModifiers::CONTROL) {
        // Terminals send Ctrl-\ ] ^ _ as bytes 0x1C-0x1F, which crossterm
        // reports as Ctrl-4 to Ctrl-7.
        let ch = match ch {
            '4' => '\\',
            '5' => ']',
            '6' => '^',
            '7' => '_',
            other => other,
        };
        // A shifted letter is already upper case; Neovim reads Ctrl-letter
        // without regard to case.
        let ch = ch.to_ascii_lowercase();
        return Some((char_text(ch), mods - KeyModifiers::SHIFT));
    }
    if ch.is_control() {
        return None;
    }
    // Shift is already part of the character itself ('A', '?').
    Some((char_text(ch), mods - KeyModifiers::SHIFT))
}

fn char_text(ch: char) -> String {
    match ch {
        ' ' => "Space".to_string(),
        '<' => "lt".to_string(),
        other => other.to_string(),
    }
}

/// A lone character as plain input: `<` must still be escaped.
fn plain_char(ch: char) -> String {
    match ch {
        '<' => "<lt>".to_string(),
        other => other.to_string(),
    }
}

fn wrap(name: &str, mods: KeyModifiers) -> String {
    let prefix: String = [
        (KeyModifiers::CONTROL, "C-"),
        (KeyModifiers::ALT, "M-"),
        (KeyModifiers::SHIFT, "S-"),
        (KeyModifiers::SUPER, "D-"),
    ]
    .iter()
    .filter(|(flag, _)| mods.contains(*flag))
    .map(|(_, text)| *text)
    .collect();
    let simple = prefix.is_empty() && name.chars().count() == 1;
    match (simple, name) {
        (true, _) => name.to_string(),
        (false, "Space") if prefix.is_empty() => " ".to_string(),
        _ => format!("<{prefix}{name}>"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, mods: KeyModifiers) -> Option<String> {
        notation(KeyEvent::new(code, mods))
    }

    const NONE: KeyModifiers = KeyModifiers::NONE;
    const CTRL: KeyModifiers = KeyModifiers::CONTROL;
    const ALT: KeyModifiers = KeyModifiers::ALT;
    const SHIFT: KeyModifiers = KeyModifiers::SHIFT;

    #[test]
    fn plain_characters_are_sent_as_they_are() {
        assert_eq!(key(KeyCode::Char('a'), NONE).as_deref(), Some("a"));
        assert_eq!(key(KeyCode::Char('A'), SHIFT).as_deref(), Some("A"));
        assert_eq!(key(KeyCode::Char('?'), SHIFT).as_deref(), Some("?"));
        assert_eq!(key(KeyCode::Char('é'), NONE).as_deref(), Some("é"));
        assert_eq!(key(KeyCode::Char(' '), NONE).as_deref(), Some(" "));
        assert_eq!(key(KeyCode::Char('\\'), NONE).as_deref(), Some("\\"));
    }

    #[test]
    fn a_less_than_sign_is_escaped() {
        assert_eq!(key(KeyCode::Char('<'), NONE).as_deref(), Some("<lt>"));
        assert_eq!(key(KeyCode::Char('<'), ALT).as_deref(), Some("<M-lt>"));
    }

    #[test]
    fn modifiers_are_written_in_a_fixed_order() {
        assert_eq!(key(KeyCode::Char('w'), CTRL).as_deref(), Some("<C-w>"));
        assert_eq!(
            key(KeyCode::Char('W'), CTRL | SHIFT).as_deref(),
            Some("<C-w>")
        );
        assert_eq!(key(KeyCode::Char('x'), ALT).as_deref(), Some("<M-x>"));
        assert_eq!(
            key(KeyCode::Char('x'), ALT | CTRL).as_deref(),
            Some("<C-M-x>")
        );
        assert_eq!(key(KeyCode::Char(' '), CTRL).as_deref(), Some("<C-Space>"));
        assert_eq!(
            key(KeyCode::Left, CTRL | SHIFT).as_deref(),
            Some("<C-S-Left>")
        );
    }

    #[test]
    fn control_punctuation_from_its_terminal_bytes() {
        assert_eq!(key(KeyCode::Char('4'), CTRL).as_deref(), Some("<C-\\>"));
        assert_eq!(key(KeyCode::Char('5'), CTRL).as_deref(), Some("<C-]>"));
        assert_eq!(key(KeyCode::Char('6'), CTRL).as_deref(), Some("<C-^>"));
        assert_eq!(key(KeyCode::Char('7'), CTRL).as_deref(), Some("<C-_>"));
    }

    #[test]
    fn special_keys_have_their_names() {
        let cases = [
            (KeyCode::Enter, "<CR>"),
            (KeyCode::Esc, "<Esc>"),
            (KeyCode::Backspace, "<BS>"),
            (KeyCode::Tab, "<Tab>"),
            (KeyCode::BackTab, "<S-Tab>"),
            (KeyCode::Delete, "<Del>"),
            (KeyCode::Home, "<Home>"),
            (KeyCode::PageDown, "<PageDown>"),
            (KeyCode::Up, "<Up>"),
            (KeyCode::F(5), "<F5>"),
        ];
        for (code, expected) in cases {
            assert_eq!(key(code, NONE).as_deref(), Some(expected), "{code:?}");
        }
        assert_eq!(key(KeyCode::BackTab, SHIFT).as_deref(), Some("<S-Tab>"));
    }

    #[test]
    fn keys_without_a_neovim_name_are_dropped() {
        assert_eq!(key(KeyCode::CapsLock, NONE), None);
        assert_eq!(key(KeyCode::Char('\u{1b}'), NONE), None);
    }

    #[test]
    fn the_leader_matches_the_key_that_types_it() {
        let cases = [
            ("C-Space", KeyCode::Char(' '), CTRL),
            ("<C-Space>", KeyCode::Char(' '), CTRL),
            ("c-space", KeyCode::Char(' '), CTRL),
            ("C-\\", KeyCode::Char('4'), CTRL),
            ("C-Bslash", KeyCode::Char('4'), CTRL),
            ("C-a", KeyCode::Char('a'), CTRL),
            ("M-C-a", KeyCode::Char('a'), CTRL | ALT),
            ("F12", KeyCode::F(12), NONE),
            (",", KeyCode::Char(','), NONE),
            ("<", KeyCode::Char('<'), NONE),
        ];
        for (text, code, mods) in cases {
            assert_eq!(leader(text), key(code, mods), "{text}");
        }
    }

    #[test]
    fn a_leader_that_is_not_a_key_is_refused() {
        for text in ["", "C-", "Ctrl+Space", "F99", "X-a", "C-\u{7}"] {
            assert_eq!(leader(text), None, "{text:?}");
        }
    }
}
