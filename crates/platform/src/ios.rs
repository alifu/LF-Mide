//! iOS simulators and Xcode.

use std::path::PathBuf;

use lf_mide_core::{Device, DeviceState, Platform, XcodeSdk};
use lf_mide_system::{RunError, System};

use crate::PlatformError;

const SIMCTL_HINT: &str = "install Xcode and select it with `sudo xcode-select -s /Applications/Xcode.app/Contents/Developer`";
/// Every simulator runtime id starts like this; iOS ones continue with `iOS-`.
const RUNTIME_PREFIX: &str = "com.apple.CoreSimulator.SimRuntime.iOS-";

/// `26.0` from `com.apple.CoreSimulator.SimRuntime.iOS-26-0`; `None` for
/// other platforms' runtimes (watchOS, visionOS, ...).
pub fn runtime_version(runtime_id: &str) -> Option<String> {
    let numbers = runtime_id.strip_prefix(RUNTIME_PREFIX)?;
    let parts: Vec<&str> = numbers.split('-').collect();
    let all_numbers = parts
        .iter()
        .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()));
    all_numbers.then(|| parts.join("."))
}

/// A simulator UDID: 8-4-4-4-12 hexadecimal digits. Only these are ever
/// passed to `simctl`.
pub fn valid_udid(udid: &str) -> bool {
    udid.len() == 36
        && udid.chars().enumerate().all(|(index, c)| match index {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_hexdigit(),
        })
}

fn state_of(simctl_state: &str) -> DeviceState {
    match simctl_state {
        "Booted" => DeviceState::Running,
        "Booting" => DeviceState::Starting,
        "Shutting Down" => DeviceState::Stopping,
        _ => DeviceState::Stopped,
    }
}

/// iOS simulators from `xcrun simctl list devices available --json`, newest
/// runtime first and by name within a runtime. Unavailable devices and
/// non-iOS runtimes are left out.
pub fn parse_simctl_devices(json: &str) -> Result<Vec<Device>, String> {
    let document: serde_json::Value = serde_json::from_str(json)
        .map_err(|error| format!("could not read simctl output: {error}"))?;
    let runtimes = document["devices"]
        .as_object()
        .ok_or_else(|| "unexpected simctl output (no device list)".to_string())?;

    // (runtime version as numbers, device); sorted below.
    let mut found: Vec<(Vec<u64>, Device)> = Vec::new();
    for (runtime_id, entries) in runtimes {
        let Some(version) = runtime_version(runtime_id) else {
            continue;
        };
        let key: Vec<u64> = version
            .split('.')
            .filter_map(|part| part.parse().ok())
            .collect();
        for entry in entries.as_array().into_iter().flatten() {
            let udid = entry["udid"].as_str().filter(|udid| valid_udid(udid));
            let name = entry["name"].as_str().filter(|name| !name.is_empty());
            let (Some(udid), Some(name)) = (udid, name) else {
                continue;
            };
            if entry["isAvailable"].as_bool() == Some(false) {
                continue;
            }
            found.push((
                key.clone(),
                Device {
                    platform: Platform::Ios,
                    id: udid.to_string(),
                    name: name.to_string(),
                    detail: format!("iOS {version}"),
                    state: state_of(entry["state"].as_str().unwrap_or("")),
                    serial: None,
                },
            ));
        }
    }
    found.sort_by(|(a_version, a), (b_version, b)| {
        b_version.cmp(a_version).then_with(|| a.name.cmp(&b.name))
    });
    Ok(found.into_iter().map(|(_, device)| device).collect())
}

fn succeeded(sys: &dyn System, program: &str, args: &[&str]) -> Option<String> {
    sys.run(program, args)
        .ok()
        .filter(|output| output.success)
        .map(|output| output.stdout)
}

/// Xcode facts: where it is, its version, and whether `simctl` works.
pub fn xcode_info(sys: &dyn System) -> XcodeSdk {
    XcodeSdk {
        developer_dir: succeeded(sys, "xcode-select", &["-p"])
            .map(|out| PathBuf::from(out.trim()))
            .filter(|path| !path.as_os_str().is_empty()),
        version: succeeded(sys, "xcodebuild", &["-version"])
            .and_then(|out| out.lines().next().map(|line| line.trim().to_string()))
            .filter(|line| !line.is_empty()),
        simctl: succeeded(sys, "xcrun", &["--find", "simctl"]).is_some(),
    }
}

fn failed(message: impl Into<String>) -> PlatformError {
    PlatformError::Failed(message.into())
}

/// The most specific line of a tool's error output (the last one).
fn last_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .unwrap_or("the command failed")
        .to_string()
}

