//! Housekeeping for a machine short on disk and memory: finding what can be
//! cleaned (`lf-mide clean`) and tuning Gradle's memory use
//! (`lf-mide tune`).

mod categories;
pub mod clean;
pub mod guard;
pub mod size;
pub mod tune;

pub use clean::{
    Action, Category, CleanEnv, Item, Outcome, Plan, Safety, execute, plan, roots_for, selection,
    total_text, unknown_ids,
};
