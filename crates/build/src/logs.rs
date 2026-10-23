//! The command that follows a device's log.

use lf_mide_core::{Device, Platform};
use lf_mide_platform::android::adb_path;
use lf_mide_system::System;
use lf_mide_system::stream::StreamSpec;

use crate::{ios, plain_identifier};

/// Recent lines shown when following starts, so the tab is not empty.
const LOGCAT_BACKLOG: &str = "200";

/// How to follow the log of `device`, or why that is not possible.
pub fn log_spec(
    device: &Device,
    derived_data: &std::path::Path,
    sys: &dyn System,
) -> Result<StreamSpec, String> {
    match device.platform {
        Platform::Android => android_spec(device, sys),
        Platform::Ios => ios_spec(device, derived_data, sys),
    }
}

fn android_spec(device: &Device, sys: &dyn System) -> Result<StreamSpec, String> {
    let serial = device
        .serial
        .as_deref()
        .filter(|serial| plain_identifier(serial))
        .ok_or_else(|| format!("{} has no adb serial", device.name))?;
    let adb = adb_path(sys).ok_or_else(|| "adb was not found".to_string())?;
    Ok(StreamSpec {
        program: adb.display().to_string(),
        args: [
            "-s",
            serial,
            "logcat",
            "-v",
            "threadtime",
            "-T",
            LOGCAT_BACKLOG,
        ]
        .map(String::from)
        .to_vec(),
        ..StreamSpec::default()
    })
}

/// The simulator's unified log, limited to the app's own process.
fn ios_spec(
    device: &Device,
    derived_data: &std::path::Path,
    sys: &dyn System,
) -> Result<StreamSpec, String> {
    if !lf_mide_platform::ios::valid_udid(&device.id) {
        return Err(format!("{} has an unusual id", device.name));
    }
    let app = ios::find_app(derived_data, sys)
        .ok_or_else(|| "build the app first (B): there is no process to follow yet".to_string())?;
    let info = ios::bundle_info(&app, sys)?;
    Ok(StreamSpec {
        program: "xcrun".to_string(),
        args: [
            "simctl",
            "spawn",
            &device.id,
            "log",
            "stream",
            "--style",
            "compact",
            "--level",
            "debug",
            "--predicate",
            &format!("process == \"{}\"", info.executable),
        ]
        .map(String::from)
        .to_vec(),
        ..StreamSpec::default()
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use lf_mide_core::DeviceState;
    use lf_mide_system::fake::FakeSystem;

    use super::*;

    const UDID: &str = "23F0A11F-0DD8-4766-853D-987DE9852BAC";
    const APP: &str = "/cache/Build/Products/Debug-iphonesimulator/iosApp.app";

    fn device(platform: Platform, id: &str, serial: Option<&str>) -> Device {
        Device {
            platform,
            id: id.into(),
            name: "Dev".into(),
            detail: String::new(),
            state: DeviceState::Running,
            serial: serial.map(String::from),
        }
    }

    fn sdk() -> FakeSystem {
        FakeSystem::new()
            .env("ANDROID_HOME", "/sdk")
            .dir("/sdk", &[])
            .file("/sdk/platform-tools/adb", "")
    }

    #[test]
    fn logcat_follows_the_chosen_emulator_with_a_short_backlog() {
        let spec = log_spec(
            &device(Platform::Android, "Pixel", Some("emulator-5554")),
            Path::new("/cache"),
            &sdk(),
        )
        .unwrap();

        assert_eq!(spec.program, "/sdk/platform-tools/adb");
        assert_eq!(
            spec.args,
            [
                "-s",
                "emulator-5554",
                "logcat",
                "-v",
                "threadtime",
                "-T",
                "200"
            ]
        );
    }

    #[test]
    fn logcat_needs_a_safe_serial_and_adb() {
        let bad = device(Platform::Android, "Pixel", Some("-e"));
        let none = device(Platform::Android, "Pixel", None);
        let ok = device(Platform::Android, "Pixel", Some("emulator-5554"));

        assert!(log_spec(&bad, Path::new("/c"), &sdk()).is_err());
        assert!(log_spec(&none, Path::new("/c"), &sdk()).is_err());
        assert!(
            log_spec(&ok, Path::new("/c"), &FakeSystem::new())
                .unwrap_err()
                .contains("adb")
        );
    }

    #[test]
    fn the_ios_log_is_limited_to_the_apps_process() {
        let sys = FakeSystem::new()
            .dir(
                "/cache/Build/Products/Debug-iphonesimulator",
                &["iosApp.app"],
            )
            .ok(
                &format!("plutil -extract CFBundleIdentifier raw -o - {APP}/Info.plist"),
                "com.a.b",
            )
            .ok(
                &format!("plutil -extract CFBundleExecutable raw -o - {APP}/Info.plist"),
                "iosApp",
            );

        let spec = log_spec(
            &device(Platform::Ios, UDID, None),
            Path::new("/cache"),
            &sys,
        )
        .unwrap();

        assert_eq!(spec.program, "xcrun");
        assert_eq!(spec.args[..4], ["simctl", "spawn", UDID, "log"]);
        assert_eq!(spec.args.last().unwrap(), "process == \"iosApp\"");
    }

    #[test]
    fn the_ios_log_needs_a_built_app_and_a_valid_id() {
        let missing = log_spec(
            &device(Platform::Ios, UDID, None),
            Path::new("/cache"),
            &FakeSystem::new(),
        );
        let bad = log_spec(
            &device(Platform::Ios, "x y", None),
            Path::new("/cache"),
            &FakeSystem::new(),
        );

        assert!(missing.unwrap_err().contains("build the app first"));
        assert!(bad.unwrap_err().contains("unusual id"));
    }
}