pub fn list_devices(sys: &dyn System) -> Result<Vec<Device>, PlatformError> {
    let output = sys
        .run(
            "xcrun",
            &["simctl", "list", "devices", "available", "--json"],
        )
        .map_err(|error| match error {
            RunError::NotFound => PlatformError::ToolMissing {
                tool: "xcrun",
                hint: SIMCTL_HINT,
            },
            other => failed(format!("simctl: {other}")),
        })?;
    if !output.success {
        return Err(failed(last_line(&output.stderr)));
    }
    parse_simctl_devices(&output.stdout).map_err(failed)
}

fn require_simulator(device: &Device) -> Result<(), PlatformError> {
    if device.platform != Platform::Ios {
        return Err(failed(format!("{} is not an iOS simulator", device.name)));
    }
    if !valid_udid(&device.id) {
        return Err(failed(
            "that simulator has an unusual id and was not touched",
        ));
    }
    Ok(())
}

/// Runs a `simctl` subcommand on `udid`; `already` is the phrase simctl uses
/// when the device is already in the wanted state, which counts as success.
fn simctl(sys: &dyn System, verb: &str, udid: &str, already: &str) -> Result<(), PlatformError> {
    let output = sys
        .run("xcrun", &["simctl", verb, udid])
        .map_err(|error| match error {
            RunError::NotFound => PlatformError::ToolMissing {
                tool: "xcrun",
                hint: SIMCTL_HINT,
            },
            other => failed(format!("simctl {verb}: {other}")),
        })?;
    if output.success || output.stderr.contains(already) {
        return Ok(());
    }
    Err(failed(last_line(&output.stderr)))
}

/// Boots the simulator and brings up the Simulator app showing it.
pub fn boot(sys: &dyn System, device: &Device) -> Result<(), PlatformError> {
    require_simulator(device)?;
    simctl(sys, "boot", &device.id, "current state: Booted")?;
    // Showing the window is a convenience: the simulator is up either way.
    let _ = sys.run(
        "open",
        &[
            "-a",
            "Simulator",
            "--args",
            "-CurrentDeviceUDID",
            &device.id,
        ],
    );
    Ok(())
}

pub fn stop(sys: &dyn System, device: &Device) -> Result<(), PlatformError> {
    require_simulator(device)?;
    simctl(sys, "shutdown", &device.id, "current state: Shutdown")
}

#[cfg(test)]
mod tests {
    use super::*;
    use lf_mide_system::fake::FakeSystem;

    const SIMCTL: &str = include_str!("../fixtures/simctl_devices.json");
    const LIST_KEY: &str = "xcrun simctl list devices available --json";
    const BOOTED_UDID: &str = "23F0A11F-0DD8-4766-853D-987DE9852BAC";

    fn sim(udid: &str, state: DeviceState) -> Device {
        Device {
            platform: Platform::Ios,
            id: udid.into(),
            name: "iPhone 17 Pro".into(),
            detail: "iOS 26.0".into(),
            state,
            serial: None,
        }
    }

    #[test]
    fn runtime_versions_come_from_ios_runtime_ids_only() {
        assert_eq!(
            runtime_version("com.apple.CoreSimulator.SimRuntime.iOS-26-0").as_deref(),
            Some("26.0")
        );
        assert_eq!(
            runtime_version("com.apple.CoreSimulator.SimRuntime.iOS-18-4").as_deref(),
            Some("18.4")
        );
        assert_eq!(
            runtime_version("com.apple.CoreSimulator.SimRuntime.iOS-26-0-1").as_deref(),
            Some("26.0.1")
        );
        assert_eq!(
            runtime_version("com.apple.CoreSimulator.SimRuntime.watchOS-26-0"),
            None
        );
        assert_eq!(
            runtime_version("com.apple.CoreSimulator.SimRuntime.xrOS-2-5"),
            None
        );
        assert_eq!(
            runtime_version("com.apple.CoreSimulator.SimRuntime.iOS-"),
            None
        );
        assert_eq!(
            runtime_version("com.apple.CoreSimulator.SimRuntime.iOS-evil"),
            None
        );
        assert_eq!(runtime_version(""), None);
    }

    #[test]
    fn only_well_formed_udids_are_accepted() {
        assert!(valid_udid(BOOTED_UDID));
        assert!(valid_udid("00000000-0000-0000-0000-000000000099"));
        for bad in [
            "",
            "not-a-udid",
            "23F0A11F-0DD8-4766-853D-987DE9852BA",
            "23F0A11F-0DD8-4766-853D-987DE9852BACC",
            "23F0A11F0DD84766853D987DE9852BAC",
            "23F0A11F-0DD8-4766-853D-987DE9852BAG",
            "-23F0A11F-0DD8-4766-853D-987DE9852BA",
            "23F0A11F-0DD8-4766-853D-987DE9852BAC\n",
        ] {
            assert!(!valid_udid(bad), "{bad:?}");
        }
    }

