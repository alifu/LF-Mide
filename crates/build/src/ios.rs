//! iOS: `xcodebuild` for the chosen simulator, then install and launch.

use std::path::{Path, PathBuf};

use lf_mide_system::System;
use lf_mide_system::stream::StreamSpec;

use crate::{
    CancelFlag, Context, Report, Step, Streamer, failed, plain_identifier, run_step, stream_step,
};

const IOS_DIR: &str = "iosApp";
const CONFIGURATION: &str = "Debug";
const PRODUCTS_DIR: &str = "Build/Products/Debug-iphonesimulator";

/// The Xcode project or workspace to build.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct XcodeContainer {
    pub flag: &'static str,
    pub path: PathBuf,
    /// The same, relative to the project folder: what goes on a command
    /// line (which runs in the project folder).
    pub relative: PathBuf,
    /// File name without the extension.
    pub stem: String,
}

pub(crate) fn run(
    ctx: &Context<'_>,
    sys: &dyn System,
    streamer: &dyn Streamer,
    report: &mut dyn Report,
    cancel: &CancelFlag,
) -> Step {
    let udid = &ctx.device.id;
    if !lf_mide_platform::ios::valid_udid(udid) {
        return Err(failed(format!("{} has an unusual id", ctx.device.name)));
    }
    let container = find_container(ctx.root, sys).map_err(failed)?;
    let scheme = match ctx.variant {
        Some(scheme) if valid_scheme(scheme) => scheme.to_string(),
        Some(scheme) => return Err(failed(format!("unusual scheme name: {scheme}"))),
        None => pick_scheme(&container, ctx.root, sys, cancel, report)?,
    };
    let container_path = container.relative.display().to_string();
    let derived = ctx.derived_data.display().to_string();

    report.step("Building");
    let spec = StreamSpec {
        program: "xcrun".to_string(),
        args: [
            "xcodebuild",
            container.flag,
            &container_path,
            "-scheme",
            &scheme,
            "-configuration",
            CONFIGURATION,
            "-destination",
            &format!("id={udid}"),
            "-derivedDataPath",
            &derived,
            "build",
        ]
        .map(String::from)
        .to_vec(),
        cwd: Some(ctx.root.to_path_buf()),
        env: Vec::new(),
    };
    stream_step(streamer, report, cancel, &spec, "xcodebuild")?;

    let app = find_app(ctx.derived_data, sys)
        .ok_or_else(|| failed("the build produced no .app for the simulator"))?;
    let app_path = app.display().to_string();
    report.step("Installing");
    run_step(
        sys,
        cancel,
        report,
        None,
        "xcrun",
        &["simctl", "install", udid, &app_path],
        "Installing the app",
    )?;

    report.step("Launching");
    let info = bundle_info(&app, sys).map_err(failed)?;
    run_step(
        sys,
        cancel,
        report,
        None,
        "xcrun",
        &["simctl", "launch", udid, &info.identifier],
        "Starting the app",
    )?;
    report.launched(&info.identifier);
    Ok(())
}

/// `iosApp/*.xcworkspace` if there is one (CocoaPods), else the project.
pub(crate) fn find_container(root: &Path, sys: &dyn System) -> Result<XcodeContainer, String> {
    let dir = root.join(IOS_DIR);
    let names = sys.list_dir(&dir);
    let pick = |extension: &str, flag: &'static str| {
        names
            .iter()
            .find(|name| name.ends_with(extension))
            .map(|name| XcodeContainer {
                flag,
                path: dir.join(name),
                relative: Path::new(IOS_DIR).join(name),
                stem: name.trim_end_matches(extension).to_string(),
            })
    };
    pick(".xcworkspace", "-workspace")
        .or_else(|| pick(".xcodeproj", "-project"))
        .ok_or_else(|| format!("no Xcode project found in {IOS_DIR}/"))
}

/// The scheme named like the project, else the first one.
fn pick_scheme(
    container: &XcodeContainer,
    root: &Path,
    sys: &dyn System,
    cancel: &CancelFlag,
    report: &mut dyn Report,
) -> Result<String, crate::Stop> {
    let path = container.relative.display().to_string();
    let json = run_step(
        sys,
        cancel,
        report,
        Some(root),
        "xcrun",
        &["xcodebuild", container.flag, &path, "-list", "-json"],
        "Listing Xcode schemes",
    )?;
    schemes_from(&json, &container.stem).ok_or_else(|| failed("no usable Xcode scheme found"))
}

