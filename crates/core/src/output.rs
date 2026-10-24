//! What the Output panel shows: build state, log buffers, problems, and the
//! background jobs the reducer asks the app to run.

use std::collections::VecDeque;
use std::path::PathBuf;

use crate::devices::{Device, Platform};
use crate::git::BaseRef;
use crate::perf::ProcessKind;

/// Lines kept per log; older ones are dropped.
pub const LOG_CAPACITY: usize = 20_000;
/// Longest log line kept; the rest is cut.
pub const MAX_STORED_LINE_CHARS: usize = 1000;
/// Problems kept per build.
pub const MAX_PROBLEMS: usize = 500;

/// The tabs of the Output panel, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum OutputTab {
    Build,
    Logcat,
    IosLog,
    Problems,
    Git,
}

impl OutputTab {
    pub const ALL: [OutputTab; 5] = [
        OutputTab::Build,
        OutputTab::Logcat,
        OutputTab::IosLog,
        OutputTab::Problems,
        OutputTab::Git,
    ];

    pub fn label(self) -> &'static str {
        match self {
            OutputTab::Build => "Build",
            OutputTab::Logcat => "Logcat",
            OutputTab::IosLog => "iOS Log",
            OutputTab::Problems => "Problems",
            OutputTab::Git => "Git",
        }
    }

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }

    /// The tab `by` places along, wrapping around.
    pub fn moved(self, by: i16) -> OutputTab {
        let count = Self::ALL.len() as i16;
        let next = (self.index() as i16 + by).rem_euclid(count);
        Self::ALL[next as usize]
    }

    /// The device log tab for a platform.
    pub fn log_of(platform: Platform) -> OutputTab {
        match platform {
            Platform::Android => OutputTab::Logcat,
            Platform::Ios => OutputTab::IosLog,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Verbose,
    Debug,
    Info,
    Warn,
    Error,
    Fatal,
}

impl LogLevel {
    pub fn from_letter(letter: &str) -> Option<LogLevel> {
        match letter {
            "V" => Some(LogLevel::Verbose),
            "D" => Some(LogLevel::Debug),
            "I" => Some(LogLevel::Info),
            "W" => Some(LogLevel::Warn),
            "E" => Some(LogLevel::Error),
            "F" => Some(LogLevel::Fatal),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

/// A compiler error or warning found in build output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub severity: Severity,
    /// As printed by the tool: usually absolute, and not trusted.
    pub path: Option<String>,
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub message: String,
}

fn shortened(line: String) -> String {
    match line.char_indices().nth(MAX_STORED_LINE_CHARS) {
        None => line,
        Some((cut, _)) => format!("{}…", &line[..cut]),
    }
}

/// What the log tabs hide: lines without the text, and (logcat only) lines
/// quieter than the minimum level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputFilter {
    pub text: String,
    pub min_level: LogLevel,
    /// Keys go into `text` instead of acting as commands.
    pub editing: bool,
}

impl Default for OutputFilter {
    fn default() -> Self {
        Self {
            text: String::new(),
            min_level: LogLevel::Verbose,
            editing: false,
        }
    }
}

impl OutputFilter {
    pub fn is_active(&self) -> bool {
        !self.text.is_empty() || self.min_level > LogLevel::Verbose || self.editing
    }

    /// Whether `line` stays visible. `level` is the line's logcat level, if
    /// it has one and the tab uses levels.
    pub fn allows(&self, line: &str, level: Option<LogLevel>) -> bool {
        if line.starts_with("— ") {
            return true;
        }
        if self.min_level > LogLevel::Verbose && level.is_none_or(|l| l < self.min_level) {
            return false;
        }
        self.text.is_empty() || line.to_lowercase().contains(&self.text.to_lowercase())
    }
}

impl LogLevel {
    /// The next louder level, wrapping from the loudest back to all.
    pub fn cycled(self) -> LogLevel {
        match self {
            LogLevel::Verbose => LogLevel::Debug,
            LogLevel::Debug => LogLevel::Info,
            LogLevel::Info => LogLevel::Warn,
            LogLevel::Warn => LogLevel::Error,
            LogLevel::Error | LogLevel::Fatal => LogLevel::Verbose,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            LogLevel::Verbose => "Verbose",
            LogLevel::Debug => "Debug",
            LogLevel::Info => "Info",
            LogLevel::Warn => "Warn",
            LogLevel::Error => "Error",
            LogLevel::Fatal => "Fatal",
        }
    }
}

/// The last [`LOG_CAPACITY`] lines of a log.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LogBuffer {
    lines: VecDeque<String>,
    dropped: u64,
}

impl LogBuffer {
    pub fn push_lines(&mut self, lines: impl IntoIterator<Item = String>) {
        for line in lines {
            if self.lines.len() == LOG_CAPACITY {
                self.lines.pop_front();
                self.dropped += 1;
            }
            self.lines.push_back(shortened(line));
        }
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub fn get(&self, index: usize) -> Option<&str> {
        self.lines.get(index).map(String::as_str)
    }

    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.lines.iter().map(String::as_str)
    }

    pub fn clear(&mut self) {
        *self = LogBuffer::default();
    }

    /// How many older lines were dropped to stay within the capacity.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BuildPhase {
    #[default]
    Idle,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

/// The build in progress, or the last one.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BuildState {
    pub phase: BuildPhase,
    pub platform: Option<Platform>,
    /// Which build the events on the way belong to; the last one wins.
    pub id: u64,
    /// What the pipeline is doing: "Building", "Installing", "Launching".
    pub step: String,
    /// The Gradle task running right now.
    pub task: Option<String>,
    /// Name of the device it runs for.
    pub device: Option<String>,
    /// Its id: names repeat (one simulator per iOS runtime), ids do not.
    pub device_id: Option<String>,
    /// What the last run started on that device (package name or bundle
    /// id), once it is running; `leader+s` stops it.
    pub app_id: Option<String>,
    pub millis: Option<u64>,
    /// The clock at the latest tick of the running build.
    pub tick_ms: u64,
    /// The clock at the first tick of the running build.
    pub first_tick_ms: Option<u64>,
    pub errors: usize,
    pub warnings: usize,
}

impl BuildState {
    /// Milliseconds the running build has taken, as far as the ticks show.
    pub fn elapsed_ms(&self) -> u64 {
        self.first_tick_ms
            .map_or(0, |first| self.tick_ms.saturating_sub(first))
    }
}

/// How a build and run pipeline ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildOutcome {
    Succeeded,
    /// With the reason when the failure was not a compiler error.
    Failed(Option<String>),
    Cancelled,
}

