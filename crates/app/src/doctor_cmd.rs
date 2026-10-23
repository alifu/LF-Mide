//! `lf-mide doctor`: run the checks and print the report.

use std::io::IsTerminal;
use std::path::Path;

use color_eyre::Result;
use color_eyre::eyre::WrapErr;
use lf_mide_core::{Project, Report, project};
use lf_mide_doctor::report::{render_json, render_text};
use lf_mide_doctor::system::SystemRunner;

use crate::cli::DoctorArgs;

/// Text or JSON for `report`. Color is only for text.
pub fn format_report(report: &Report, json: bool, color: bool) -> String {
    if json {
        format!("{}\n", render_json(report))
    } else {
        render_text(report, color)
    }
}

/// The project containing `start`, if any. Doctor still works outside a
/// project; it just skips the project-specific checks.
fn detect_project(start: &Path) -> Option<Project> {
    project::detect(start).ok()
}

/// Prints the report and returns the process exit code.
pub fn run(args: DoctorArgs) -> Result<i32> {
    let start = match args.path {
        Some(path) => path,
        None => std::env::current_dir().wrap_err("cannot read current directory")?,
    };
    let start = start
        .canonicalize()
        .wrap_err_with(|| format!("cannot open {}", start.display()))?;
    let project = detect_project(&start);

    let report = lf_mide_doctor::diagnose(&SystemRunner::new(), project.as_ref());
    let color = !args.json && std::io::stdout().is_terminal();
    print!("{}", format_report(&report, args.json, color));
    Ok(report.exit_code())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lf_mide_core::{Category, CheckResult, Status};
    use tempfile::TempDir;

    fn report() -> Report {
        Report::new(vec![CheckResult::new(
            Category::Editor,
            "git",
            Status::Ok,
            "2.50.1",
        )])
    }

    #[test]
    fn json_flag_selects_json_without_color() {
        let out = format_report(&report(), true, true);

        assert!(out.trim_start().starts_with('{'));
        assert!(!out.contains('\x1b'));
    }

    #[test]
    fn text_mode_honours_color() {
        assert!(format_report(&report(), false, true).contains('\x1b'));
        assert!(!format_report(&report(), false, false).contains('\x1b'));
    }

    #[test]
    fn detects_a_project_and_tolerates_other_directories() {
        let project_dir = TempDir::new().unwrap();
        for dir in ["composeApp", "iosApp"] {
            std::fs::create_dir(project_dir.path().join(dir)).unwrap();
        }
        std::fs::write(project_dir.path().join("settings.gradle.kts"), "").unwrap();
        let elsewhere = TempDir::new().unwrap();

        assert!(detect_project(project_dir.path()).is_some());
        assert!(detect_project(elsewhere.path()).is_none());
    }
}