pub(crate) fn schemes_from(json: &str, stem: &str) -> Option<String> {
    let schemes = all_schemes(json);
    schemes
        .iter()
        .find(|scheme| scheme.as_str() == stem)
        .or_else(|| schemes.first())
        .cloned()
}

/// Every usable scheme in `xcodebuild -list -json` output.
fn all_schemes(json: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    ["workspace", "project"]
        .iter()
        .filter_map(|key| value.get(key)?.get("schemes")?.as_array())
        .flatten()
        .filter_map(|scheme| scheme.as_str())
        .filter(|scheme| valid_scheme(scheme))
        .map(String::from)
        .collect()
}

/// A scheme name that is safe as a command-line value.
fn valid_scheme(scheme: &str) -> bool {
    !scheme.is_empty() && !scheme.starts_with('-') && !scheme.chars().any(char::is_control)
}

/// The schemes of the Xcode project, the default first.
pub(crate) fn scheme_names(root: &Path, sys: &dyn System) -> Result<Vec<String>, String> {
    let container = find_container(root, sys)?;
    let relative = container.relative.display().to_string();
    let listing = sys
        .run_in(
            root,
            "xcrun",
            &["xcodebuild", container.flag, &relative, "-list", "-json"],
        )
        .map_err(|error| format!("xcodebuild: {error}"))?;
    if !listing.success {
        return Err("xcodebuild could not list the Xcode schemes".to_string());
    }
    let mut schemes = all_schemes(&listing.stdout);
    if let Some(at) = schemes.iter().position(|scheme| *scheme == container.stem) {
        schemes.swap(0, at);
    }
    Ok(schemes)
}

/// The built `.app` for the simulator.
pub(crate) fn find_app(derived_data: &Path, sys: &dyn System) -> Option<PathBuf> {
    let products = derived_data.join(PRODUCTS_DIR);
    sys.list_dir(&products)
        .into_iter()
        .find(|name| name.ends_with(".app"))
        .map(|name| products.join(name))
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct BundleInfo {
    pub identifier: String,
    pub executable: String,
}

/// Bundle id and executable name from the app's `Info.plist`.
pub(crate) fn bundle_info(app: &Path, sys: &dyn System) -> Result<BundleInfo, String> {
    let plist = app.join("Info.plist").display().to_string();
    let read = |key: &str| {
        let output = sys
            .run("plutil", &["-extract", key, "raw", "-o", "-", &plist])
            .map_err(|error| format!("plutil: {error}"))?;
        let value = output.stdout.trim().to_string();
        if output.success && !value.is_empty() {
            Ok(value)
        } else {
            Err(format!("{key} is missing from the app's Info.plist"))
        }
    };
    let identifier = read("CFBundleIdentifier")?;
    let executable = read("CFBundleExecutable")?;
    if !plain_identifier(&identifier) {
        return Err("the app's bundle identifier looks unusual".to_string());
    }
    if !valid_executable_name(&executable) {
        return Err("the app's executable name looks unusual".to_string());
    }
    Ok(BundleInfo {
        identifier,
        executable,
    })
}

/// Executable names may contain spaces; nothing that could end a predicate
/// string or start an option.
fn valid_executable_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 100
        && !name.starts_with(['-', '.'])
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ' '))
}

#[cfg(test)]
mod tests {
    use lf_mide_core::{BuildOutcome, Device, DeviceState, Platform, ProjectLayout};
    use lf_mide_system::fake::FakeSystem;

    use super::*;
    use crate::build_and_run;
    use crate::testing::{Recorder, ScriptedStreamer};

    const UDID: &str = "23F0A11F-0DD8-4766-853D-987DE9852BAC";
    const APP: &str = "/cache/Build/Products/Debug-iphonesimulator/iosApp.app";
    const LIST: &str = "xcrun xcodebuild -project iosApp/iosApp.xcodeproj -list -json";
    const SCHEMES: &str =
        r#"{"project":{"name":"iosApp","schemes":["Other","iosApp"],"targets":["iosApp"]}}"#;

    fn simulator(id: &str) -> Device {
        Device {
            platform: Platform::Ios,
            id: id.into(),
            name: "iPhone 17 Pro".into(),
            detail: String::new(),
            state: DeviceState::Running,
            serial: None,
        }
    }

