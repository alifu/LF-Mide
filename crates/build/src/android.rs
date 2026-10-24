//! Android: `gradlew :<module>:installDebug` on the chosen emulator, then
//! start the app's launcher activity.

use std::path::{Path, PathBuf};

use lf_mide_core::ProjectLayout;
use lf_mide_platform::android::adb_path;
use lf_mide_system::System;
use lf_mide_system::stream::StreamSpec;

use crate::{
    CancelFlag, Context, Report, Step, Streamer, failed, plain_identifier, run_step, stream_step,
};

const GRADLE_WRAPPER: &str = "gradlew";
const GRADLE_FILES: [&str; 2] = ["build.gradle.kts", "build.gradle"];
/// Reads the target device (`ANDROID_SERIAL`) when installing.
const SERIAL_ENV: &str = "ANDROID_SERIAL";

/// What the build file says about the Android app.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct AndroidApp {
    pub module: &'static str,
    /// Package names the installed debug app may have, most likely first:
    /// the id with its suffixes, then without them.
    pub package_candidates: Vec<String>,
}

pub(crate) fn run(
    ctx: &Context<'_>,
    sys: &dyn System,
    streamer: &dyn Streamer,
    report: &mut dyn Report,
    cancel: &CancelFlag,
) -> Step {
    let serial = ctx
        .device
        .serial
        .as_deref()
        .filter(|serial| plain_identifier(serial))
        .ok_or_else(|| failed(format!("{} has no adb serial", ctx.device.name)))?;
    let app = app_info(ctx.root, ctx.layout, sys).map_err(failed)?;
    // Checked to be inside the project; then run by its name relative to the
    // project folder (the working directory), so the project's own path is
    // never turned into text.
    gradle_wrapper(ctx.root, sys).map_err(failed)?;

    report.step("Building");
    let spec = StreamSpec {
        program: format!("./{GRADLE_WRAPPER}"),
        args: vec![
            "--console=plain".to_string(),
            format!(":{}:installDebug", app.module),
        ],
        cwd: Some(ctx.root.to_path_buf()),
        env: vec![(SERIAL_ENV.to_string(), serial.to_string())],
    };
    stream_step(streamer, report, cancel, &spec, "Gradle")?;

    report.step("Launching");
    let adb = adb_path(sys).ok_or_else(|| failed("adb was not found"))?;
    let package = installed_package(sys, &adb, serial, &app.package_candidates)
        .ok_or_else(|| failed("the installed app was not found on the device"))?;
    run_step(
        sys,
        cancel,
        report,
        None,
        &adb.display().to_string(),
        &[
            "-s",
            serial,
            "shell",
            "monkey",
            "-p",
            &package,
            "-c",
            "android.intent.category.LAUNCHER",
            "1",
        ],
        "Starting the app",
    )?;
    Ok(())
}

/// The module and application id of the Android app.
pub(crate) fn app_info(
    root: &Path,
    layout: ProjectLayout,
    sys: &dyn System,
) -> Result<AndroidApp, String> {
    let module = match layout {
        ProjectLayout::ComposeApp => "composeApp",
        ProjectLayout::AndroidAppShared => "androidApp",
    };
    let build_file = GRADLE_FILES
        .iter()
        .map(|name| root.join(module).join(name))
        .find(|path| sys.exists(path))
        .ok_or_else(|| format!("{module}/build.gradle.kts was not found"))?;
    let text = sys
        .read_to_string(&build_file)
        .ok_or_else(|| format!("could not read {}", build_file.display()))?;
    let application_id = quoted_value(&text, "applicationId")
        .or_else(|| quoted_value(&text, "namespace"))
        .filter(|id| valid_application_id(id))
        .ok_or_else(|| {
            format!(
                "no valid applicationId in {module}/{}",
                file_name(&build_file)
            )
        })?;
    Ok(AndroidApp {
        module,
        package_candidates: package_candidates(&application_id, &text),
    })
}

