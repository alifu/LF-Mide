//! Dependency checks behind `lf-mide doctor`.

pub mod checks;
#[cfg(test)]
mod fake;
pub mod props;
pub mod report;
pub mod system;
pub mod version;

use checks::{Ctx, all, run_checks};
use lf_mide_core::{Project, Report};
use system::System;

/// Runs every check against `sys`. `project` enables the project-specific ones.
pub fn diagnose(sys: &dyn System, project: Option<&Project>) -> Report {
    run_checks(&Ctx { sys, project }, &all())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeSystem;

    #[test]
    fn diagnose_runs_every_check_group() {
        let report = diagnose(&FakeSystem::new(), None);

        let categories: std::collections::HashSet<_> =
            report.results.iter().map(|r| r.category).collect();
        assert!(categories.len() >= 5, "{categories:?}");
    }
}
