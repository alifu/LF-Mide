//! The individual checks, grouped by category.

pub mod android;
pub mod editor;
pub mod ios;
pub mod macos;
pub mod project;
pub mod resources;
pub mod terminal;

use std::path::PathBuf;

use lf_mide_core::{Category, CheckResult, Project, Report, Status};

use crate::system::{RunError, System};
use crate::version::Version;

/// What every check can see.
pub struct Ctx<'a> {
    pub sys: &'a dyn System,
    pub project: Option<&'a Project>,
}

/// A check returns zero or more results (zero when it doesn't apply).
pub type CheckFn = fn(&Ctx) -> Vec<CheckResult>;

/// Description of a command-line tool to look up and version-check.
pub struct ToolSpec<'a> {
    pub category: Category,
    pub name: &'a str,
    /// Looked up on `$PATH` unless a path is given to [`tool`].
    pub binary: &'a str,
    /// Arguments that print the version; empty means "just check it exists".
    pub version_args: &'a [&'a str],
    pub min: Option<Version>,
    /// Absent required tools are `Missing`; optional ones are `Warn`.
    pub required: bool,
    pub hint: &'a str,
}

/// Checks one tool, preferring `at` over `$PATH` when it exists.
pub fn tool(ctx: &Ctx, spec: &ToolSpec, at: Option<PathBuf>) -> CheckResult {
    let absent = if spec.required {
        Status::Missing
    } else {
        Status::Warn
    };
    let result = |status, detail: &str| CheckResult::new(spec.category, spec.name, status, detail);
    let fix = |result: CheckResult| result.with_hint(spec.hint);

    let found = at
        .filter(|path| ctx.sys.exists(path))
        .or_else(|| ctx.sys.which(spec.binary));
    let Some(path) = found else {
        return fix(result(absent, "not found"));
    };
    let shown = path.display().to_string();
    let located = |status, detail: &str| result(status, detail).with_path(shown.clone());

    if spec.version_args.is_empty() {
        return located(Status::Ok, "found");
    }
    let output = match ctx.sys.run(&shown, spec.version_args) {
        Ok(output) if output.success => output,
        Ok(_) => return fix(located(absent, "exited with an error")),
        Err(RunError::TimedOut) => {
            return located(Status::Warn, "timed out while checking version");
        }
        Err(err) => return fix(located(absent, &format!("cannot run: {err}"))),
    };
    match (Version::find_in(&output.text()), spec.min) {
        (Some(found), Some(min)) if found < min => {
            fix(located(Status::Warn, &format!("{found} (need ≥ {min})")))
        }
        (Some(found), _) => located(Status::Ok, &found.to_string()),
        (None, _) => located(Status::Ok, "found"),
    }
}