/// The id of the debug variant (`installDebug`): the base id plus the
/// `applicationIdSuffix` of `defaultConfig` and of the `debug` build type.
/// The other spellings follow in case the build file is more unusual than
/// this reads; the device decides which one is really installed.
fn package_candidates(base: &str, build_file: &str) -> Vec<String> {
    let suffix_in = |block: &str| {
        block_after(build_file, block)
            .and_then(|text| quoted_value(text, "applicationIdSuffix"))
            .unwrap_or_default()
    };
    let default_suffix = suffix_in("defaultConfig");
    let debug_suffix = suffix_in("debug");
    let mut candidates = Vec::new();
    for id in [
        format!("{base}{default_suffix}{debug_suffix}"),
        format!("{base}{default_suffix}"),
        base.to_string(),
        format!("{base}.debug"),
    ] {
        if valid_application_id(&id) && !candidates.contains(&id) {
            candidates.push(id);
        }
    }
    candidates
}

/// The first of `candidates` that is installed on the device.
fn installed_package(
    sys: &dyn System,
    adb: &Path,
    serial: &str,
    candidates: &[String],
) -> Option<String> {
    let adb = adb.display().to_string();
    candidates
        .iter()
        .find(|id| {
            sys.run(&adb, &["-s", serial, "shell", "pm", "path", id])
                .is_ok_and(|out| out.success && out.stdout.trim_start().starts_with("package:"))
        })
        .cloned()
}

/// The text between the braces of the first `name { ... }` block (also
/// `name("...") {`, `getByName("name") {`).
fn block_after<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let mut from = 0;
    while let Some(found) = text[from..].find(name) {
        let start = from + found + name.len();
        from = start;
        let rest = text[start..].trim_start_matches(['"', '\'', ')', '(', ' ', '\t']);
        let Some(inner) = rest.strip_prefix('{') else {
            continue;
        };
        let mut depth = 1usize;
        for (index, ch) in inner.char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(&inner[..index]);
                    }
                }
                _ => {}
            }
        }
        return Some(inner);
    }
    None
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The `gradlew` script inside the project, never one that leads outside.
fn gradle_wrapper(root: &Path, sys: &dyn System) -> Result<PathBuf, String> {
    let wrapper = root.join(GRADLE_WRAPPER);
    let resolved = sys
        .canonicalize(&wrapper)
        .ok_or_else(|| "gradlew was not found in the project".to_string())?;
    let inside = sys
        .canonicalize(root)
        .is_some_and(|root| resolved.starts_with(root));
    if inside {
        Ok(wrapper)
    } else {
        Err("gradlew points outside the project".to_string())
    }
}

/// The first quoted string after `key =` / `key "..."` on a line, ignoring
/// comments.
fn quoted_value(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let line = line.split("//").next().unwrap_or("");
        let at = line.find(key)?;
        let rest = &line[at + key.len()..];
        if !rest.starts_with(|c: char| c.is_whitespace() || c == '=') {
            return None;
        }
        let start = rest.find('"')? + 1;
        let len = rest[start..].find('"')?;
        Some(rest[start..start + len].to_string())
    })
}

