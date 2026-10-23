//! Terminal detection and split-pane launching.

pub mod detect;
pub mod launch;
pub mod quote;

pub use detect::{detect, resolve};
pub use launch::{
    Invocation, LaunchError, Launched, Plan, SplitDirection, SplitSpec, clipboard_invocation,
    copy_to_clipboard, launcher_script, open_split, plan_split,
};