/// Build and run on this device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildRequest {
    pub id: u64,
    pub device: Device,
    /// The variant or scheme to build; `None` builds the default.
    pub variant: Option<String>,
}

/// Follow the log of this device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogRequest {
    pub id: u64,
    pub device: Device,
    /// The app the last run launched on this device: the log can be
    /// limited to its process.
    pub app_id: Option<String>,
}

/// Stop this app on this device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StopAppRequest {
    pub device: Device,
    pub app_id: String,
}

/// Work for the app to start; queued by the reducer like device commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Job {
    Build(BuildRequest),
    CancelBuild {
        id: u64,
    },
    /// Read the project for the variants (Android) or schemes (iOS) it has.
    LoadVariants(Platform),
    StartLogs(LogRequest),
    StopLogs(Platform),
    StopApp(StopAppRequest),
    /// Compute the diff of `path` against `base`.
    LoadDiff {
        id: u64,
        path: PathBuf,
        base: BaseRef,
    },
    /// List what a diff can be taken against.
    LoadBases,
    /// Draw git signs (and blame) on `path` in Neovim.
    SyncMarks {
        socket: PathBuf,
        path: PathBuf,
        blame: bool,
    },
    /// Ask the Neovim on `socket` which file is current.
    FollowEditor {
        socket: PathBuf,
    },
    /// Ask a watched process to end (after checking it is still that process).
    StopProcess {
        pid: u32,
        kind: ProcessKind,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_move_and_wrap() {
        assert_eq!(OutputTab::Build.moved(1), OutputTab::Logcat);
        assert_eq!(OutputTab::Build.moved(-1), OutputTab::Git);
        assert_eq!(OutputTab::Git.moved(1), OutputTab::Build);
        assert_eq!(OutputTab::log_of(Platform::Ios), OutputTab::IosLog);
    }

    #[test]
    fn very_long_lines_are_cut_when_stored() {
        let mut buffer = LogBuffer::default();

        buffer.push_lines(["é".repeat(MAX_STORED_LINE_CHARS + 50), "short".to_string()]);

        assert_eq!(
            buffer.get(0).unwrap().chars().count(),
            MAX_STORED_LINE_CHARS + 1
        );
        assert!(buffer.get(0).unwrap().ends_with('…'));
        assert_eq!(buffer.get(1), Some("short"));
    }

    #[test]
    fn the_buffer_keeps_only_the_newest_lines_and_counts_the_rest() {
        let mut buffer = LogBuffer::default();

        buffer.push_lines((0..LOG_CAPACITY + 5).map(|n| n.to_string()));

        assert_eq!(buffer.len(), LOG_CAPACITY);
        assert_eq!(buffer.dropped(), 5);
        assert_eq!(buffer.get(0), Some("5"));
        assert_eq!(
            buffer.get(LOG_CAPACITY - 1),
            Some((LOG_CAPACITY + 4).to_string().as_str())
        );
        assert_eq!(buffer.get(LOG_CAPACITY), None);
    }

    #[test]
    fn the_filter_matches_text_without_regard_to_case_and_keeps_markers() {
        let filter = OutputFilter {
            text: "Boom".into(),
            ..OutputFilter::default()
        };

        assert!(filter.allows("something went BOOM here", None));
        assert!(!filter.allows("all fine", None));
        assert!(filter.allows("— following Pixel —", None));
        assert!(OutputFilter::default().allows("anything", None));
    }

    #[test]
    fn the_level_hides_quieter_and_unlevelled_lines() {
        let filter = OutputFilter {
            min_level: LogLevel::Warn,
            ..OutputFilter::default()
        };

        assert!(filter.allows("x", Some(LogLevel::Error)));
        assert!(filter.allows("x", Some(LogLevel::Warn)));
        assert!(!filter.allows("x", Some(LogLevel::Info)));
        assert!(!filter.allows("a stack trace line", None));
    }

    #[test]
    fn the_level_cycles_through_the_useful_ones_and_back() {
        let mut level = LogLevel::Verbose;
        let mut seen = Vec::new();
        for _ in 0..6 {
            level = level.cycled();
            seen.push(level.label());
        }

        assert_eq!(seen, ["Debug", "Info", "Warn", "Error", "Verbose", "Debug"]);
    }

    #[test]
    fn levels_are_ordered_by_loudness() {
        assert!(LogLevel::Error > LogLevel::Warn);
        assert!(LogLevel::Verbose < LogLevel::Debug);
        assert_eq!(LogLevel::from_letter("Z"), None);
    }
}
