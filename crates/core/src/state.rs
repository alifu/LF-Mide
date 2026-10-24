//! Application state. Treated as immutable: the reducer returns new values.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;

use crate::config::{Config, TerminalKind};
use crate::devices::{Device, DeviceState, Platform, SdkInfo};
use crate::diff_view::DiffView;
use crate::doctor::Report;
use crate::git::{BaseRef, FileChange};
use crate::git_reducer::MarksKey;
use crate::nav::{FileList, FileMatch, GrepHit, TreeIndex, TreeRow};
use crate::output::{BuildState, Job, LogBuffer, OutputFilter, OutputTab, Problem};
use crate::perf::{PerfSample, StopRequest, Trend};
use crate::prefs::ProjectPrefs;
use crate::project::Project;

/// Focusable areas of the main layout, in Tab order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    Files,
    Editor,
    Devices,
    Bottom,
}

impl Panel {
    pub const ORDER: [Panel; 4] = [Panel::Files, Panel::Editor, Panel::Devices, Panel::Bottom];

    pub fn next(self) -> Panel {
        self.offset(1)
    }

    pub fn prev(self) -> Panel {
        self.offset(Self::ORDER.len() - 1)
    }

    fn offset(self, by: usize) -> Panel {
        let index = Self::ORDER
            .iter()
            .position(|panel| *panel == self)
            .unwrap_or(0);
        Self::ORDER[(index + by) % Self::ORDER.len()]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchInfo {
    /// Branch name, or the short commit id when HEAD is detached.
    pub name: String,
    pub detached: bool,
    pub dirty: bool,
}

/// The doctor overlay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DoctorPanel {
    Closed,
    /// Checks are running in the background.
    Running,
    Ready(Report),
}

/// The fuzzy file finder overlay.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FinderState {
    pub query: String,
    /// Best matches for `query`, filled in after each change of the query.
    pub matches: Arc<Vec<FileMatch>>,
    pub selected: usize,
}

/// The project search overlay.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GrepState {
    pub query: String,
    /// The query the current `hits` belong to, once a search was started.
    pub searched: Option<String>,
    pub hits: Arc<Vec<GrepHit>>,
    pub selected: usize,
    pub running: bool,
    /// Why the last search failed (for example an invalid pattern).
    pub error: Option<String>,
}

/// Longest query accepted in either overlay.
pub const MAX_QUERY_CHARS: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceAction {
    Boot,
    Stop,
}

/// A start/stop the app should perform for a device (queued by the reducer,
/// carried out by the event loop).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceCommand {
    pub action: DeviceAction,
    pub device: Device,
}

/// A device that was asked to start or stop and has not got there yet.
/// Booting takes a minute; without this the list would flip back to
/// "stopped" on the next poll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Transition {
    pub state: DeviceState,
    /// Polls seen since the request.
    pub polls: u8,
}

/// Polls after which a pending start/stop is given up on (4 s each).
pub const MAX_TRANSITION_POLLS: u8 = 45;

/// Assumed until the first resize event arrives.
pub const DEFAULT_TERMINAL_HEIGHT: u16 = 24;

