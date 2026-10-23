//! Application state. Treated as immutable: the reducer returns new values.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;

use crate::config::{Config, TerminalKind};
use crate::devices::{Device, DeviceState, Platform, SdkInfo};
use crate::doctor::Report;
use crate::nav::{FileList, FileMatch, GrepHit, TreeRow};
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
    /// True once a file was shown in that Neovim, i.e. its socket is known
    /// to work; opens can then give up quickly if it has gone.
    pub editor_ready: bool,
    /// Every project file (relative paths); empty until the first scan ends.
    pub files: FileList,
    pub files_loading: bool,
    /// Directories open in the file tree (relative paths).
    pub expanded: BTreeSet<PathBuf>,
    /// The visible tree rows, derived from `files` and `expanded`.
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
            files: Arc::new(Vec::new()),
            files_loading: true,
            expanded: BTreeSet::new(),
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
