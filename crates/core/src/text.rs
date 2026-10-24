//! Text from outside (file names, commit messages, build output, tool
//! output) made safe to put on the screen.
//!
//! The terminal library already drops control and zero-width characters, but
//! silently: a file name with a right-to-left override in it would look
//! perfectly ordinary. Here such characters become a visible `�`, one for
//! one, so the length (and anything counted in characters, like the
//! highlighted letters of a fuzzy match) stays the same and the oddity can
//! be seen.

use std::borrow::Cow;

/// What stands in for a character that must not be shown as it is.
pub const REPLACEMENT: char = '\u{FFFD}';

/// Characters that change how surrounding text reads, or show nothing:
/// zero-width and joiner marks, bidirectional overrides, embeddings and
/// isolates, line and paragraph separators, invisible operators, the byte
/// order mark.
pub fn is_hidden(ch: char) -> bool {
    matches!(
        ch,
        // Soft hyphen, combining grapheme joiner, Arabic letter mark.
        '\u{00AD}' | '\u{034F}' | '\u{061C}'
            // Fillers that draw nothing.
            | '\u{115F}' | '\u{1160}' | '\u{17B4}' | '\u{17B5}' | '\u{3164}' | '\u{FFA0}'
            // Mongolian free variation selectors and vowel separator.
            | '\u{180B}'..='\u{180F}'
            // Zero-width marks and direction marks, separators, embeddings.
            | '\u{200B}'..='\u{200F}'
            | '\u{2028}'..='\u{202E}'
            // Invisible operators, isolates, and the deprecated format controls.
            | '\u{2060}'..='\u{206F}'
            // A blank braille cell.
            | '\u{2800}'
            | '\u{FEFF}'
            // Interlinear annotation marks.
            | '\u{FFF9}'..='\u{FFFB}'
            // Tag characters and supplementary variation selectors.
            | '\u{E0000}'..='\u{E007F}'
            | '\u{E0100}'..='\u{E01EF}'
    )
}

fn must_change(ch: char) -> bool {
    ch == '\t' || ch.is_control() || is_hidden(ch)
}

/// `text` with every control or hidden character replaced by [`REPLACEMENT`]
/// (a tab by a space). Borrowed when there is nothing to change.
pub fn visible(text: &str) -> Cow<'_, str> {
    if !text.chars().any(must_change) {
        return Cow::Borrowed(text);
    }
    Cow::Owned(
        text.chars()
            .map(|ch| match ch {
                '\t' => ' ',
                ch if ch.is_control() || is_hidden(ch) => REPLACEMENT,
                ch => ch,
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_text_is_left_alone_and_not_copied() {
        for text in ["", "src/App.kt", "naïve 日本語 ✓ 🎉", "a b"] {
            assert!(matches!(visible(text), Cow::Borrowed(_)), "{text}");
            assert_eq!(visible(text), text);
        }
    }

    #[test]
    fn a_direction_override_in_a_name_becomes_a_visible_mark() {
        let name = "invoice\u{202E}fdp.exe";

        let shown = visible(name);

        assert_eq!(shown, "invoice\u{FFFD}fdp.exe");
        assert_eq!(shown.chars().count(), name.chars().count());
    }

    #[test]
    fn zero_width_isolates_separators_and_the_byte_order_mark_are_marked() {
        let hidden = "a\u{200B}b\u{2066}c\u{2029}d\u{FEFF}e\u{2060}f\u{200F}g";

        let shown = visible(hidden);

        assert_eq!(
            shown,
            "a\u{FFFD}b\u{FFFD}c\u{FFFD}d\u{FFFD}e\u{FFFD}f\u{FFFD}g"
        );
    }

    #[test]
    fn look_alike_blanks_and_tag_characters_are_marked_too() {
        for hidden in [
            '\u{2800}',
            '\u{061C}',
            '\u{3164}',
            '\u{00AD}',
            '\u{E0041}',
            '\u{E0100}',
            '\u{206A}',
            '\u{FFF9}',
        ] {
            assert!(is_hidden(hidden), "{hidden:?}");
            assert_eq!(visible(&format!("a{hidden}b")), "a\u{FFFD}b");
        }
    }

    #[test]
    fn ordinary_marks_in_real_names_are_left_alone() {
        // The emoji presentation selector is part of ordinary emoji.
        for fine in [
            "❤\u{FE0F}.txt",
            "e\u{0301}.kt",
            "日本語",
            "Привет",
            "a-b_c.d",
        ] {
            assert_eq!(visible(fine), fine, "{fine:?}");
        }
    }

    #[test]
    fn control_characters_are_marked_and_a_tab_is_a_space() {
        assert_eq!(
            visible("a\u{1b}[2Jb\nc\u{7}d"),
            "a\u{FFFD}[2Jb\u{FFFD}c\u{FFFD}d"
        );
        assert_eq!(visible("a\tb"), "a b");
    }

    #[test]
    fn the_length_in_characters_never_changes() {
        let text = "x\u{202A}\u{202B}\u{202C}\u{202D}\u{202E}\ty\u{85}z";

        assert_eq!(visible(text).chars().count(), text.chars().count());
    }
}
