//! Core of LF-Mide: immutable app state, actions, the reducer, config and
//! project detection. No terminal or process I/O lives here.

pub mod action;
pub mod config;
pub mod project;
pub mod reducer;
pub mod state;

pub use action::Action;
pub use config::{Config, ConfigError, TerminalKind};
pub use project::{Project, ProjectError, ProjectLayout};
pub use reducer::reduce;
pub use state::{AppState, BranchInfo, Panel};
