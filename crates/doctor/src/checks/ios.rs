//! Xcode, simulator runtimes and CocoaPods.

use lf_mide_core::{Category, CheckResult, Status};

use super::{CheckFn, Ctx, ToolSpec, tool};
use crate::system::RunError;

/// More installed runtimes than this is worth a nudge: each is several GB.
const MAX_COMFORTABLE_RUNTIMES: usize = 3;
const SELECT_XCODE_HINT: &str = "sudo xcode-select -s /Applications/Xcode.app/Contents/Developer";

pub const CHECKS: &[CheckFn] = &[xcode, first_launch, simulator_runtimes, cocoapods];

/// Versions of usable iOS runtimes from `xcrun simctl list runtimes`.
pub fn parse_ios_runtimes(output: &str) -> Vec<String> {
    output
        .lines()
        .filter(|line| line.starts_with("iOS ") && !line.contains("unavailable"))
        .filter_map(|line| line.split_whitespace().nth(1))
        .map(str::to_string)
        .collect()
}

fn xcode(ctx: &Ctx) -> Vec<CheckResult> {
    const NAME: &str = "Xcode";
    let missing = |detail: &str, hint: &str| {
        vec![CheckResult::new(Category::Ios, NAME, Status::Missing, detail).with_hint(hint)]
    };
    let selected = match ctx.sys.run("xcode-select", &["-p"]) {
        Ok(output) if output.success => output.stdout.trim().to_string(),
        _ => {
            return missing(
                "not installed",
                &format!("Install Xcode from the App Store, then: {SELECT_XCODE_HINT}"),
            );
        }
    };
    if selected.contains("CommandLineTools") {
        return missing("only Command Line Tools selected", SELECT_XCODE_HINT);
    }
    match ctx.sys.run("xcodebuild", &["-version"]) {
        Ok(output) if output.success => {
            let first_line = output.stdout.lines().next().unwrap_or("installed").trim();
            vec![CheckResult::new(Category::Ios, NAME, Status::Ok, first_line).with_path(selected)]
        }
        _ => missing("xcodebuild failed", "sudo xcodebuild -license accept"),
    }
}

fn first_launch(ctx: &Ctx) -> Vec<CheckResult> {
    const NAME: &str = "Xcode first-launch setup";
    let result = match ctx.sys.run("xcodebuild", &["-checkFirstLaunchStatus"]) {
        Ok(output) if output.success => {
            CheckResult::new(Category::Ios, NAME, Status::Ok, "complete")
        }
        Ok(_) => CheckResult::new(Category::Ios, NAME, Status::Warn, "pending")
            .with_hint("sudo xcodebuild -runFirstLaunch"),
        Err(RunError::TimedOut) => CheckResult::new(Category::Ios, NAME, Status::Warn, "timed out"),
        // No xcodebuild at all: the Xcode check already reports that.
        Err(_) => return Vec::new(),
    };
    vec![result]
}

fn simulator_runtimes(ctx: &Ctx) -> Vec<CheckResult> {
    const NAME: &str = "iOS simulator runtimes";
    let output = match ctx.sys.run("xcrun", &["simctl", "list", "runtimes"]) {
        Ok(output) if output.success => output,
        // A cold CoreSimulator service can be slow; that is not "missing".
        Err(RunError::TimedOut) => {
            return vec![
                CheckResult::new(
                    Category::Ios,
                    NAME,
                    Status::Warn,
                    "timed out (CoreSimulator is slow to start)",
                )
                .with_hint("Run `xcrun simctl list runtimes` once, then `lf-mide doctor` again"),
            ];
        }
        _ => {
            return vec![
                CheckResult::new(Category::Ios, NAME, Status::Missing, "simctl not available")
                    .with_hint(SELECT_XCODE_HINT),
            ];
        }
    };
    let versions = parse_ios_runtimes(&output.stdout);
    let list = versions.join(", ");
    let result = match versions.len() {
        0 => CheckResult::new(Category::Ios, NAME, Status::Missing, "none installed")
            .with_hint("xcodebuild -downloadPlatform iOS"),
        n if n <= MAX_COMFORTABLE_RUNTIMES => CheckResult::new(Category::Ios, NAME, Status::Ok, format!("{n}: {list}")),
        n => CheckResult::new(Category::Ios, NAME, Status::Warn, format!("{n} installed: {list}")).with_hint(
            "Each runtime takes several GB. List: xcrun simctl runtime list; remove: xcrun simctl runtime delete <id>",
        ),
    };
    vec![result]
}

