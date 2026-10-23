//! Reading build and device log lines: which Gradle task is running, what
//! the compilers complain about, how a build ended, and how loud a logcat
//! line is. Everything here is a pure function of one line of text.

use crate::output::{LogLevel, Problem, Severity};

/// What one line of build output means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildLine {
    /// Gradle started this task (`> Task :androidApp:compileDebugKotlin`).
    Task(String),
    /// A compiler error or warning.
    Problem(Problem),
    /// The build tool announced its result.
    Finished {
        succeeded: bool,
    },
    Other,
}

/// Longest message kept for a problem.
const MAX_MESSAGE_CHARS: usize = 300;

pub fn parse_build_line(line: &str) -> BuildLine {
    let line = line.trim_end();
    if let Some(task) = gradle_task(line) {
        return BuildLine::Task(task);
    }
    if let Some(succeeded) = build_result(line) {
        return BuildLine::Finished { succeeded };
    }
    if let Some(problem) = kotlin_problem(line).or_else(|| compiler_problem(line)) {
        return BuildLine::Problem(problem);
    }
    BuildLine::Other
}

fn gradle_task(line: &str) -> Option<String> {
    let rest = line.strip_prefix("> Task ")?;
    let name = rest.split_whitespace().next()?;
    Some(name.to_string())
}

fn build_result(line: &str) -> Option<bool> {
    let line = line.trim_matches(|c: char| c == '*' || c.is_whitespace());
    match line {
        "BUILD SUCCEEDED" => Some(true),
        "BUILD FAILED" => Some(false),
        _ if line.starts_with("BUILD SUCCESSFUL") => Some(true),
        _ if line.starts_with("BUILD FAILED") => Some(false),
        _ => None,
    }
}

/// `e: file:///dir/A.kt:12:34 message` (also `w:`, and without `file://`).
fn kotlin_problem(line: &str) -> Option<Problem> {
    let (severity, rest) = if let Some(rest) = line.strip_prefix("e: ") {
        (Severity::Error, rest)
    } else {
        (Severity::Warning, line.strip_prefix("w: ")?)
    };
    let (rest, is_url) = match rest.strip_prefix("file://") {
        Some(url) => (url, true),
        None => (rest, false),
    };
    if !rest.starts_with('/') {
        return None;
    }
    let (location, message) = split_location(rest)?;
    let (path, line_no, column) = parse_location(location)?;
    let path = if is_url { percent_decoded(&path) } else { path };
    Some(problem(severity, Some(path), line_no, column, message))
}

/// `/dir/A.swift:12:5: error: message` (clang, swiftc, javac with a column
/// or without) and the bare `error: message` of linkers and xcodebuild.
fn compiler_problem(line: &str) -> Option<Problem> {
    if let Some((severity, message)) = leading_severity(line) {
        return Some(problem(severity, None, None, None, message));
    }
    for (marker, severity) in [
        (": error: ", Severity::Error),
        (": warning: ", Severity::Warning),
    ] {
        let Some(at) = line.find(marker) else {
            continue;
        };
        let (location, message) = (&line[..at], &line[at + marker.len()..]);
        if !location.starts_with('/') {
            continue;
        }
        let (path, line_no, column) = parse_location(location)?;
        return Some(problem(severity, Some(path), line_no, column, message));
    }
    None
}

/// `error: ...`, also after a tool name (`clang: error: ...`,
/// `ld: warning: ...`) or `fatal`.
fn leading_severity(line: &str) -> Option<(Severity, &str)> {
    severity_prefix(line).or_else(|| {
        let (tool, rest) = line.split_once(": ")?;
        let tool_name = !tool.is_empty()
            && tool
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '+' | '-'));
        tool_name.then(|| severity_prefix(rest)).flatten()
    })
}

fn severity_prefix(text: &str) -> Option<(Severity, &str)> {
    let text = text.strip_prefix("fatal ").unwrap_or(text);
    if let Some(message) = text.strip_prefix("error: ") {
        return Some((Severity::Error, message));
    }
    text.strip_prefix("warning: ")
        .map(|message| (Severity::Warning, message))
}

