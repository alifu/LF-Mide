//! Stopping the app a run started, on the device it was started on.

use lf_mide_core::{Device, Platform};
use lf_mide_platform::android::adb_path;
use lf_mide_system::System;

use crate::plain_identifier;

/// Stops `app_id` on `device`: `am force-stop` on an emulator, `simctl
/// terminate` on a simulator. Everything that reaches a command line is
/// validated first; the app id came from the project's build files.
pub fn stop_app(device: &Device, app_id: &str, sys: &dyn System) -> Result<(), String> {
    if !plain_identifier(app_id) {
        return Err("the app id looks unusual".to_string());
    }
    match device.platform {
        Platform::Android => {
            let serial = device
                .serial
                .as_deref()
                .filter(|serial| plain_identifier(serial))
                .ok_or_else(|| format!("{} has no adb serial", device.name))?;
            let adb = adb_path(sys).ok_or_else(|| "adb was not found".to_string())?;
            run(
                sys,
                &adb.display().to_string(),
                &["-s", serial, "shell", "am", "force-stop", app_id],
            )
        }
        Platform::Ios => {
            if !lf_mide_platform::ios::valid_udid(&device.id) {
                return Err(format!("{} has an unusual id", device.name));
            }
            run(sys, "xcrun", &["simctl", "terminate", &device.id, app_id])
        }
    }
}

fn run(sys: &dyn System, program: &str, args: &[&str]) -> Result<(), String> {
    let output = sys
        .run(program, args)
        .map_err(|error| format!("{program}: {error}"))?;
    if output.success {
        return Ok(());
    }
    let reason = output.stderr.lines().next().unwrap_or("it failed").trim();
    Err(reason.to_string())
}

#[cfg(test)]
mod tests {
    use lf_mide_core::DeviceState;
    use lf_mide_system::fake::FakeSystem;

    use super::*;

    fn device(platform: Platform, id: &str, serial: Option<&str>) -> Device {
        Device {
            platform,
            id: id.into(),
            name: "Pixel_8".into(),
            detail: String::new(),
            state: DeviceState::Running,
            serial: serial.map(String::from),
        }
    }

    const FORCE_STOP: &str =
        "/sdk/platform-tools/adb -s emulator-5554 shell am force-stop com.example.app";

    fn with_adb() -> FakeSystem {
        FakeSystem::new()
            .env("ANDROID_HOME", "/sdk")
            .dir("/sdk", &[])
            .file("/sdk/platform-tools/adb", "")
    }

    #[test]
    fn an_emulator_app_is_stopped_with_force_stop_on_its_own_serial() {
        let sys = with_adb().ok(FORCE_STOP, "");
        let phone = device(Platform::Android, "Pixel_8", Some("emulator-5554"));

        assert_eq!(stop_app(&phone, "com.example.app", &sys), Ok(()));
    }

    #[test]
    fn a_simulator_app_is_terminated_by_udid() {
        let udid = "11111111-1111-1111-1111-111111111111";
        let sys = FakeSystem::new().ok(
            &format!("xcrun simctl terminate {udid} com.example.app"),
            "",
        );
        let sim = device(Platform::Ios, udid, None);

        assert_eq!(stop_app(&sim, "com.example.app", &sys), Ok(()));
    }

    #[test]
    fn a_failing_tool_says_why() {
        let sys = with_adb().fails(FORCE_STOP, "error: device offline\nmore");
        let phone = device(Platform::Android, "Pixel_8", Some("emulator-5554"));

        let error = stop_app(&phone, "com.example.app", &sys).unwrap_err();

        assert_eq!(error, "error: device offline");
    }

    #[test]
    fn an_app_id_that_could_be_an_option_is_refused() {
        let sys = FakeSystem::default();
        let phone = device(Platform::Android, "Pixel_8", Some("emulator-5554"));

        assert!(stop_app(&phone, "--help", &sys).is_err());
        assert!(stop_app(&phone, "a b", &sys).is_err());
    }

    #[test]
    fn an_emulator_without_a_serial_cannot_be_addressed() {
        let phone = device(Platform::Android, "Pixel_8", None);

        let error = stop_app(&phone, "com.example.app", &FakeSystem::default()).unwrap_err();

        assert!(error.contains("no adb serial"), "{error}");
    }

    #[test]
    fn a_simulator_with_an_odd_id_is_refused() {
        let sim = device(Platform::Ios, "not-a-udid", None);

        assert!(stop_app(&sim, "com.example.app", &FakeSystem::default()).is_err());
    }
}
