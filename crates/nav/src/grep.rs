//! Project-wide text search through ripgrep.

use std::path::{Path, PathBuf};

use lf_mide_core::GrepHit;
use lf_mide_core::nav::clean_relative;
use lf_mide_system::{RunError, System};
use serde_json::Value;
use thiserror::Error;

use crate::scan::EXCLUDED_DIRS;

/// Longest line text kept for display.
const MAX_TEXT_CHARS: usize = 200;
/// Matches per file, so one huge file can't crowd out the rest.
const MAX_PER_FILE: &str = "20";
/// Larger files are not searched.
const MAX_FILE_SIZE: &str = "1M";

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum GrepError {
    #[error("ripgrep (rg) was not found: brew install ripgrep")]
    RgMissing,
    #[error("the search timed out")]
    TimedOut,
    #[error("{0}")]
    Failed(String),
}

/// The ripgrep arguments for searching `root` for `query`. The query is
/// passed after `-e`, so a leading `-` is part of the pattern, never a flag.
pub fn grep_args(query: &str) -> Vec<String> {
    let mut args: Vec<String> = [
        "--no-config",
        "--json",
        "--line-number",
        "--smart-case",
        "--max-columns",
        "400",
        "--max-count",
        MAX_PER_FILE,
        "--hidden",
        // Apply .gitignore files even without a `.git` directory, like the scan.
        "--no-require-git",
        // The scan ignores the user's global ignore file; so must the search.
        "--no-ignore-global",
        // `--max-columns` does not apply to `--json` output, so bound the work
        // on huge (often generated or hostile) files another way.
        "--max-filesize",
        MAX_FILE_SIZE,
    ]
    .map(String::from)
    .to_vec();
    for dir in EXCLUDED_DIRS {
        args.push("--glob".to_string());
        // Trailing slash: directories only, so a file named `build` is kept.
        args.push(format!("!{dir}/"));
    }
    args.extend(["-e", query, "--"].map(String::from));
    // The search runs inside the project folder and looks at `.`: the folder
    // is never written out as an argument, so a path that is not valid text
    // works, and results come back relative to it.
    args.push(".".to_string());
    args
}

/// Hits from ripgrep's `--json` output, paths made relative to `root`.
/// Lines that are not complete JSON (for example output cut off at a size
/// limit) are skipped, and so are paths outside the project.
pub fn parse_hits(output: &str, root: &Path, limit: usize) -> Vec<GrepHit> {
    output
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|event| event["type"] == "match")
        .filter_map(|event| hit_from(&event["data"], root))
        .take(limit)
        .collect()
}

/// The path ripgrep reports: text, or (for names that are not valid text)
/// the raw bytes in base64.
fn reported_path(data: &Value) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStringExt;

    if let Some(text) = data["path"]["text"].as_str() {
        return Some(PathBuf::from(text));
    }
    let bytes = decode_base64(data["path"]["bytes"].as_str()?)?;
    Some(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
}

/// Standard base64 with or without padding; `None` for anything else.
fn decode_base64(text: &str) -> Option<Vec<u8>> {
    let mut bytes = Vec::with_capacity(text.len() / 4 * 3);
    let mut bits = 0u32;
    let mut have = 0;
    for ch in text.trim_end_matches('=').chars() {
        let value = match ch {
            'A'..='Z' => ch as u32 - 'A' as u32,
            'a'..='z' => ch as u32 - 'a' as u32 + 26,
            '0'..='9' => ch as u32 - '0' as u32 + 52,
            '+' => 62,
            '/' => 63,
            _ => return None,
        };
        bits = (bits << 6) | value;
        have += 6;
        if have >= 8 {
            have -= 8;
            bytes.push((bits >> have) as u8);
            bits &= (1 << have) - 1;
        }
    }
    Some(bytes)
}