    #[test]
    fn parses_the_real_simctl_output() {
        let devices = parse_simctl_devices(SIMCTL).unwrap();

        let summary: Vec<_> = devices
            .iter()
            .map(|d| format!("{} | {} | {:?}", d.detail, d.name, d.state))
            .collect();
        assert_eq!(
            summary,
            vec![
                "iOS 27.2 | iPhone 17e | Stopping",
                "iOS 27.2 | iPhone 18 Pro | Stopped",
                "iOS 27.2 | iPhone 18 Pro Max | Starting",
                "iOS 26.0 | iPhone 17 Pro | Running",
                "iOS 26.0 | iPhone 17 Pro Max | Stopped",
                "iOS 26.0 | iPhone Air | Stopped",
            ]
        );
    }

    #[test]
    fn parsed_devices_carry_their_udid_and_no_serial() {
        let devices = parse_simctl_devices(SIMCTL).unwrap();

        let booted = devices.iter().find(|d| d.is_running()).unwrap();
        assert_eq!(booted.id, BOOTED_UDID);
        assert_eq!(booted.platform, Platform::Ios);
        assert!(
            devices
                .iter()
                .all(|d| d.serial.is_none() && valid_udid(&d.id))
        );
    }

    #[test]
    fn leaves_out_unavailable_devices_other_platforms_and_empty_runtimes() {
        let devices = parse_simctl_devices(SIMCTL).unwrap();

        assert!(devices.iter().all(|d| !d.name.contains("unavailable")));
        assert!(
            devices.iter().all(|d| d.detail.starts_with("iOS ")),
            "no watchOS or visionOS"
        );
    }

