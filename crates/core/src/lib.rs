//! Core of LF-Mide: immutable app state, actions, the reducer, config and
//! project detection. No terminal or process I/O lives here.

pub mod action;
pub mod config;
pub mod devices;
pub mod diff_view;
pub mod doctor;
mod editor_reducer;
pub mod git;
mod git_reducer;
pub mod layout;
pub mod logparse;
pub mod nav;
pub mod output;
mod output_reducer;
pub mod perf;
pub mod perf_reducer;
pub mod prefs;
pub mod project;
pub mod props;
pub mod reducer;
pub mod screen;
pub mod state;
pub mod text;
pub mod version;

pub use action::Action;
pub use config::{Config, ConfigError, EditorMode, TerminalKind, set_terminal_in_config};
pub use devices::{
    AndroidSdk, Device, DeviceState, Platform, SdkInfo, ToolInfo, XcodeSdk, display_path,
};
pub use diff_view::{DiffContent, DiffView, SideRow};
pub use doctor::{
    Category, CheckResult, LineKind, Report, ReportLine, Status, popup_content_rows, popup_height,
};
pub use git::{
    BaseKind, BaseRef, BlameLine, ChangeKind, DiffLine, FileChange, FileDiff, Hunk, LineMark,
    LineOrigin, MarkKind, blame_text, hunk_lines, marks_from_diff, relative_time,
};
pub use git_reducer::MarksKey;
pub use nav::{FileList, FileMatch, GrepHit, TreeRow};
pub use output::{
    BuildOutcome, BuildPhase, BuildRequest, BuildState, Job, LOG_CAPACITY, LogBuffer, LogLevel,
    LogRequest, MAX_PROBLEMS, OutputFilter, OutputTab, Problem, Severity,
};
pub use perf::{
    Health, PerfSample, Pressure, ProcessKind, ProcessStat, StopRequest, SwapLimits, SystemStats,
    Trend, format_cpu, format_size, swap_trend,
};
pub use prefs::{ProjectPrefs, prefs_file_name};
pub use project::{Project, ProjectError, ProjectLayout};
pub use reducer::reduce;
pub use state::{
    AppState, BranchInfo, DeviceAction, DeviceCommand, DoctorPanel, FinderState, GrepState,
    MAX_QUERY_CHARS, MAX_TRANSITION_POLLS, Panel, Transition,
};
