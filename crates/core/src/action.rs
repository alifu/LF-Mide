//! Everything that can change [`AppState`](crate::AppState).

use std::path::PathBuf;
use std::sync::Arc;

use crate::config::TerminalKind;
use std::collections::BTreeMap;

use crate::devices::{Device, Platform, SdkInfo};
use crate::doctor::Report;
use crate::git::{BaseRef, FileChange, FileDiff};
use crate::nav::{FileList, FileMatch, GrepHit};
use crate::output::{BuildOutcome, OutputTab};
use crate::perf::PerfSample;
use crate::prefs::ProjectPrefs;
use crate::screen::Screen;
use crate::state::BranchInfo;
use crate::state::Panel;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Quit; with our Neovim running, after it has closed.
    Quit,
    /// Quit at once (the terminal went away or the app was told to end).
    QuitNow,
    FocusNext,
    FocusPrev,
    BranchChanged(Option<BranchInfo>),
    /// A user-facing message shown in the status bar (e.g. a recoverable error).
    Status(String),
    /// Open the doctor overlay (starting a run) or close it.
    ToggleDoctor,
    /// Run the doctor again while the overlay is open.
    RefreshDoctor,
    /// A background doctor run finished.
    DoctorFinished(Report),
    /// Scroll the doctor report by this many lines (negative = up).
    DoctorScroll(i16),
    /// The terminal is now this many rows tall.
    ViewportChanged(u16),
    /// `TerminalKind::Auto` resolves to this terminal (or none).
    TerminalDetected(Option<TerminalKind>),
    OpenTerminalPicker,
    /// Move the picker highlight by this many rows (wraps around).
    TerminalPickerMove(i16),
    TerminalPickerSelect,
    TerminalPickerCancel,
    /// The user asked for Neovim in a split pane (the app performs it).
    OpenEditor,
    /// Neovim is running and listening on this socket.
    EditorOpened(PathBuf),
    /// The event loop has started Neovim.
    EditorLaunchStarted,
    /// Neovim could not be started (or only by hand); the file the user
    /// wanted to open is dropped.
    EditorLaunchFailed(String),
    /// The file waiting for Neovim has been handed over.
    PendingOpenSent,
    /// Neovim could not be reached; forget it so `e` starts a new one.
    EditorLost,
    /// Our own Neovim (in the editor panel) has been started.
    EmbeddedStarted,
    /// What that Neovim shows now.
    EditorScreen(Arc<Screen>),
    /// Keys for that Neovim, in its notation (the app sends them).
    EditorKeys(String),
    /// Pasted text for that Neovim (the app sends it).
    EditorPaste(String),
    /// That Neovim has ended.
    EditorExited,
    /// The user said Cancel to Neovim's question on quitting.
    EditorQuitCancelled,
    /// This project file is now showing in Neovim.
    FileOpened(PathBuf),
    /// A scan of the project finished.
    FilesLoaded(FileList),
    /// Scan the project again.
    RefreshFiles,
    /// Move the file-tree selection (negative = up).
    TreeMove(i16),
    /// Open or close the selected directory.
    TreeToggle,
    /// Open the selected directory, or step into an already open one.
    TreeExpand,
    /// Close the selected directory, or step out to its parent.
    TreeCollapse,
    /// Show this project file in Neovim, at `line` (1-based) if given.
    OpenFile {
        path: PathBuf,
        line: Option<u32>,
    },
    OpenFinder,
    FinderInput(char),
    FinderBackspace,
    /// Move the finder highlight (negative = up).
    FinderMove(i16),
    FinderCancel,
    /// Ranked matches for `query`; ignored if the query has changed since.
    FinderResults {
        query: String,
        matches: Vec<FileMatch>,
    },
    OpenGrep,
    GrepInput(char),
    GrepBackspace,
    GrepMove(i16),
    GrepCancel,
    /// Start searching for the typed query.
    GrepSearch,
    /// The outcome of the search for `query`; ignored if it is out of date.
    GrepResults {
        query: String,
        result: Result<Vec<GrepHit>, String>,
    },
    /// Give this panel the focus.
    Focus(Panel),
    /// Switch the build target between Android and iOS.
    ToggleTarget,
    /// What is installed for each platform.
    SdkLoaded(SdkInfo),
    /// A fresh device listing, and why a platform could not be listed.
    DevicesLoaded {
        devices: Arc<Vec<Device>>,
        errors: BTreeMap<Platform, String>,
    },
    /// List the devices again now.
    RefreshDevices,
    /// Move the device highlight within the current target (negative = up).
    DeviceMove(i16),
    /// Start the chosen device.
    BootDevice,
    /// Stop the chosen device.
    StopDevice,
    /// Starting or stopping `id` on `platform` failed.
    DeviceActionFailed {
        platform: Platform,
        id: String,
        message: String,
    },
    /// Choices remembered from an earlier run.
    PrefsLoaded(ProjectPrefs),
    /// Show or hide the environment (SDK) overlay.
    ToggleInfo,
    /// Build the app and run it on the chosen device.
    RunApp,
    /// Stop the build in progress.
    CancelBuild,
    /// What build `id` is doing now ("Installing", ...).
    BuildStep {
        id: u64,
        step: String,
    },
    /// Output lines of build `id`.
    BuildOutput {
        id: u64,
        lines: Vec<String>,
    },
    /// Build `id` ended after `millis`.
    BuildFinished {
        id: u64,
        outcome: BuildOutcome,
        millis: u64,
    },
    /// Follow (or stop following) the log of the chosen device.
    ToggleLogs,
    /// Lines from device log stream `id`.
    LogOutput {
        platform: Platform,
        id: u64,
        lines: Vec<String>,
    },
    /// Device log stream `id` ended, with the reason if it failed.
    LogEnded {
        platform: Platform,
        id: u64,
        reason: Option<String>,
    },
    /// Show this Output tab.
    ShowOutputTab(OutputTab),
    /// Move to the next (positive) or previous Output tab.
    OutputTabMove(i16),
    /// Scroll the Output panel; positive goes back in time.
    OutputScroll(i32),
    /// Jump to the newest line and follow from there.
    OutputFollow,
    /// Empty the log shown in the Output panel.
    ClearOutput,
    /// Start typing a text filter for the logs.
    FilterOpen,
    FilterInput(char),
    FilterBackspace,
    /// Stop typing but keep the filter.
    FilterDone,
    /// Drop the text filter and stop typing.
    FilterClear,
    /// Show only louder log lines (wraps back to all).
    CycleLogLevel,
    /// A new look at memory and the dev processes.
    PerfSampled(PerfSample),
    /// Open or close the performance panel.
    TogglePerfPanel,
    /// Show or hide the performance indicator in the status bar.
    TogglePerfIndicator,
    /// Move the highlight in the performance panel.
    PerfMove(i16),
    /// Stop the highlighted process (asks for a second press first).
    PerfStop,
    /// The repository was read: its HEAD and the files that differ from it.
    GitLoaded {
        head: Option<String>,
        changes: Arc<Vec<FileChange>>,
    },
    /// Read the repository again now.
    RefreshGit,
    /// Show the Git tab of the Output panel.
    ShowGit,
    /// Diff the file highlighted in the Git tab.
    OpenDiffSelected,
    /// Diff the file shown in Neovim.
    OpenDiffCurrent,
    /// The diff for request `id` is ready (or why it is not).
    DiffLoaded {
        id: u64,
        result: Result<Arc<FileDiff>, String>,
    },
    /// Scroll the diff by rows (positive goes down).
    DiffScroll(i32),
    /// Jump to the next (positive) or previous hunk.
    DiffHunk(i32),
    /// Switch between unified and side-by-side.
    DiffLayout,
    DiffClose,
    /// Choose what the diff is taken against.
    BasePickerOpen,
    BasePickerMove(i16),
    BasePickerSelect,
    BasePickerCancel,
    BasesLoaded(Arc<Vec<BaseRef>>),
    /// Show or hide blame text in Neovim.
    ToggleBlame,
    /// The gutter could not be drawn in Neovim; try again at the next look.
    MarksStale,
    /// Neovim says this absolute path is its current file.
    EditorFileChanged(Option<PathBuf>),
}