    #[test]
    fn runtimes_sort_numerically_not_alphabetically() {
        let json = r#"{"devices":{
            "com.apple.CoreSimulator.SimRuntime.iOS-9-3":[{"udid":"11111111-1111-1111-1111-111111111111","name":"Old","state":"Shutdown","isAvailable":true}],
            "com.apple.CoreSimulator.SimRuntime.iOS-18-4":[{"udid":"22222222-2222-2222-2222-222222222222","name":"Mid","state":"Shutdown","isAvailable":true}],
            "com.apple.CoreSimulator.SimRuntime.iOS-26-0":[{"udid":"33333333-3333-3333-3333-333333333333","name":"New","state":"Shutdown","isAvailable":true}]}}"#;

        let names: Vec<_> = parse_simctl_devices(json)
            .unwrap()
            .into_iter()
            .map(|d| d.name)
            .collect();

        assert_eq!(names, vec!["New", "Mid", "Old"]);
    }

    #[test]
    fn malformed_entries_are_skipped_and_bad_documents_are_errors() {
        let json = r#"{"devices":{"com.apple.CoreSimulator.SimRuntime.iOS-26-0":[
            {"udid":"not-a-udid","name":"Bad","state":"Shutdown","isAvailable":true},
            {"name":"NoUdid","state":"Shutdown","isAvailable":true},
            {"udid":"33333333-3333-3333-3333-333333333333","state":"Shutdown","isAvailable":true},
            {"udid":"44444444-4444-4444-4444-444444444444","name":"Good","state":"Mystery","isAvailable":true}]}}"#;

        let devices = parse_simctl_devices(json).unwrap();

        assert_eq!(devices.len(), 1);
        assert_eq!(
            (devices[0].name.as_str(), devices[0].state),
            ("Good", DeviceState::Stopped)
        );
        assert!(parse_simctl_devices("not json").is_err());
        assert!(parse_simctl_devices("{}").is_err());
        assert!(parse_simctl_devices(r#"{"devices": []}"#).is_err());
    }

    #[test]
    fn hostile_device_names_are_kept_as_text() {
        let json = r#"{"devices":{"com.apple.CoreSimulator.SimRuntime.iOS-26-0":[
            {"udid":"33333333-3333-3333-3333-333333333333","name":"Evil\u001b[2J $(id)","state":"Shutdown","isAvailable":true}]}}"#;

        let devices = parse_simctl_devices(json).unwrap();

        assert_eq!(
            devices[0].name, "Evil\u{1b}[2J $(id)",
            "the UI strips control characters when drawing"
        );
    }

    #[test]
    fn lists_devices_through_simctl() {
        let devices = list_devices(&FakeSystem::new().ok(LIST_KEY, SIMCTL)).unwrap();

        assert_eq!(devices.len(), 6);
    }

    #[test]
    fn listing_errors_say_what_went_wrong() {
        let missing = list_devices(&FakeSystem::new()).unwrap_err();
        let failing = list_devices(
            &FakeSystem::new().fails(LIST_KEY, "xcrun: error: unable to find utility \"simctl\""),
        )
        .unwrap_err();
        let garbage = list_devices(&FakeSystem::new().ok(LIST_KEY, "oops")).unwrap_err();

        assert!(matches!(
            missing,
            PlatformError::ToolMissing { tool: "xcrun", .. }
        ));
        assert!(
            matches!(&failing, PlatformError::Failed(m) if m.contains("simctl")),
            "{failing:?}"
        );
        assert!(matches!(garbage, PlatformError::Failed(_)));
    }

    #[test]
    fn xcode_info_reports_directory_version_and_simctl() {
        let sys = FakeSystem::new()
            .ok(
                "xcode-select -p",
                "/Applications/Xcode.app/Contents/Developer\n",
            )
            .ok("xcodebuild -version", "Xcode 26.5\nBuild version 17F42\n")
            .ok(
                "xcrun --find simctl",
                "/Applications/Xcode.app/Contents/Developer/usr/bin/simctl\n",
            );

        let info = xcode_info(&sys);

        assert_eq!(
            info.developer_dir,
            Some(PathBuf::from("/Applications/Xcode.app/Contents/Developer"))
        );
        assert_eq!(info.version.as_deref(), Some("Xcode 26.5"));
        assert!(info.simctl);
        assert_eq!(xcode_info(&FakeSystem::new()), XcodeSdk::default());
    }

    #[test]
    fn boot_starts_the_simulator_and_shows_the_simulator_app() {
        let sys = FakeSystem::new()
            .ok(&format!("xcrun simctl boot {BOOTED_UDID}"), "")
            .ok(
                &format!("open -a Simulator --args -CurrentDeviceUDID {BOOTED_UDID}"),
                "",
            );

        assert_eq!(boot(&sys, &sim(BOOTED_UDID, DeviceState::Stopped)), Ok(()));
    }

    #[test]
    fn boot_treats_already_booted_as_success_and_other_failures_as_errors() {
        let key = format!("xcrun simctl boot {BOOTED_UDID}");
        let already = FakeSystem::new().fails(&key, "An error was encountered processing the command (domain=com.apple.CoreSimulator.SimError, code=405):\nUnable to boot device in current state: Booted\n");
        let broken = FakeSystem::new().fails(&key, "Invalid device state");

        assert_eq!(
            boot(&already, &sim(BOOTED_UDID, DeviceState::Stopped)),
            Ok(())
        );
        assert!(
            matches!(boot(&broken, &sim(BOOTED_UDID, DeviceState::Stopped)), Err(PlatformError::Failed(m)) if m.contains("Invalid device state"))
        );
    }

    #[test]
    fn a_missing_simulator_app_does_not_fail_the_boot() {
        let sys = FakeSystem::new()
            .ok(&format!("xcrun simctl boot {BOOTED_UDID}"), "")
            .fails(
                &format!("open -a Simulator --args -CurrentDeviceUDID {BOOTED_UDID}"),
                "Unable to find application",
            );

        assert_eq!(boot(&sys, &sim(BOOTED_UDID, DeviceState::Stopped)), Ok(()));
    }

    #[test]
    fn boot_and_stop_refuse_bad_ids_and_other_platforms_without_running_anything() {
        let sys = FakeSystem::new();
        let bad = Device {
            id: "-h; rm -rf ~".into(),
            ..sim(BOOTED_UDID, DeviceState::Stopped)
        };
        let android = Device {
            platform: Platform::Android,
            ..sim(BOOTED_UDID, DeviceState::Stopped)
        };

        assert!(boot(&sys, &bad).is_err() && stop(&sys, &bad).is_err());
        assert!(boot(&sys, &android).is_err() && stop(&sys, &android).is_err());
        assert!(sys.spawned().is_empty());
    }

    #[test]
    fn stop_shuts_the_simulator_down_and_tolerates_one_already_off() {
        let key = format!("xcrun simctl shutdown {BOOTED_UDID}");
        let ok = FakeSystem::new().ok(&key, "");
        let already =
            FakeSystem::new().fails(&key, "Unable to shutdown device in current state: Shutdown");
        let broken = FakeSystem::new().fails(&key, "something else");

        assert_eq!(stop(&ok, &sim(BOOTED_UDID, DeviceState::Running)), Ok(()));
        assert_eq!(
            stop(&already, &sim(BOOTED_UDID, DeviceState::Running)),
            Ok(())
        );
        assert!(
            matches!(stop(&broken, &sim(BOOTED_UDID, DeviceState::Running)), Err(PlatformError::Failed(m)) if m.contains("something else"))
        );
    }
}