#[derive(Debug, Clone, PartialEq)]
pub struct AppState {
    pub project: Project,
    pub config: Config,
    pub branch: Option<BranchInfo>,
    /// False until the first git check has finished, so "no git repo" is
    /// not shown while it is merely unknown.
    pub branch_known: bool,
    pub focus: Panel,
    pub status: Option<String>,
    pub should_quit: bool,
    pub doctor: DoctorPanel,
    /// First visible line of the doctor report.
    pub doctor_scroll: u16,
    pub terminal_height: u16,
    /// What `TerminalKind::Auto` resolved to at startup, if anything.
    pub detected_terminal: Option<TerminalKind>,
    /// Highlighted row of the terminal picker while it is open.
    pub terminal_picker: Option<usize>,
    /// Where the Neovim we launched listens, once it is open.
    pub editor_socket: Option<PathBuf>,
    /// A file the user chose before Neovim was running: opened as soon as it
    /// is (the launch was started for it).
    pub pending_open: Option<(PathBuf, Option<u32>)>,
    /// Neovim is being started, so a second request must not start another.
    pub editor_launching: bool,
    /// The event loop should start Neovim now.
    pub editor_launch_wanted: bool,
    /// True once a file was shown in that Neovim, i.e. its socket is known
    /// to work; opens can then give up quickly if it has gone.
    pub editor_ready: bool,
    /// Every project file (relative paths); empty until the first scan ends.
    pub files: FileList,
    pub files_loading: bool,
    /// Directories open in the file tree (relative paths).
    pub expanded: BTreeSet<PathBuf>,
    /// `files` ordered once, so a toggle only walks the visible rows.
    pub tree_index: Arc<TreeIndex>,
    /// The visible tree rows, derived from `tree_index` and `expanded`.
    pub tree: Arc<Vec<TreeRow>>,
    pub tree_selected: usize,
    /// First visible tree row.
    pub tree_top: usize,
    pub finder: Option<FinderState>,
    pub grep: Option<GrepState>,
    /// What the app builds and runs for.
    pub target: Platform,
    pub sdk: SdkInfo,
    pub sdk_loaded: bool,
    pub devices: Arc<Vec<Device>>,
    pub devices_loaded: bool,
    /// Why a platform could not be listed (tool missing, ...).
    pub device_errors: BTreeMap<Platform, String>,
    /// The chosen emulator/simulator per platform (AVD name / UDID).
    pub selected_devices: BTreeMap<Platform, String>,
    /// First visible row of the device list.
    pub device_top: usize,
    pub transitions: BTreeMap<(Platform, String), Transition>,
    /// Starts/stops waiting to be carried out.
    pub pending_commands: Vec<DeviceCommand>,
    /// The environment overlay (SDK paths and versions) is open.
    pub info_open: bool,
    pub output_tab: OutputTab,
    pub build: BuildState,
    pub build_log: Arc<LogBuffer>,
    pub logcat: Arc<LogBuffer>,
    pub ios_log: Arc<LogBuffer>,
    /// Errors and warnings found in the last build's output.
    pub problems: Arc<Vec<Problem>>,
    pub problem_selected: usize,
    pub output_filter: OutputFilter,
    /// Per tab (by [`OutputTab::index`]): lines scrolled back from the end;
    /// 0 follows new output.
    pub output_scroll: [usize; OutputTab::ALL.len()],
    /// Device logs being followed.
    pub log_streams: BTreeMap<Platform, LogStream>,
    /// Source of ids for builds and log streams.
    pub last_job_id: u64,
    /// Builds and log streams waiting to be started or stopped.
    pub pending_jobs: Vec<Job>,
    /// The latest performance sample, once one has arrived.
    pub perf: Option<PerfSample>,
    pub perf_open: bool,
    /// Highlighted row of the performance panel.
    pub perf_selected: usize,
    /// The process the highlight is on. The list is sorted by memory and
    /// reorders between samples; the highlight must stay with its process.
    pub perf_selected_pid: Option<u32>,
    /// The process the user has asked once to stop and must confirm.
    pub perf_confirm: Option<StopRequest>,
    pub swap_trend: Trend,
    /// A swap warning was already shown for this stretch of high swap.
    pub swap_alerted: bool,
    /// Files that differ from HEAD (project-relative), and HEAD's id.
    pub git_changes: Arc<Vec<FileChange>>,
    pub git_head: Option<String>,
    /// False until the first look at the repository has finished.
    pub git_loaded: bool,
    pub git_selected: usize,
    /// Show who last changed each line, at the end of the line in Neovim.
    pub blame_on: bool,
    /// The file Neovim is showing (project-relative), as far as we know.
    pub current_file: Option<std::path::PathBuf>,
    /// What the gutter in Neovim was last drawn from.
    pub marks_key: Option<MarksKey>,
    pub diff: Option<DiffView>,
    /// What a diff can be taken against, once listed.
    pub bases: Arc<Vec<BaseRef>>,
    /// The chosen base (HEAD until the user picks another).
    pub base: BaseRef,
}