    fn ctx<'a>(device: &'a Device) -> Context<'a> {
        Context {
            root: Path::new("/proj"),
            layout: ProjectLayout::AndroidAppShared,
            derived_data: Path::new("/cache"),
            device,
            variant: None,
        }
    }

    fn project() -> FakeSystem {
        FakeSystem::new()
            .dir(
                "/proj/iosApp",
                &["Configuration", "iosApp", "iosApp.xcodeproj"],
            )
            .ok(LIST, SCHEMES)
            .dir(
                "/cache/Build/Products/Debug-iphonesimulator",
                &["iosApp.app", "iosApp.app.dSYM"],
            )
            .ok(&format!("xcrun simctl install {UDID} {APP}"), "")
            .ok(
                &format!("plutil -extract CFBundleIdentifier raw -o - {APP}/Info.plist"),
                "com.example.app\n",
            )
            .ok(
                &format!("plutil -extract CFBundleExecutable raw -o - {APP}/Info.plist"),
                "iosApp\n",
            )
            .ok(
                &format!("xcrun simctl launch {UDID} com.example.app"),
                "com.example.app: 4242\n",
            )
    }

    #[test]
    fn builds_for_the_simulator_installs_and_launches() {
        let streamer =
            ScriptedStreamer::new(vec![ScriptedStreamer::ok(vec!["** BUILD SUCCEEDED **"])]);
        let mut report = Recorder::default();
        let device = simulator(UDID);
        let sys = project();

        let outcome = build_and_run(
            &ctx(&device),
            &sys,
            &streamer,
            &mut report,
            &CancelFlag::default(),
        );
        let sys_ran_in = sys.ran_in();
        assert_eq!(
            sys_ran_in.first().map(|(dir, _)| dir.as_path()),
            Some(Path::new("/proj")),
            "the scheme list runs inside the project folder"
        );

        assert_eq!(outcome, BuildOutcome::Succeeded);
        assert_eq!(report.steps, ["Building", "Installing", "Launching"]);
        let specs = streamer.specs.borrow();
        assert_eq!(specs[0].program, "xcrun");
        assert_eq!(
            specs[0].args,
            [
                "xcodebuild",
                "-project",
                "iosApp/iosApp.xcodeproj",
                "-scheme",
                "iosApp",
                "-configuration",
                "Debug",
                "-destination",
                &format!("id={UDID}"),
                "-derivedDataPath",
                "/cache",
                "build"
            ]
        );
        assert_eq!(specs[0].cwd.as_deref(), Some(Path::new("/proj")));
    }

    #[test]
    fn a_failed_xcodebuild_stops_the_pipeline() {
        let streamer = ScriptedStreamer::new(vec![ScriptedStreamer::exits(
            65,
            vec!["** BUILD FAILED **"],
        )]);
        let mut report = Recorder::default();
        let device = simulator(UDID);

        let outcome = build_and_run(
            &ctx(&device),
            &project(),
            &streamer,
            &mut report,
            &CancelFlag::default(),
        );

        assert_eq!(
            outcome,
            BuildOutcome::Failed(Some("xcodebuild exited with code 65".into()))
        );
        assert_eq!(report.steps, ["Building"]);
    }

    #[test]
    fn a_build_without_an_app_is_explained() {
        let sys = project().dir("/cache/Build/Products/Debug-iphonesimulator", &["other"]);
        let streamer = ScriptedStreamer::new(vec![ScriptedStreamer::ok(vec![])]);
        let device = simulator(UDID);

        let outcome = build_and_run(
            &ctx(&device),
            &sys,
            &streamer,
            &mut Recorder::default(),
            &CancelFlag::default(),
        );

        assert!(matches!(outcome, BuildOutcome::Failed(Some(m)) if m.contains("no .app")));
    }

    #[test]
    fn an_unusual_simulator_id_never_reaches_a_command() {
        let streamer = ScriptedStreamer::new(vec![]);
        let device = simulator("-x; rm -rf ~");

        let outcome = build_and_run(
            &ctx(&device),
            &project(),
            &streamer,
            &mut Recorder::default(),
            &CancelFlag::default(),
        );

        assert!(matches!(outcome, BuildOutcome::Failed(Some(m)) if m.contains("unusual id")));
        assert!(streamer.specs.borrow().is_empty());
    }

    #[test]
    fn a_workspace_is_preferred_over_the_project() {
        let sys =
            FakeSystem::new().dir("/proj/iosApp", &["iosApp.xcodeproj", "iosApp.xcworkspace"]);

        let found = find_container(Path::new("/proj"), &sys).unwrap();

        assert_eq!(found.flag, "-workspace");
        assert_eq!(found.path, PathBuf::from("/proj/iosApp/iosApp.xcworkspace"));
        assert_eq!(found.stem, "iosApp");
        assert!(find_container(Path::new("/proj"), &FakeSystem::new()).is_err());
    }

    #[test]
    fn the_scheme_named_like_the_project_wins_else_the_first() {
        assert_eq!(schemes_from(SCHEMES, "iosApp").as_deref(), Some("iosApp"));
        assert_eq!(schemes_from(SCHEMES, "nope").as_deref(), Some("Other"));
        assert_eq!(
            schemes_from(r#"{"workspace":{"schemes":["W"]}}"#, "x").as_deref(),
            Some("W")
        );
        assert_eq!(
            schemes_from(r#"{"project":{"schemes":["-bad"]}}"#, "x"),
            None
        );
        assert_eq!(schemes_from("not json", "x"), None);
        assert_eq!(schemes_from(r#"{"project":{"schemes":[]}}"#, "x"), None);
    }

    #[test]
    fn bundle_info_rejects_unusual_values() {
        for (id, exe) in [
            ("com.a;b", "App"),
            ("com.a.b", "App\"; drop"),
            ("com.a.b", "-App"),
        ] {
            let sys = FakeSystem::new()
                .ok(
                    &format!("plutil -extract CFBundleIdentifier raw -o - {APP}/Info.plist"),
                    id,
                )
                .ok(
                    &format!("plutil -extract CFBundleExecutable raw -o - {APP}/Info.plist"),
                    exe,
                );

            assert!(bundle_info(Path::new(APP), &sys).is_err(), "{id} {exe}");
        }
        let spaced = FakeSystem::new()
            .ok(
                &format!("plutil -extract CFBundleIdentifier raw -o - {APP}/Info.plist"),
                "com.a.b",
            )
            .ok(
                &format!("plutil -extract CFBundleExecutable raw -o - {APP}/Info.plist"),
                "My App",
            );
        assert_eq!(
            bundle_info(Path::new(APP), &spaced).unwrap().executable,
            "My App"
        );
    }

    #[test]
    fn the_chosen_scheme_is_built_without_listing_the_schemes() {
        let streamer =
            ScriptedStreamer::new(vec![ScriptedStreamer::ok(vec!["** BUILD SUCCEEDED **"])]);
        let device = simulator(UDID);
        let sys = project();
        let context = Context {
            variant: Some("iosApp Staging"),
            ..ctx(&device)
        };

        let _ = build_and_run(
            &context,
            &sys,
            &streamer,
            &mut Recorder::default(),
            &CancelFlag::default(),
        );

        let specs = streamer.specs.borrow();
        let scheme_at = specs[0].args.iter().position(|a| a == "-scheme").unwrap();
        assert_eq!(specs[0].args[scheme_at + 1], "iosApp Staging");
        assert!(sys.ran_in().is_empty(), "no `-list` was needed");
    }

    #[test]
    fn a_scheme_that_could_pass_for_an_option_is_refused() {
        let streamer = ScriptedStreamer::new(vec![]);
        let device = simulator(UDID);
        let context = Context {
            variant: Some("-destination"),
            ..ctx(&device)
        };

        let outcome = build_and_run(
            &context,
            &project(),
            &streamer,
            &mut Recorder::default(),
            &CancelFlag::default(),
        );

        assert!(matches!(outcome, BuildOutcome::Failed(Some(m)) if m.contains("scheme")));
        assert!(streamer.specs.borrow().is_empty());
    }

    #[test]
    fn schemes_are_listed_with_the_one_named_like_the_project_first() {
        let sys = project().ok(LIST, r#"{"project":{"schemes":["Other","-bad","iosApp"]}}"#);

        assert_eq!(
            scheme_names(Path::new("/proj"), &sys).unwrap(),
            ["iosApp", "Other"]
        );
    }
}