fn hit_from(data: &Value, root: &Path) -> Option<GrepHit> {
    let reported = reported_path(data)?;
    // Relative to the folder it ran in (`./src/App.kt`); an absolute path
    // must lie inside the project.
    let relative = if reported.is_absolute() {
        reported.strip_prefix(root).ok()?.to_path_buf()
    } else {
        reported
    };
    let path = clean_relative(&relative)?;
    let line = u32::try_from(data["line_number"].as_u64()?).ok()?;
    let column = data["submatches"][0]["start"]
        .as_u64()
        .and_then(|start| u32::try_from(start).ok())
        .unwrap_or(0);
    let text = data["lines"]["text"]
        .as_str()
        .unwrap_or("")
        .trim()
        .to_string();
    Some(GrepHit {
        path,
        line,
        column,
        text: shorten(&text),
    })
}

/// `text` cut to [`MAX_TEXT_CHARS`] characters.
fn shorten(text: &str) -> String {
    if text.chars().count() <= MAX_TEXT_CHARS {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(MAX_TEXT_CHARS).collect();
    cut.push('…');
    cut
}

/// The most useful line of ripgrep's error output.
fn summarize_error(stderr: &str) -> String {
    let lines: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let best = lines
        .iter()
        .find(|line| line.starts_with("error:"))
        .or_else(|| lines.first())
        .copied()
        .unwrap_or("ripgrep failed");
    shorten(best.strip_prefix("rg: ").unwrap_or(best))
}

/// Searches `root` for `query` (a regular expression, smart-case). A blank
/// query finds nothing. At most `limit` hits are returned.
pub fn grep(
    sys: &dyn System,
    root: &Path,
    query: &str,
    limit: usize,
) -> Result<Vec<GrepHit>, GrepError> {
    if query.trim().is_empty() || limit == 0 {
        return Ok(Vec::new());
    }
    let args = grep_args(query);
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match sys.run_in(root, "rg", &args) {
        Err(RunError::NotFound) => Err(GrepError::RgMissing),
        Err(RunError::TimedOut) => Err(GrepError::TimedOut),
        Err(RunError::Other(message)) => Err(GrepError::Failed(message)),
        Ok(output) => {
            // ripgrep searches files in parallel, so its order varies run to
            // run. Sort everything it gave us (its output is size-capped)
            // and only then cut to the limit, so the same hits are kept and
            // shown in the same order every time.
            let mut hits = parse_hits(&output.stdout, root, usize::MAX);
            hits.sort_by(|a, b| a.path.cmp(&b.path).then(a.line.cmp(&b.line)));
            hits.truncate(limit);
            // Exit code 1 (no matches) prints nothing; 2 with no hits is an
            // error worth showing. Warnings next to real hits are not.
            if hits.is_empty() && !output.success && !output.stderr.trim().is_empty() {
                return Err(GrepError::Failed(summarize_error(&output.stderr)));
            }
            Ok(hits)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lf_mide_system::SystemRunner;
    use lf_mide_system::fake::FakeSystem;
    use std::fs;
    use std::path::PathBuf;

    const ROOT: &str = "/work/pos";

    /// Real ripgrep output, shortened (see `rg --json -e setActive`).
    fn sample() -> String {
        let begin = r#"{"type":"begin","data":{"path":{"text":"/work/pos/src/App.kt"}}}"#;
        let first = r#"{"type":"match","data":{"path":{"text":"/work/pos/src/App.kt"},"lines":{"text":"    vm.setActive(true)\n"},"line_number":3,"absolute_offset":30,"submatches":[{"match":{"text":"setActive"},"start":7,"end":16}]}}"#;
        let end = r#"{"type":"end","data":{"path":{"text":"/work/pos/src/App.kt"},"binary_offset":null,"stats":{}}}"#;
        let second = r#"{"type":"match","data":{"path":{"text":"/work/pos/notes.txt"},"lines":{"text":"setActive twice: setActive\n"},"line_number":1,"absolute_offset":0,"submatches":[{"match":{"text":"setActive"},"start":0,"end":9},{"match":{"text":"setActive"},"start":17,"end":26}]}}"#;
        let summary = r#"{"type":"summary","data":{"stats":{"matches":3}}}"#;
        [begin, first, end, second, summary].join("\n") + "\n"
    }

    #[test]
    fn args_search_the_root_with_the_query_after_dash_e() {
        let args = grep_args("-rf evil");

        let e = args.iter().position(|a| a == "-e").unwrap();
        assert_eq!(args[e + 1], "-rf evil");
        assert_eq!(args[e + 2], "--");
        assert_eq!(
            args.last().unwrap(),
            ".",
            "searched from inside the project"
        );
        assert!(
            !args.iter().any(|a| a == ROOT),
            "the folder is never an argument"
        );
        assert!(args.contains(&"--json".to_string()));
        assert!(
            args.contains(&"--no-config".to_string()),
            "the user's rg config must not change results"
        );
        assert!(args.contains(&"--smart-case".to_string()));
    }

    #[test]
    fn args_exclude_the_same_directories_as_the_file_scan() {
        let args = grep_args("x");

        for dir in EXCLUDED_DIRS {
            // A trailing slash makes the glob match directories only, so a
            // file that happens to be called `build` is still searched.
            assert!(args.contains(&format!("!{dir}/")), "missing !{dir}/");
        }
        assert!(args.contains(&"--hidden".to_string()));
        assert!(args.contains(&"--no-require-git".to_string()));
    }

    #[test]
    fn args_bound_the_work_ripgrep_does_on_hostile_files_and_match_the_scan_rules() {
        let args = grep_args("x");

        let at = args
            .iter()
            .position(|a| a == "--max-filesize")
            .expect("--max-filesize");
        assert_eq!(args[at + 1], "1M", "huge generated files are skipped");
        assert!(
            args.contains(&"--no-ignore-global".to_string()),
            "like the scan, ignore the user's global ignore file"
        );
    }

    #[test]
    fn parses_matches_into_relative_hits_with_line_and_column() {
        let hits = parse_hits(&sample(), Path::new(ROOT), 100);

        assert_eq!(
            hits,
            vec![
                GrepHit {
                    path: PathBuf::from("src/App.kt"),
                    line: 3,
                    column: 7,
                    text: "vm.setActive(true)".into()
                },
                GrepHit {
                    path: PathBuf::from("notes.txt"),
                    line: 1,
                    column: 0,
                    text: "setActive twice: setActive".into()
                },
            ]
        );
    }

    #[test]
    fn skips_incomplete_or_foreign_lines() {
        let truncated = &sample()[..sample().len() - 20];
        let outside = r#"{"type":"match","data":{"path":{"text":"/elsewhere/x.kt"},"lines":{"text":"a\n"},"line_number":1,"submatches":[]}}"#;
        let junk = "not json at all\n{\"type\":\n";
        let escaping = r#"{"type":"match","data":{"path":{"text":"../secret.kt"},"lines":{"text":"a\n"},"line_number":1,"submatches":[]}}"#;
        let input = format!("{junk}{outside}\n{escaping}\n{truncated}");

        let hits = parse_hits(&input, Path::new(ROOT), 100);

        assert_eq!(hits.len(), 2, "{hits:?}");
        assert!(hits.iter().all(|h| !h.path.is_absolute()));
    }

    #[test]
    fn paths_relative_to_the_search_folder_are_understood() {
        let line = r#"{"type":"match","data":{"path":{"text":"./src/App.kt"},"lines":{"text":"x\n"},"line_number":4,"submatches":[]}}"#;

        let hits = parse_hits(line, Path::new(ROOT), 5);

        assert_eq!(hits[0].path, PathBuf::from("src/App.kt"));
    }

    #[test]
    fn a_name_that_is_not_valid_text_comes_back_byte_for_byte() {
        use std::os::unix::ffi::OsStrExt;

        // "dir/caf\xe9.kt" in Latin-1: not valid UTF-8.
        let line = r#"{"type":"match","data":{"path":{"bytes":"Li9kaXIvY2Fm6S5rdA=="},"lines":{"text":"x\n"},"line_number":2,"submatches":[]}}"#;

        let hits = parse_hits(line, Path::new(ROOT), 5);

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path.as_os_str().as_bytes(), b"dir/caf\xe9.kt");
    }

    #[test]
    fn base64_is_decoded_with_or_without_padding_and_bad_input_is_refused() {
        assert_eq!(decode_base64("aGk=").unwrap(), b"hi");
        assert_eq!(decode_base64("aGk").unwrap(), b"hi");
        assert_eq!(decode_base64("aGVsbG8gd29ybGQ=").unwrap(), b"hello world");
        assert_eq!(decode_base64("").unwrap(), b"");
        assert_eq!(decode_base64("a*k="), None);
        assert_eq!(decode_base64("aGk\n"), None);
    }

    #[test]
    fn stops_at_the_limit() {
        assert_eq!(parse_hits(&sample(), Path::new(ROOT), 1).len(), 1);
        assert!(parse_hits(&sample(), Path::new(ROOT), 0).is_empty());
    }

    #[test]
    fn long_lines_are_shortened_and_matches_without_submatches_get_column_zero() {
        let long = "x".repeat(1000);
        let line = format!(
            r#"{{"type":"match","data":{{"path":{{"text":"/work/pos/a.kt"}},"lines":{{"text":"{long}\n"}},"line_number":9,"submatches":[]}}}}"#
        );

        let hits = parse_hits(&line, Path::new(ROOT), 5);

        assert_eq!(hits[0].text.chars().count(), MAX_TEXT_CHARS + 1);
        assert!(hits[0].text.ends_with('…'));
        assert_eq!(hits[0].column, 0);
    }

    fn key(query: &str) -> String {
        format!("rg {}", grep_args(query).join(" "))
    }

    #[test]
    fn grep_returns_the_parsed_hits() {
        let sys = FakeSystem::new().ok(&key("setActive"), &sample());

        let hits = grep(&sys, Path::new(ROOT), "setActive", 50).unwrap();
        assert_eq!(
            sys.ran_in()[0].0,
            Path::new(ROOT),
            "ripgrep runs inside the project folder"
        );

        assert_eq!(hits.len(), 2);
    }

    #[test]
    fn hits_come_back_sorted_by_file_then_line_whatever_order_ripgrep_used() {
        let line = |path: &str, number: u32| {
            format!(
                r#"{{"type":"match","data":{{"path":{{"text":"/work/pos/{path}"}},"lines":{{"text":"x\n"}},"line_number":{number},"submatches":[]}}}}"#
            )
        };
        let output = [
            line("z.kt", 1),
            line("a.kt", 9),
            line("a.kt", 2),
            line("m/b.kt", 5),
        ]
        .join("\n");
        let sys = FakeSystem::new().ok(&key("x"), &output);

        let hits = grep(&sys, Path::new(ROOT), "x", 50).unwrap();

        let order: Vec<_> = hits
            .iter()
            .map(|h| (h.path.display().to_string(), h.line))
            .collect();
        assert_eq!(
            order,
            vec![
                ("a.kt".to_string(), 2),
                ("a.kt".to_string(), 9),
                ("m/b.kt".to_string(), 5),
                ("z.kt".to_string(), 1)
            ]
        );
    }

    #[test]
    fn the_limit_keeps_the_first_hits_in_sorted_order_not_whichever_came_first() {
        let line = |path: &str, number: u32| {
            format!(
                r#"{{"type":"match","data":{{"path":{{"text":"/work/pos/{path}"}},"lines":{{"text":"x\n"}},"line_number":{number},"submatches":[]}}}}"#
            )
        };
        // ripgrep's file order is random; the 2 hits kept must still be the
        // two that sort first.
        let output = [
            line("z.kt", 1),
            line("m.kt", 1),
            line("a.kt", 7),
            line("b.kt", 3),
        ]
        .join("\n");
        let sys = FakeSystem::new().ok(&key("x"), &output);

        let hits = grep(&sys, Path::new(ROOT), "x", 2).unwrap();

        let paths: Vec<_> = hits.iter().map(|h| h.path.display().to_string()).collect();
        assert_eq!(paths, vec!["a.kt", "b.kt"]);
    }

    #[test]
    fn grep_treats_no_matches_as_an_empty_result_and_blank_queries_as_nothing_to_do() {
        let sys = FakeSystem::new().exits_nonzero(&key("zzz"));

        assert_eq!(grep(&sys, Path::new(ROOT), "zzz", 50), Ok(vec![]));
        assert_eq!(
            grep(&FakeSystem::new(), Path::new(ROOT), "   ", 50),
            Ok(vec![])
        );
    }

    #[test]
    fn grep_explains_a_bad_pattern_in_one_line() {
        let stderr = "rg: regex parse error:\n    (?:()\n    ^\nerror: unclosed group\n";
        let sys = FakeSystem::new().fails(&key("("), stderr);

        let err = grep(&sys, Path::new(ROOT), "(", 50).unwrap_err();

        assert_eq!(err, GrepError::Failed("error: unclosed group".into()));
    }

    #[test]
    fn grep_reports_a_missing_ripgrep_and_a_timeout() {
        assert_eq!(
            grep(&FakeSystem::new(), Path::new(ROOT), "x", 5),
            Err(GrepError::RgMissing)
        );
        let slow = FakeSystem::new().timeout(&key("x"));
        assert_eq!(
            grep(&slow, Path::new(ROOT), "x", 5),
            Err(GrepError::TimedOut)
        );
    }

    #[test]
    fn grep_keeps_partial_results_when_ripgrep_also_printed_warnings() {
        // e.g. one unreadable file: exit code 2, but matches were found.
        let sys = FakeSystem::new().fails_with_output(
            &key("setActive"),
            &sample(),
            "rg: /work/pos/secret: Permission denied\n",
        );

        let hits = grep(&sys, Path::new(ROOT), "setActive", 50).unwrap();

        assert_eq!(hits.len(), 2);
    }

    fn real_rg_available() -> bool {
        SystemRunner::new()
            .run("rg", &["--version"])
            .is_ok_and(|o| o.success)
    }

    #[test]
    fn real_ripgrep_finds_hits_and_honors_the_project_rules() {
        if !real_rg_available() {
            return;
        }
        let dir = tempfile::TempDir::new().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        let write = |rel: &str, text: &str| {
            let path = root.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        };
        write("src/App.kt", "fun main() {\n    vm.setActive(true)\n}\n");
        write(".github/ci.yml", "run: setActive\n");
        write("build/out.txt", "setActive in build output\n");
        write(".git/config", "setActive in git\n");
        write("ignored.txt", "setActive but ignored\n");
        write(".gitignore", "ignored.txt\n");
        write("-rf", "setActive in a file named like a flag\n");

        let hits = grep(&SystemRunner::new(), &root, "setActive", 100).unwrap();

        let mut found: Vec<String> = hits.iter().map(|h| h.path.display().to_string()).collect();
        found.sort();
        assert_eq!(found, vec!["-rf", ".github/ci.yml", "src/App.kt"]);
        let app = hits
            .iter()
            .find(|h| h.path == Path::new("src/App.kt"))
            .unwrap();
        assert_eq!((app.line, app.column), (2, 7));
        // A pattern that looks like a flag is just a pattern.
        assert!(grep(&SystemRunner::new(), &root, "-rf", 100).is_ok());
    }

    #[test]
    fn real_ripgrep_reports_a_bad_pattern() {
        if !real_rg_available() {
            return;
        }
        let dir = tempfile::TempDir::new().unwrap();
        fs::write(dir.path().join("a.txt"), "x").unwrap();

        let err = grep(&SystemRunner::new(), dir.path(), "(", 10).unwrap_err();

        assert!(
            matches!(err, GrepError::Failed(ref m) if m.contains("unclosed group")),
            "{err:?}"
        );
    }
}