/// `com.example.app`: dot-separated identifiers.
fn valid_application_id(id: &str) -> bool {
    id.contains('.')
        && id.split('.').all(|part| {
            part.starts_with(|c: char| c.is_ascii_alphabetic())
                && part.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
}

#[cfg(test)]
mod tests {
    use lf_mide_core::{BuildOutcome, Device, DeviceState, Platform};
    use lf_mide_system::fake::FakeSystem;

    use super::*;
    use crate::build_and_run;
    use crate::testing::{Recorder, ScriptedStreamer};

    const BUILD_FILE: &str = r#"
android {
    namespace = "com.example.lf_pos_mobile"
    defaultConfig {
        applicationId = "com.example.lf_pos_mobile" // the app
        minSdk = 24
    }
}
"#;

    fn emulator(serial: Option<&str>) -> Device {
        Device {
            platform: Platform::Android,
            id: "Pixel_8".into(),
            name: "Pixel_8".into(),
            detail: String::new(),
            state: DeviceState::Running,
            serial: serial.map(String::from),
        }
    }

    fn project() -> FakeSystem {
        FakeSystem::new()
            .ok(PM_PATH, "package:/data/app/~~x/base.apk\n")
            .env("ANDROID_HOME", "/sdk")
            .dir("/sdk", &[])
            .file("/sdk/platform-tools/adb", "")
            .file("/proj/gradlew", "#!/bin/sh")
            .dir("/proj", &["gradlew"])
            .file("/proj/androidApp/build.gradle.kts", BUILD_FILE)
    }

    fn ctx<'a>(device: &'a Device) -> Context<'a> {
        Context {
            root: Path::new("/proj"),
            layout: ProjectLayout::AndroidAppShared,
            derived_data: Path::new("/cache"),
            device,
        }
    }

    const PM_PATH: &str =
        "/sdk/platform-tools/adb -s emulator-5554 shell pm path com.example.lf_pos_mobile";
    const LAUNCH: &str = "/sdk/platform-tools/adb -s emulator-5554 shell monkey -p com.example.lf_pos_mobile -c android.intent.category.LAUNCHER 1";

    #[test]
    fn builds_installs_on_the_chosen_emulator_then_launches() {
        let sys = project().ok(LAUNCH, "Events injected: 1\n");
        let streamer = ScriptedStreamer::new(vec![ScriptedStreamer::ok(vec![
            "> Task :androidApp:installDebug",
            "BUILD SUCCESSFUL in 9s",
        ])]);
        let mut report = Recorder::default();
        let device = emulator(Some("emulator-5554"));

        let outcome = build_and_run(
            &ctx(&device),
            &sys,
            &streamer,
            &mut report,
            &CancelFlag::default(),
        );

        assert_eq!(outcome, BuildOutcome::Succeeded);
        assert_eq!(report.steps, ["Building", "Launching"]);
        let specs = streamer.specs.borrow();
        assert_eq!(specs[0].program, "./gradlew");
        assert_eq!(
            specs[0].args,
            ["--console=plain", ":androidApp:installDebug"]
        );
        assert_eq!(specs[0].cwd.as_deref(), Some(Path::new("/proj")));
        assert_eq!(
            specs[0].env,
            [("ANDROID_SERIAL".to_string(), "emulator-5554".to_string())]
        );
        assert!(report.log.iter().any(|l| l.contains("BUILD SUCCESSFUL")));
        assert!(report.log.iter().any(|l| l.starts_with("$ ./gradlew")));
    }

    #[test]
    fn a_failing_gradle_build_stops_before_launching() {
        let sys = project();
        let streamer = ScriptedStreamer::new(vec![ScriptedStreamer::exits(
            1,
            vec!["e: file:///proj/a.kt:1:1 bad"],
        )]);
        let mut report = Recorder::default();
        let device = emulator(Some("emulator-5554"));

        let outcome = build_and_run(
            &ctx(&device),
            &sys,
            &streamer,
            &mut report,
            &CancelFlag::default(),
        );

        assert_eq!(
            outcome,
            BuildOutcome::Failed(Some("Gradle exited with code 1".into()))
        );
        assert_eq!(report.steps, ["Building"]);
    }

    #[test]
    fn a_cancelled_build_reports_cancelled_not_failed() {
        let sys = project();
        let streamer = ScriptedStreamer::new(vec![ScriptedStreamer::exits(143, vec![])]);
        let cancel = CancelFlag::default();
        cancel.cancel();
        let device = emulator(Some("emulator-5554"));

        let outcome = build_and_run(
            &ctx(&device),
            &sys,
            &streamer,
            &mut Recorder::default(),
            &cancel,
        );

        assert_eq!(outcome, BuildOutcome::Cancelled);
    }

    const SUFFIXED: &str = r#"
android {
    defaultConfig {
        applicationId = "com.example.app"
        applicationIdSuffix = ".beta"
    }
    buildTypes {
        getByName("release") { applicationIdSuffix = ".rel" }
        debug {
            applicationIdSuffix = ".debug"
        }
    }
}
"#;

    #[test]
    fn the_debug_variants_package_is_the_base_id_with_its_suffixes() {
        let candidates = package_candidates("com.example.app", SUFFIXED);

        assert_eq!(
            candidates,
            [
                "com.example.app.beta.debug",
                "com.example.app.beta",
                "com.example.app",
                "com.example.app.debug"
            ]
        );
        assert_eq!(
            package_candidates("com.a.b", "android { }"),
            ["com.a.b", "com.a.b.debug"]
        );
    }

    #[test]
    fn a_suffix_in_another_build_type_is_not_used() {
        let text = "buildTypes { release { applicationIdSuffix = \".rel\" } }";

        assert_eq!(package_candidates("com.a.b", text)[0], "com.a.b");
    }

    #[test]
    fn the_package_that_is_really_installed_is_the_one_launched() {
        let sys = FakeSystem::new()
            .env("ANDROID_HOME", "/sdk")
            .dir("/sdk", &[])
            .file("/sdk/platform-tools/adb", "")
            .exits_nonzero(
                "/sdk/platform-tools/adb -s emulator-5554 shell pm path com.a.b.beta.debug",
            )
            .ok(
                "/sdk/platform-tools/adb -s emulator-5554 shell pm path com.a.b.beta",
                "",
            )
            .ok(
                "/sdk/platform-tools/adb -s emulator-5554 shell pm path com.a.b",
                "package:/data/app/b.apk\n",
            );
        let candidates = package_candidates(
            "com.a.b",
            "defaultConfig { applicationIdSuffix = \".beta\" }\ndebug { applicationIdSuffix = \".debug\" }",
        );

        assert_eq!(
            installed_package(
                &sys,
                Path::new("/sdk/platform-tools/adb"),
                "emulator-5554",
                &candidates
            )
            .as_deref(),
            Some("com.a.b")
        );
        assert_eq!(
            installed_package(
                &sys,
                Path::new("/sdk/platform-tools/adb"),
                "emulator-5554",
                &["com.nope.x".to_string()]
            ),
            None
        );
    }

    #[test]
    fn a_launch_with_a_suffixed_package_uses_the_installed_one() {
        let sys = project()
            .file("/proj/androidApp/build.gradle.kts", SUFFIXED)
            .exits_nonzero("/sdk/platform-tools/adb -s emulator-5554 shell pm path com.example.app.beta.debug")
            .ok("/sdk/platform-tools/adb -s emulator-5554 shell pm path com.example.app.beta", "package:/x.apk\n")
            .ok("/sdk/platform-tools/adb -s emulator-5554 shell monkey -p com.example.app.beta -c android.intent.category.LAUNCHER 1", "ok\n");
        let streamer = ScriptedStreamer::new(vec![ScriptedStreamer::ok(vec![])]);
        let device = emulator(Some("emulator-5554"));

        let outcome = build_and_run(
            &ctx(&device),
            &sys,
            &streamer,
            &mut Recorder::default(),
            &CancelFlag::default(),
        );

        assert_eq!(outcome, BuildOutcome::Succeeded);
    }

    #[test]
    fn nothing_installed_under_any_candidate_name_is_reported() {
        let sys = project()
            .exits_nonzero(PM_PATH)
            .exits_nonzero("/sdk/platform-tools/adb -s emulator-5554 shell pm path com.example.lf_pos_mobile.debug");
        let streamer = ScriptedStreamer::new(vec![ScriptedStreamer::ok(vec![])]);
        let device = emulator(Some("emulator-5554"));

        let outcome = build_and_run(
            &ctx(&device),
            &sys,
            &streamer,
            &mut Recorder::default(),
            &CancelFlag::default(),
        );

        assert!(
            matches!(outcome, BuildOutcome::Failed(Some(m)) if m.contains("not found on the device"))
        );
    }

    #[test]
    fn a_cancel_during_the_build_stops_before_the_launch_command() {
        let sys = project().ok(LAUNCH, "Events injected: 1\n");
        let cancel = CancelFlag::default();
        cancel.cancel();
        let streamer = ScriptedStreamer::new(vec![ScriptedStreamer::ok(vec![])]);
        let mut report = Recorder::default();
        let device = emulator(Some("emulator-5554"));

        let outcome = build_and_run(&ctx(&device), &sys, &streamer, &mut report, &cancel);

        assert_eq!(outcome, BuildOutcome::Cancelled);
        assert!(!report.log.iter().any(|l| l.contains("monkey")));
    }

    #[test]
    fn a_failed_launch_is_reported() {
        let sys = project().fails(LAUNCH, "Error: no activity");
        let streamer = ScriptedStreamer::new(vec![ScriptedStreamer::ok(vec![])]);
        let device = emulator(Some("emulator-5554"));

        let outcome = build_and_run(
            &ctx(&device),
            &sys,
            &streamer,
            &mut Recorder::default(),
            &CancelFlag::default(),
        );

        assert_eq!(
            outcome,
            BuildOutcome::Failed(Some("Starting the app failed".into()))
        );
    }

    #[test]
    fn refuses_a_device_without_a_safe_serial() {
        for serial in [None, Some("-e"), Some("emulator-5554; rm -rf ~")] {
            let sys = project();
            let streamer = ScriptedStreamer::new(vec![]);
            let device = emulator(serial);

            let outcome = build_and_run(
                &ctx(&device),
                &sys,
                &streamer,
                &mut Recorder::default(),
                &CancelFlag::default(),
            );

            assert!(
                matches!(outcome, BuildOutcome::Failed(Some(m)) if m.contains("serial")),
                "{serial:?}"
            );
            assert!(streamer.specs.borrow().is_empty());
        }
    }

    #[test]
    fn the_application_id_is_read_and_validated() {
        let ok = app_info(
            Path::new("/proj"),
            ProjectLayout::AndroidAppShared,
            &project(),
        )
        .unwrap();
        assert_eq!(ok.package_candidates[0], "com.example.lf_pos_mobile");
        assert_eq!(ok.module, "androidApp");

        for bad in [
            "applicationId = \"com.example;rm\"",
            "applicationId = \"nodots\"",
            "applicationId = \"-x.y\"",
            "no id here",
        ] {
            let sys = project().file("/proj/androidApp/build.gradle.kts", bad);
            assert!(
                app_info(Path::new("/proj"), ProjectLayout::AndroidAppShared, &sys).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn falls_back_to_the_namespace_and_groovy_syntax() {
        let sys = project().file(
            "/proj/androidApp/build.gradle.kts",
            "namespace 'com.a.b'\nnamespace \"com.x.y\"",
        );
        let app = app_info(Path::new("/proj"), ProjectLayout::AndroidAppShared, &sys).unwrap();

        assert_eq!(app.package_candidates[0], "com.x.y");
    }

    #[test]
    fn a_gradle_wrapper_that_leads_outside_the_project_is_refused() {
        let sys = project()
            .file("/elsewhere/gradlew", "")
            .symlink("/proj/gradlew", "/elsewhere/gradlew");
        let streamer = ScriptedStreamer::new(vec![]);
        let device = emulator(Some("emulator-5554"));

        let outcome = build_and_run(
            &ctx(&device),
            &sys,
            &streamer,
            &mut Recorder::default(),
            &CancelFlag::default(),
        );

        assert!(matches!(outcome, BuildOutcome::Failed(Some(m)) if m.contains("outside")));
        assert!(streamer.specs.borrow().is_empty());
    }

    #[test]
    fn a_missing_wrapper_or_build_file_is_explained() {
        let no_wrapper = FakeSystem::new()
            .file("/proj/androidApp/build.gradle.kts", BUILD_FILE)
            .dir("/proj", &[]);
        let device = emulator(Some("emulator-5554"));
        let a = build_and_run(
            &ctx(&device),
            &no_wrapper,
            &ScriptedStreamer::new(vec![]),
            &mut Recorder::default(),
            &CancelFlag::default(),
        );
        let b = build_and_run(
            &ctx(&device),
            &FakeSystem::new(),
            &ScriptedStreamer::new(vec![]),
            &mut Recorder::default(),
            &CancelFlag::default(),
        );

        assert!(matches!(a, BuildOutcome::Failed(Some(m)) if m.contains("gradlew")));
        assert!(matches!(b, BuildOutcome::Failed(Some(m)) if m.contains("build.gradle")));
    }

    #[test]
    fn the_compose_app_layout_builds_the_compose_app_module() {
        let sys = project().file("/proj/composeApp/build.gradle.kts", BUILD_FILE);

        let app = app_info(Path::new("/proj"), ProjectLayout::ComposeApp, &sys).unwrap();

        assert_eq!(app.module, "composeApp");
    }
}