fn cocoapods(ctx: &Ctx) -> Vec<CheckResult> {
    let Some(project) = ctx.project else {
        return Vec::new();
    };
    if !ctx.sys.exists(&project.root.join("iosApp/Podfile")) {
        return Vec::new();
    }
    let spec = ToolSpec {
        category: Category::Ios,
        name: "CocoaPods",
        binary: "pod",
        version_args: &["--version"],
        min: None,
        required: true,
        hint: "brew install cocoapods",
    };
    vec![tool(ctx, &spec, None)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeSystem;
    use lf_mide_core::{Project, ProjectLayout};
    use std::path::PathBuf;

    const DEV_DIR: &str = "/Applications/Xcode.app/Contents/Developer";
    const CLT_DIR: &str = "/Library/Developer/CommandLineTools";

    const RUNTIMES: &str = "== Runtimes ==
iOS 18.2 (18.2 - 22C150) - com.apple.CoreSimulator.SimRuntime.iOS-18-2
iOS 26.0 (26.0 - 23A343) - com.apple.CoreSimulator.SimRuntime.iOS-26-0
watchOS 26.0 (26.0 - 23R344) - com.apple.CoreSimulator.SimRuntime.watchOS-26-0
iOS 17.0 (17.0 - 21A328) - com.apple.CoreSimulator.SimRuntime.iOS-17-0 (unavailable, The iOS 17.0 simulator runtime is not supported)
";

    fn run(check: CheckFn, sys: &FakeSystem) -> Vec<CheckResult> {
        check(&Ctx { sys, project: None })
    }

    fn one(mut results: Vec<CheckResult>) -> CheckResult {
        assert_eq!(results.len(), 1, "{results:?}");
        results.remove(0)
    }

    fn xcode_installed() -> FakeSystem {
        FakeSystem::new()
            .ok("xcode-select -p", &format!("{DEV_DIR}\n"))
            .ok("xcodebuild -version", "Xcode 26.5\nBuild version 17F42\n")
    }

    #[test]
    fn xcode_ok_reports_version_and_developer_dir() {
        let r = one(run(xcode, &xcode_installed()));

        assert_eq!(
            (r.status, r.detail.as_str(), r.path.as_deref()),
            (Status::Ok, "Xcode 26.5", Some(DEV_DIR))
        );
    }

    #[test]
    fn command_line_tools_only_cannot_build_ios() {
        let sys = FakeSystem::new()
            .ok("xcode-select -p", &format!("{CLT_DIR}\n"))
            .exits_nonzero("xcodebuild -version");

        let r = one(run(xcode, &sys));

        assert_eq!(r.status, Status::Missing);
        assert!(r.detail.contains("Command Line Tools"), "{}", r.detail);
        assert_eq!(r.hint.as_deref(), Some(SELECT_XCODE_HINT));
    }

    #[test]
    fn xcode_missing_entirely() {
        let r = one(run(xcode, &FakeSystem::new()));

        assert_eq!(r.status, Status::Missing);
        assert!(r.hint.unwrap().contains("App Store"));
    }

    #[test]
    fn xcodebuild_failing_on_a_selected_xcode_points_to_the_license() {
        let sys = FakeSystem::new()
            .ok("xcode-select -p", &format!("{DEV_DIR}\n"))
            .exits_nonzero("xcodebuild -version");

        let r = one(run(xcode, &sys));

        assert_eq!(r.status, Status::Missing);
        assert_eq!(r.hint.as_deref(), Some("sudo xcodebuild -license accept"));
    }

    #[test]
    fn first_launch_complete_pending_and_skipped() {
        let done = FakeSystem::new().ok("xcodebuild -checkFirstLaunchStatus", "");
        let pending = FakeSystem::new().exits_nonzero("xcodebuild -checkFirstLaunchStatus");

        assert_eq!(one(run(first_launch, &done)).status, Status::Ok);
        let r = one(run(first_launch, &pending));
        assert_eq!(r.status, Status::Warn);
        assert_eq!(r.hint.as_deref(), Some("sudo xcodebuild -runFirstLaunch"));
        assert!(
            run(first_launch, &FakeSystem::new()).is_empty(),
            "no xcodebuild: the Xcode check covers it"
        );
    }

    #[test]
    fn parses_only_available_ios_runtimes() {
        assert_eq!(parse_ios_runtimes(RUNTIMES), vec!["18.2", "26.0"]);
        assert!(parse_ios_runtimes("== Runtimes ==\n").is_empty());
    }

    #[test]
    fn runtimes_ok_when_a_few_installed() {
        let r = one(run(
            simulator_runtimes,
            &FakeSystem::new().ok("xcrun simctl list runtimes", RUNTIMES),
        ));

        assert_eq!((r.status, r.detail.as_str()), (Status::Ok, "2: 18.2, 26.0"));
    }

    #[test]
    fn no_runtime_is_missing_with_download_hint() {
        let r = one(run(
            simulator_runtimes,
            &FakeSystem::new().ok("xcrun simctl list runtimes", "== Runtimes ==\n"),
        ));

        assert_eq!(r.status, Status::Missing);
        assert_eq!(r.hint.as_deref(), Some("xcodebuild -downloadPlatform iOS"));
    }

    #[test]
    fn many_runtimes_warn_about_disk_use() {
        let many: String = (18..=22)
            .map(|v| format!("iOS {v}.0 ({v}.0 - 1) - id\n"))
            .collect();

        let r = one(run(
            simulator_runtimes,
            &FakeSystem::new().ok("xcrun simctl list runtimes", &many),
        ));

        assert_eq!(r.status, Status::Warn);
        assert!(r.detail.starts_with("5 installed"), "{}", r.detail);
        assert!(r.hint.unwrap().contains("xcrun simctl runtime delete"));
    }

    #[test]
    fn a_slow_simctl_is_a_warning_not_a_missing_tool() {
        let r = one(run(
            simulator_runtimes,
            &FakeSystem::new().timeout("xcrun simctl list runtimes"),
        ));

        assert_eq!(r.status, Status::Warn);
        assert!(r.detail.contains("timed out"), "{}", r.detail);
    }

    #[test]
    fn simctl_unavailable_is_missing() {
        assert_eq!(
            one(run(simulator_runtimes, &FakeSystem::new())).status,
            Status::Missing
        );
    }

    #[test]
    fn cocoapods_only_checked_when_the_project_has_a_podfile() {
        let project = Project {
            root: PathBuf::from("/p"),
            name: "d".into(),
            layout: ProjectLayout::ComposeApp,
        };
        let with_podfile = FakeSystem::new()
            .file("/p/iosApp/Podfile", "")
            .env("PATH", "/bin");
        let without = FakeSystem::new().env("PATH", "/bin");

        let used = cocoapods(&Ctx {
            sys: &with_podfile,
            project: Some(&project),
        });
        let unused = cocoapods(&Ctx {
            sys: &without,
            project: Some(&project),
        });

        assert_eq!(one(used).status, Status::Missing);
        assert!(unused.is_empty());
        assert!(run(cocoapods, &FakeSystem::new()).is_empty());
    }
}