/// A device log being followed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogStream {
    pub id: u64,
    pub device: String,
}

impl AppState {
    /// The devices of the current build target, in list order.
    pub fn target_devices(&self) -> Vec<&Device> {
        self.devices
            .iter()
            .filter(|d| d.platform == self.target)
            .collect()
    }

    /// The chosen device for the current target, if it is in the list.
    pub fn selected_device(&self) -> Option<&Device> {
        let id = self.selected_devices.get(&self.target)?;
        self.devices
            .iter()
            .find(|d| d.platform == self.target && &d.id == id)
    }

    /// The choices worth remembering between runs.
    pub fn prefs(&self) -> ProjectPrefs {
        ProjectPrefs {
            target: Some(self.target),
            android_device: self.selected_devices.get(&Platform::Android).cloned(),
            ios_device: self.selected_devices.get(&Platform::Ios).cloned(),
        }
    }
}

impl AppState {
    pub fn new(project: Project, config: Config) -> Self {
        let blame_on = config.show_blame;
        Self {
            project,
            config,
            branch: None,
            branch_known: false,
            focus: Panel::Files,
            status: None,
            should_quit: false,
            doctor: DoctorPanel::Closed,
            doctor_scroll: 0,
            terminal_height: DEFAULT_TERMINAL_HEIGHT,
            detected_terminal: None,
            terminal_picker: None,
            editor_socket: None,
            editor_ready: false,
            pending_open: None,
            editor_launching: false,
            editor_launch_wanted: false,
            files: Arc::new(Vec::new()),
            files_loading: true,
            expanded: BTreeSet::new(),
            tree_index: Arc::new(TreeIndex::default()),
            tree: Arc::new(Vec::new()),
            tree_selected: 0,
            tree_top: 0,
            finder: None,
            grep: None,
            target: Platform::Android,
            sdk: SdkInfo::default(),
            sdk_loaded: false,
            devices: Arc::new(Vec::new()),
            devices_loaded: false,
            device_errors: BTreeMap::new(),
            selected_devices: BTreeMap::new(),
            device_top: 0,
            transitions: BTreeMap::new(),
            pending_commands: Vec::new(),
            info_open: false,
            output_tab: OutputTab::Build,
            build: BuildState::default(),
            build_log: Arc::new(LogBuffer::default()),
            logcat: Arc::new(LogBuffer::default()),
            ios_log: Arc::new(LogBuffer::default()),
            problems: Arc::new(Vec::new()),
            problem_selected: 0,
            output_filter: OutputFilter::default(),
            output_scroll: [0; OutputTab::ALL.len()],
            log_streams: BTreeMap::new(),
            last_job_id: 0,
            pending_jobs: Vec::new(),
            perf: None,
            perf_open: false,
            perf_selected: 0,
            perf_selected_pid: None,
            perf_confirm: None,
            swap_trend: Trend::Flat,
            swap_alerted: false,
            git_changes: Arc::new(Vec::new()),
            git_head: None,
            git_loaded: false,
            git_selected: 0,
            blame_on,
            current_file: None,
            marks_key: None,
            diff: None,
            bases: Arc::new(Vec::new()),
            base: BaseRef::head(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_and_prev_wrap_around() {
        assert_eq!(Panel::Files.next(), Panel::Editor);
        assert_eq!(Panel::Bottom.next(), Panel::Files);
        assert_eq!(Panel::Files.prev(), Panel::Bottom);
        assert_eq!(Panel::Devices.prev(), Panel::Editor);
    }
}
