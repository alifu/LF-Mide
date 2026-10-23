//! Quoting for the two languages a split command passes through: POSIX
//! `sh` (what terminals use to run a command string) and AppleScript (what
//! iTerm2 and Terminal.app are scripted with). Folder names come from the
//! repository, so both must hold for any input.

/// `arg` as one `sh` word that expands to exactly `arg`.
pub fn shell_quote(arg: &str) -> String {
    // Inside single quotes nothing is special except the quote itself:
    // close the string, emit an escaped quote, reopen.
    format!("'{}'", arg.replace('\'', "'\\''"))
}

/// `arg` as a shell word, left bare when every character is plain (so a
/// terminal that splits the command itself, without a shell, still sees one
/// word) and quoted otherwise. Only for text we control, such as our own
/// launcher path; repo-controlled text never goes through here.
pub fn shell_word(arg: &str) -> String {
    let plain = |c: char| c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c);
    let bare = !arg.is_empty() && arg.chars().all(plain) && !arg.starts_with(['-', '~']);
    if bare {
        arg.to_string()
    } else {
        shell_quote(arg)
    }
}

/// `text` as an AppleScript string literal, quotes included.
pub fn applescript_quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    /// Strings chosen to break naive quoting.
    const HOSTILE: &[&str] = &[
        "plain",
        "with space",
        "it's",
        "'",
        "''",
        "$(touch /tmp/lf-mide-pwned)",
        "`id`",
        "a\"b",
        "a\\b",
        "\\\"",
        "line1\nline2",
        "tab\there",
        "; rm -rf ~",
        "&& echo hi",
        "--flag",
        "*",
        "~",
        "$HOME",
        "é✓日本",
        "",
    ];

    fn osascript_available() -> bool {
        Command::new("osascript")
            .arg("-e")
            .arg("return 1")
            .output()
            .is_ok()
    }

    #[test]
    fn shell_quote_shapes() {
        assert_eq!(shell_quote("plain"), "'plain'");
        assert_eq!(shell_quote(""), "''");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }

    #[test]
    fn shell_quote_round_trips_through_a_real_shell() {
        for text in HOSTILE {
            let script = format!("printf %s {}", shell_quote(text));

            let out = Command::new("sh").arg("-c").arg(&script).output().unwrap();

            assert_eq!(String::from_utf8_lossy(&out.stdout), *text, "for {text:?}");
        }
        assert!(!std::path::Path::new("/tmp/lf-mide-pwned").exists());
    }

    #[test]
    fn shell_word_leaves_plain_paths_bare_and_quotes_the_rest() {
        assert_eq!(
            shell_word("/var/folders/x/T/lf-mide-42/open-nvim.sh"),
            "/var/folders/x/T/lf-mide-42/open-nvim.sh"
        );
        assert_eq!(shell_word("/path with space/x"), "'/path with space/x'");
        assert_eq!(shell_word("~/x"), "'~/x'");
        assert_eq!(shell_word("-rf"), "'-rf'");
        assert_eq!(shell_word("it's"), "'it'\\''s'");
        assert_eq!(shell_word(""), "''");
    }

    #[test]
    fn shell_word_round_trips_through_a_real_shell() {
        for text in
            HOSTILE
                .iter()
                .copied()
                .chain(["/plain/path-1.0_x/open.sh", "a=b", "x,y:z+w@v%u"])
        {
            let script = format!("printf %s {}", shell_word(text));

            let out = Command::new("sh").arg("-c").arg(&script).output().unwrap();

            assert_eq!(String::from_utf8_lossy(&out.stdout), text, "for {text:?}");
        }
    }

    #[test]
    fn shell_quote_keeps_several_words_separate() {
        let script = format!("printf '%s|' {} {}", shell_quote("a b"), shell_quote("c;d"));

        let out = Command::new("sh").arg("-c").arg(&script).output().unwrap();

        assert_eq!(String::from_utf8_lossy(&out.stdout), "a b|c;d|");
    }

    #[test]
    fn applescript_quote_shapes() {
        assert_eq!(applescript_quote("plain"), "\"plain\"");
        assert_eq!(applescript_quote("a\"b"), "\"a\\\"b\"");
        assert_eq!(applescript_quote("a\\b"), "\"a\\\\b\"");
        assert_eq!(applescript_quote("a\nb"), "\"a\\nb\"");
    }

    #[test]
    fn applescript_quote_never_leaves_a_raw_quote_or_newline_inside() {
        for text in HOSTILE {
            let quoted = applescript_quote(text);
            let inner = &quoted[1..quoted.len() - 1];

            assert!(quoted.starts_with('"') && quoted.ends_with('"'));
            assert!(!inner.contains('\n'), "raw newline for {text:?}");
            // Every quote inside must be escaped by an odd run of backslashes.
            let bytes = inner.as_bytes();
            for (i, byte) in bytes.iter().enumerate() {
                if *byte == b'"' {
                    let slashes = bytes[..i].iter().rev().take_while(|b| **b == b'\\').count();
                    assert_eq!(slashes % 2, 1, "unescaped quote for {text:?}: {quoted}");
                }
            }
        }
    }

    #[test]
    fn applescript_quote_round_trips_through_real_osascript() {
        if !osascript_available() {
            return;
        }
        for text in HOSTILE
            .iter()
            .filter(|t| !t.is_empty() && !t.ends_with('\n'))
        {
            let script = format!("return {}", applescript_quote(text));

            let out = Command::new("osascript")
                .arg("-e")
                .arg(&script)
                .output()
                .unwrap();

            let printed = String::from_utf8_lossy(&out.stdout);
            assert_eq!(
                printed.strip_suffix('\n').unwrap_or(&printed),
                *text,
                "for {text:?}"
            );
        }
    }
}