/// `%20` and friends in a `file://` URL.
fn percent_decoded(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let escaped = (bytes[index] == b'%')
            .then(|| bytes.get(index + 1..index + 3))
            .flatten()
            .and_then(|hex| std::str::from_utf8(hex).ok())
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match escaped {
            Some(byte) => {
                out.push(byte);
                index += 3;
            }
            None => {
                out.push(bytes[index]);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Splits `path:12:34 message` / `path:12:34: message` at the end of the
/// location.
fn split_location(text: &str) -> Option<(&str, &str)> {
    let mut end = 0;
    let mut colons = 0;
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b':' {
            let digits = bytes[index + 1..]
                .iter()
                .take_while(|b| b.is_ascii_digit())
                .count();
            if digits > 0 {
                colons += 1;
                end = index + 1 + digits;
                index = end;
                continue;
            }
        }
        if colons > 0 {
            break;
        }
        index += 1;
    }
    if colons == 0 {
        return None;
    }
    let message = text[end..].trim_start_matches(':').trim_start();
    Some((&text[..end], message))
}

/// `path`, `path:12` or `path:12:34`.
fn parse_location(location: &str) -> Option<(String, Option<u32>, Option<u32>)> {
    let mut parts = location.rsplitn(3, ':');
    let last = parts.next()?;
    let middle = parts.next();
    let first = parts.next();
    match (first, middle) {
        (Some(path), Some(line)) => Some((path.to_string(), line.parse().ok(), last.parse().ok())),
        (None, Some(path)) => Some((path.to_string(), last.parse().ok(), None)),
        _ => None,
    }
    .filter(|(path, _, _)| !path.is_empty())
}

fn problem(
    severity: Severity,
    path: Option<String>,
    line: Option<u32>,
    column: Option<u32>,
    message: &str,
) -> Problem {
    Problem {
        severity,
        path,
        line,
        column,
        message: message.trim().chars().take(MAX_MESSAGE_CHARS).collect(),
    }
}

/// The level of a `logcat -v threadtime` line:
/// `01-02 03:04:05.678  1234  5678 E Tag: message`.
pub fn logcat_level(line: &str) -> Option<LogLevel> {
    let mut fields = line.split_whitespace();
    let (date, time) = (fields.next()?, fields.next()?);
    if !date.contains('-') || !time.contains(':') {
        return None;
    }
    let (_pid, _tid) = (fields.next()?, fields.next()?);
    LogLevel::from_letter(fields.next()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn problem_of(line: &str) -> Problem {
        match parse_build_line(line) {
            BuildLine::Problem(p) => p,
            other => panic!("not a problem: {other:?}"),
        }
    }

    #[test]
    fn gradle_tasks_are_recognised_without_their_outcome() {
        assert_eq!(
            parse_build_line("> Task :androidApp:compileDebugKotlin"),
            BuildLine::Task(":androidApp:compileDebugKotlin".into())
        );
        assert_eq!(
            parse_build_line("> Task :shared:generateResourceAccessors UP-TO-DATE"),
            BuildLine::Task(":shared:generateResourceAccessors".into())
        );
        assert_eq!(parse_build_line("> Configure project :"), BuildLine::Other);
    }

    #[test]
    fn build_results_from_gradle_and_xcodebuild() {
        let ok = BuildLine::Finished { succeeded: true };
        let bad = BuildLine::Finished { succeeded: false };

        assert_eq!(parse_build_line("BUILD SUCCESSFUL in 12s"), ok);
        assert_eq!(parse_build_line("BUILD FAILED in 3s"), bad);
        assert_eq!(parse_build_line("** BUILD SUCCEEDED **"), ok);
        assert_eq!(parse_build_line("** BUILD FAILED **"), bad);
    }

    #[test]
    fn kotlin_errors_with_a_file_url() {
        let p = problem_of("e: file:///Users/me/app/src/App.kt:12:34 Unresolved reference: foo");

        assert_eq!(p.severity, Severity::Error);
        assert_eq!(p.path.as_deref(), Some("/Users/me/app/src/App.kt"));
        assert_eq!((p.line, p.column), (Some(12), Some(34)));
        assert_eq!(p.message, "Unresolved reference: foo");
    }

    #[test]
    fn kotlin_warnings_and_plain_paths_and_a_colon_after_the_location() {
        let warning = problem_of("w: file:///a/B.kt:3:1 Variable 'x' is never used");
        let plain = problem_of("e: /a/B.kt:7:2: Type mismatch");

        assert_eq!(warning.severity, Severity::Warning);
        assert_eq!(plain.path.as_deref(), Some("/a/B.kt"));
        assert_eq!(plain.message, "Type mismatch");
    }

    #[test]
    fn swift_and_clang_errors_with_line_and_column() {
        let p = problem_of("/Users/me/ios/ContentView.swift:21:9: error: cannot find 'x' in scope");

        assert_eq!(p.severity, Severity::Error);
        assert_eq!(p.path.as_deref(), Some("/Users/me/ios/ContentView.swift"));
        assert_eq!((p.line, p.column), (Some(21), Some(9)));
        assert_eq!(p.message, "cannot find 'x' in scope");
    }

    #[test]
    fn javac_style_errors_have_a_line_but_no_column() {
        let p = problem_of("/src/Main.java:10: error: ';' expected");

        assert_eq!((p.line, p.column), (Some(10), None));
        assert_eq!(p.message, "';' expected");
    }

    #[test]
    fn a_path_with_spaces_and_colons_keeps_them() {
        let p = problem_of("/Users/me/My App/a:b.swift:5:2: warning: unused");

        assert_eq!(p.severity, Severity::Warning);
        assert_eq!(p.path.as_deref(), Some("/Users/me/My App/a:b.swift"));
    }

    #[test]
    fn linker_and_fatal_errors_are_recognised_after_a_tool_name() {
        let ld = problem_of("ld: error: undefined symbol: _foo");
        let clang = problem_of("clang: error: linker command failed with exit code 1");
        let fatal = problem_of("fatal error: 'x.h' file not found");
        let ld_warning = problem_of("ld: warning: dylib was built for newer version");

        assert_eq!(ld.message, "undefined symbol: _foo");
        assert_eq!(clang.severity, Severity::Error);
        assert_eq!(fatal.message, "'x.h' file not found");
        assert_eq!(ld_warning.severity, Severity::Warning);
        assert!(ld.path.is_none());
    }

    #[test]
    fn a_file_url_is_percent_decoded_but_a_plain_path_is_not() {
        let url = problem_of("e: file:///Users/me/My%20App/A.kt:1:2 bad");
        let plain = problem_of("e: /Users/me/100%25/A.kt:1:2 bad");

        assert_eq!(url.path.as_deref(), Some("/Users/me/My App/A.kt"));
        assert_eq!(plain.path.as_deref(), Some("/Users/me/100%25/A.kt"));
        assert_eq!(percent_decoded("%zz%4"), "%zz%4");
    }

    #[test]
    fn bare_errors_have_no_location() {
        let p = problem_of("error: Signing for \"iosApp\" requires a development team.");

        assert_eq!(p.path, None);
        assert_eq!(
            p.message,
            "Signing for \"iosApp\" requires a development team."
        );
        assert_eq!(
            problem_of("warning: deprecated").severity,
            Severity::Warning
        );
    }

    #[test]
    fn ordinary_lines_are_not_problems() {
        for line in [
            "",
            "Compiling App.kt",
            "FAILURE: Build failed with an exception.",
            "* What went wrong:",
            "see: https://docs.gradle.org/error: here",
            "e: not a path",
            "relative/File.kt:1:1: error: nope",
        ] {
            assert_eq!(parse_build_line(line), BuildLine::Other, "{line:?}");
        }
    }

    #[test]
    fn a_huge_message_is_shortened() {
        let line = format!("e: /a/B.kt:1:1 {}", "x".repeat(5000));

        assert_eq!(problem_of(&line).message.chars().count(), MAX_MESSAGE_CHARS);
    }

    #[test]
    fn logcat_levels_come_from_the_threadtime_format() {
        let line = |level: &str| format!("10-24 09:15:02.123  4321  4350 {level} MyTag: hello");

        assert_eq!(logcat_level(&line("E")), Some(LogLevel::Error));
        assert_eq!(logcat_level(&line("W")), Some(LogLevel::Warn));
        assert_eq!(logcat_level(&line("I")), Some(LogLevel::Info));
        assert_eq!(logcat_level(&line("D")), Some(LogLevel::Debug));
        assert_eq!(logcat_level(&line("V")), Some(LogLevel::Verbose));
        assert_eq!(logcat_level(&line("F")), Some(LogLevel::Fatal));
    }

    #[test]
    fn lines_that_are_not_threadtime_have_no_level() {
        assert_eq!(logcat_level("--------- beginning of main"), None);
        assert_eq!(logcat_level("hello world foo bar baz"), None);
        assert_eq!(logcat_level("10-24 09:15:02.123  1 2 X Tag: m"), None);
        assert_eq!(logcat_level(""), None);
    }
}