/// Runs `checks` concurrently. A check that panics becomes a `Warn` result
/// instead of taking the whole report down.
pub fn run_checks(ctx: &Ctx, checks: &[CheckFn]) -> Report {
    let results = std::thread::scope(|scope| {
        let handles: Vec<_> = checks
            .iter()
            .map(|check| {
                std::thread::Builder::new()
                    .name(CHECK_THREAD_NAME.to_string())
                    .spawn_scoped(scope, move || check(ctx))
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|spawned| match spawned {
                Ok(handle) => handle.join().unwrap_or_else(|_| vec![crashed()]),
                Err(_) => vec![crashed()],
            })
            .collect()
    });
    Report::new(results)
}

fn crashed() -> CheckResult {
    CheckResult::new(
        Category::Resources,
        "Doctor",
        Status::Warn,
        "a check crashed (this is a bug)",
    )
    .with_hint("re-run with RUST_BACKTRACE=1 and report the output")
}

/// Name of the threads checks run on; the app uses it to treat a panic in
/// one check as contained (it becomes a `Warn`) instead of fatal.
pub const CHECK_THREAD_NAME: &str = "doctor-check";

/// Every check in the doctor, in no particular order (the report sorts).
pub fn all() -> Vec<CheckFn> {
    [
        android::CHECKS,
        ios::CHECKS,
        editor::CHECKS,
        terminal::CHECKS,
        macos::CHECKS,
        project::CHECKS,
        resources::CHECKS,
    ]
    .concat()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeSystem;

    fn spec<'a>(required: bool, min: Option<Version>) -> ToolSpec<'a> {
        ToolSpec {
            category: Category::Editor,
            name: "Neovim",
            binary: "nvim",
            version_args: &["--version"],
            min,
            required,
            hint: "brew install neovim",
        }
    }

    fn check(sys: &FakeSystem, spec: &ToolSpec, at: Option<PathBuf>) -> CheckResult {
        tool(&Ctx { sys, project: None }, spec, at)
    }

    fn with_nvim(version_line: &str) -> FakeSystem {
        FakeSystem::new()
            .env("PATH", "/opt/bin")
            .file("/opt/bin/nvim", "")
            .ok("/opt/bin/nvim --version", version_line)
    }

    #[test]
    fn found_tool_reports_version_and_path() {
        let r = check(&with_nvim("NVIM v0.12.6\nBuild"), &spec(true, None), None);

        assert_eq!(r.status, Status::Ok);
        assert_eq!(r.detail, "0.12.6");
        assert_eq!(r.path.as_deref(), Some("/opt/bin/nvim"));
        assert_eq!(r.hint, None);
    }

    #[test]
    fn missing_required_tool_is_missing_with_hint() {
        let r = check(
            &FakeSystem::new().env("PATH", "/opt/bin"),
            &spec(true, None),
            None,
        );

        assert_eq!(r.status, Status::Missing);
        assert_eq!(r.hint.as_deref(), Some("brew install neovim"));
    }

    #[test]
    fn missing_optional_tool_is_only_a_warning() {
        let r = check(
            &FakeSystem::new().env("PATH", "/opt/bin"),
            &spec(false, None),
            None,
        );

        assert_eq!(r.status, Status::Warn);
    }

    #[test]
    fn too_old_version_warns_with_requirement() {
        let r = check(
            &with_nvim("NVIM v0.9.5"),
            &spec(true, Some(Version::new(0, 11, 0))),
            None,
        );

        assert_eq!(r.status, Status::Warn);
        assert_eq!(r.detail, "0.9.5 (need ≥ 0.11.0)");
        assert!(r.hint.is_some());
    }

    #[test]
    fn explicit_path_wins_over_path_lookup() {
        let sys = with_nvim("NVIM v0.12.6")
            .file("/sdk/nvim", "")
            .ok("/sdk/nvim --version", "NVIM v0.11.0");

        let r = check(&sys, &spec(true, None), Some(PathBuf::from("/sdk/nvim")));

        assert_eq!(r.detail, "0.11.0");
        assert_eq!(r.path.as_deref(), Some("/sdk/nvim"));
    }

    #[test]
    fn explicit_path_that_does_not_exist_falls_back_to_path() {
        let r = check(
            &with_nvim("NVIM v0.12.6"),
            &spec(true, None),
            Some(PathBuf::from("/nope/nvim")),
        );

        assert_eq!(r.path.as_deref(), Some("/opt/bin/nvim"));
    }

    #[test]
    fn tool_that_fails_to_run_counts_as_missing() {
        // e.g. the macOS /usr/bin/java stub when no JDK is installed.
        let sys = FakeSystem::new()
            .env("PATH", "/opt/bin")
            .file("/opt/bin/nvim", "")
            .exits_nonzero("/opt/bin/nvim --version");

        let r = check(&sys, &spec(true, None), None);

        assert_eq!(r.status, Status::Missing);
        assert!(r.detail.contains("error"));
    }

    #[test]
    fn timeout_is_a_warning_not_a_failure() {
        let sys = FakeSystem::new()
            .env("PATH", "/opt/bin")
            .file("/opt/bin/nvim", "")
            .timeout("/opt/bin/nvim --version");

        let r = check(&sys, &spec(true, None), None);

        assert_eq!(r.status, Status::Warn);
        assert!(r.detail.contains("timed out"));
    }

    #[test]
    fn unexpected_run_errors_are_shown_readably() {
        let sys = FakeSystem::new()
            .env("PATH", "/opt/bin")
            .file("/opt/bin/nvim", "")
            .run_error(
                "/opt/bin/nvim --version",
                RunError::Other("permission denied".into()),
            );

        let r = check(&sys, &spec(true, None), None);

        assert_eq!(r.detail, "cannot run: permission denied");
    }

    #[test]
    fn checks_run_on_threads_named_for_the_panic_policy() {
        fn report_thread_name(_: &Ctx) -> Vec<CheckResult> {
            let name = std::thread::current().name().unwrap_or("").to_string();
            vec![CheckResult::new(
                Category::Editor,
                "thread",
                Status::Ok,
                name,
            )]
        }
        let sys = FakeSystem::new();
        let ctx = Ctx {
            sys: &sys,
            project: None,
        };

        let report = run_checks(&ctx, &[report_thread_name]);

        assert_eq!(report.results[0].detail, CHECK_THREAD_NAME);
    }

    #[test]
    fn empty_version_args_only_check_existence() {
        let sys = FakeSystem::new()
            .env("PATH", "/opt/bin")
            .file("/opt/bin/nvim", "");
        let spec = ToolSpec {
            version_args: &[],
            ..spec(true, None)
        };

        let r = check(&sys, &spec, None);

        assert_eq!(r.status, Status::Ok);
        assert_eq!(r.detail, "found");
    }

    #[test]
    fn output_without_version_still_counts_as_found() {
        let r = check(&with_nvim("something unversioned"), &spec(true, None), None);

        assert_eq!((r.status, r.detail.as_str()), (Status::Ok, "found"));
    }

    fn ok_check(_: &Ctx) -> Vec<CheckResult> {
        vec![CheckResult::new(Category::Editor, "fine", Status::Ok, "")]
    }
    fn none_check(_: &Ctx) -> Vec<CheckResult> {
        Vec::new()
    }
    fn panicking_check(_: &Ctx) -> Vec<CheckResult> {
        panic!("boom")
    }

    #[test]
    fn run_checks_collects_results_and_survives_panics() {
        let sys = FakeSystem::new();
        let ctx = Ctx {
            sys: &sys,
            project: None,
        };

        let report = run_checks(&ctx, &[ok_check, none_check, panicking_check]);

        assert_eq!(report.results.len(), 2);
        assert_eq!(report.count(Status::Ok), 1);
        assert_eq!(report.count(Status::Warn), 1);
        assert!(report.results.iter().any(|r| r.detail.contains("crashed")));
    }

    #[test]
    fn all_checks_run_on_an_empty_machine_without_panicking() {
        let sys = FakeSystem::new();
        let ctx = Ctx {
            sys: &sys,
            project: None,
        };

        let report = run_checks(&ctx, &all());

        assert!(report.results.len() >= 15, "got {}", report.results.len());
        assert!(report.results.iter().all(|r| !r.detail.contains("crashed")));
        assert!(
            report.exit_code() == 1,
            "an empty machine must fail the doctor"
        );
    }
}
