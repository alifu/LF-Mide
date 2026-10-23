//! Core of LF-Mide: immutable app state, actions, the reducer, config and
//! project detection. No terminal or process I/O lives here.

pub mod action;
pub mod config;
pub mod doctor;
pub mod project;
pub mod reducer;
pub mod state;

pub use action::Action;
pub use config::{Config, ConfigError, TerminalKind, set_terminal_in_config};
pub use doctor::{
    Category, CheckResult, LineKind, Report, ReportLine, Status, popup_content_rows, popup_height,
};
pub use project::{Project, ProjectError, ProjectLayout};
pub use reducer::reduce;
pub use state::{AppState, BranchInfo, DoctorPanel, Panel};
