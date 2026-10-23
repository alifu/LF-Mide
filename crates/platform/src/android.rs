//! Android emulators (AVDs) and the SDK tools around them.
//!
//! Trust: tools are only taken from `ANDROID_HOME`, `ANDROID_SDK_ROOT` or the
//! default `~/Library/Android/sdk`. A project's `local.properties` is
//! repository-controlled and never decides which programs we run.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use lf_mide_core::version::Version;
use lf_mide_core::{AndroidSdk, Device, DeviceState, Platform, ToolInfo, props};
use lf_mide_system::{RunError, System};

use crate::PlatformError;

const DEFAULT_SDK_RELATIVE: &str = "Library/Android/sdk";
const EMULATOR_HINT: &str = "install the Android SDK (brew install --cask android-commandlinetools) and the emulator package";
const MAX_AVD_NAME: usize = 100;
const ADB_HINT: &str = "install the Android platform-tools (sdkmanager \"platform-tools\")";

/// Where the SDK is, and how we know.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sdk {
    pub root: PathBuf,
    pub source: &'static str,
}

/// One line of `adb devices -l`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdbDevice {
    pub serial: String,
    /// `device`, `offline`, `unauthorized`, ...
    pub state: String,
    pub model: Option<String>,
}

/// First existing SDK directory from `ANDROID_HOME`, `ANDROID_SDK_ROOT`,
/// then `~/Library/Android/sdk`.
pub fn find_sdk(sys: &dyn System) -> Option<Sdk> {
    let from_env = |key: &'static str| {
        sys.env(key)
            .filter(|value| !value.is_empty())
            .map(|value| (PathBuf::from(value), key))
    };
    let default_location = || {
        let home = sys.home()?;
        Some((home.join(DEFAULT_SDK_RELATIVE), "default location"))
    };
    [
        from_env("ANDROID_HOME"),
        from_env("ANDROID_SDK_ROOT"),
        default_location(),
    ]
    .into_iter()
    .flatten()
    .find(|(path, _)| sys.is_dir(path))
    .map(|(root, source)| Sdk { root, source })
}

/// `relative` inside the SDK if it exists there, else `name` on `$PATH`.
fn tool_path(sys: &dyn System, name: &str, relative: &str) -> Option<PathBuf> {
    find_sdk(sys)
        .map(|sdk| sdk.root.join(relative))
        .filter(|path| sys.exists(path))
        .or_else(|| sys.which(name))
}

fn adb_path(sys: &dyn System) -> Option<PathBuf> {
    tool_path(sys, "adb", "platform-tools/adb")
}

fn emulator_path(sys: &dyn System) -> Option<PathBuf> {
    tool_path(sys, "emulator", "emulator/emulator")
}

fn tool_info(sys: &dyn System, path: PathBuf, version_args: &[&str]) -> ToolInfo {
    let version = sys
        .run(&path.display().to_string(), version_args)
        .ok()
        .filter(|output| output.success)
        .and_then(|output| Version::find_in(&output.text()))
        .map(|version| version.to_string());
    ToolInfo { path, version }
}

/// Facts about the installed SDK tools.
pub fn sdk_info(sys: &dyn System) -> AndroidSdk {
    let sdk = find_sdk(sys);
    AndroidSdk {
        root: sdk.as_ref().map(|sdk| sdk.root.clone()),
        source: sdk.map(|sdk| sdk.source.to_string()),
        adb: adb_path(sys).map(|path| tool_info(sys, path, &["version"])),
        emulator: emulator_path(sys).map(|path| tool_info(sys, path, &["-version"])),
    }
}

/// Devices listed by `adb devices -l` (header and daemon noise skipped).
pub fn parse_adb_devices(output: &str) -> Vec<AdbDevice> {
    output
        .lines()
        .map(str::trim)
        .filter(|line| {
            !line.is_empty() && !line.starts_with('*') && !line.starts_with("List of devices")
        })
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let serial = words.next()?.to_string();
            let state = words.next()?.to_string();
            let model = words
                .find_map(|word| word.strip_prefix("model:"))
                .map(String::from);
            Some(AdbDevice {
                serial,
                state,
                model,
            })
        })
        .collect()
}

/// AVD names from `emulator -list-avds` (log lines skipped).
pub fn parse_avd_names(output: &str) -> Vec<String> {
    output
        .lines()
        .map(str::trim)
        // The emulator prints `INFO | ...` log lines around the names.
        .filter(|line| !line.is_empty() && !line.contains(char::is_whitespace))
        .map(String::from)
        .collect()
}

/// Whether `name` is a plausible AVD name: letters, digits, `.`, `_`, `-`,
/// not starting with `-` or `.`. Anything else is never passed on.
pub fn valid_avd_name(name: &str) -> bool {
    let plain = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-');
    !name.is_empty()
        && name.len() <= MAX_AVD_NAME
        && !name.starts_with(['-', '.'])
        && name.chars().all(plain)
}

/// `API 36 · arm64-v8a` from an AVD's `config.ini`.
pub fn avd_detail(config_ini: &str) -> String {
    let api = props::get(config_ini, "image.sysdir.1").and_then(|dir| {
        let level = dir
            .split('/')
            .find_map(|part| part.strip_prefix("android-"))?;
        (!level.is_empty()).then(|| format!("API {level}"))
    });
    let abi = props::get(config_ini, "abi.type").filter(|abi| !abi.is_empty());
    match (api, abi) {
        (Some(api), Some(abi)) => format!("{api} · {abi}"),
        (Some(api), None) => api,
        (None, Some(abi)) => abi,
        (None, None) => String::new(),
    }
}

/// Where AVD definitions live: `$ANDROID_AVD_HOME`, `$ANDROID_USER_HOME/avd`
/// or `~/.android/avd`.
fn avd_home(sys: &dyn System) -> Option<PathBuf> {
    if let Some(dir) = sys.env("ANDROID_AVD_HOME").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    if let Some(dir) = sys.env("ANDROID_USER_HOME").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(dir).join("avd"));
    }
    Some(sys.home()?.join(".android/avd"))
}

/// The detail line for an AVD, from its `<name>.ini` and the `config.ini` it
/// points to. Empty when either is unreadable.
fn read_avd_detail(sys: &dyn System, home: &Path, name: &str) -> String {
    let Some(ini) = sys.read_to_string(&home.join(format!("{name}.ini"))) else {
        return String::new();
    };
    let dir = match props::get(&ini, "path").map(PathBuf::from) {
        Some(path) if path.is_absolute() => path,
        _ => match (props::get(&ini, "path.rel"), home.parent()) {
            (Some(rel), Some(base)) => base.join(rel),
            _ => return String::new(),
        },
    };
    sys.read_to_string(&dir.join("config.ini"))
        .map(|config| avd_detail(&config))
        .unwrap_or_default()
}

/// AVD name -> `emulator-NNNN` for every emulator that is up. Failures just
/// mean "none running": adb being absent or slow must not hide the AVDs.
fn running_emulators(sys: &dyn System) -> BTreeMap<String, String> {
    let mut running = BTreeMap::new();
    let Some(adb) = adb_path(sys) else {
        return running;
    };
    let adb = adb.display().to_string();
    let Ok(listing) = sys.run(&adb, &["devices", "-l"]) else {
        return running;
    };
    for device in parse_adb_devices(&listing.stdout) {
        if !valid_serial(&device.serial) || device.state != "device" {
            continue;
        }
        let Ok(named) = sys.run(&adb, &["-s", &device.serial, "emu", "avd", "name"]) else {
            continue;
        };
        let name = named.stdout.lines().next().unwrap_or("").trim();
        if named.success && valid_avd_name(name) {
            running.insert(name.to_string(), device.serial);
        }
    }
    running
}

/// `emulator-5554`: the only serials we ever pass to adb.
fn valid_serial(serial: &str) -> bool {
    serial
        .strip_prefix("emulator-")
        .is_some_and(|port| !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()))
}

fn failed(message: impl Into<String>) -> PlatformError {
    PlatformError::Failed(message.into())
}

/// Every AVD, with whether it is running.
pub fn list_devices(sys: &dyn System) -> Result<Vec<Device>, PlatformError> {
    let emulator = emulator_path(sys).ok_or(PlatformError::ToolMissing {
        tool: "emulator",
        hint: EMULATOR_HINT,
    })?;
    let output = sys
        .run(&emulator.display().to_string(), &["-list-avds"])
        .map_err(|error| failed(format!("emulator: {error}")))?;
    if !output.success {
        let reason = output
            .stderr
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("exited with an error");
        return Err(failed(format!(
            "emulator -list-avds failed: {}",
            reason.trim()
        )));
    }

    let running = running_emulators(sys);
    let home = avd_home(sys);
    Ok(parse_avd_names(&output.stdout)
        .into_iter()
        .filter(|name| valid_avd_name(name))
        .map(|name| {
            let serial = running.get(&name).cloned();
            Device {
                platform: Platform::Android,
                detail: home
                    .as_deref()
                    .map(|h| read_avd_detail(sys, h, &name))
                    .unwrap_or_default(),
                state: if serial.is_some() {
                    DeviceState::Running
                } else {
                    DeviceState::Stopped
                },
                serial,
                name: name.clone(),
                id: name,
            }
        })
        .collect())
}

fn require_android(device: &Device) -> Result<(), PlatformError> {
    if device.platform == Platform::Android {
        Ok(())
    } else {
        Err(failed(format!(
            "{} is not an Android emulator",
            device.name
        )))
    }
}

/// Starts the emulator for `device` and returns at once (it takes about a
/// minute to finish booting).
pub fn boot(sys: &dyn System, device: &Device) -> Result<(), PlatformError> {
    require_android(device)?;
    if !valid_avd_name(&device.id) {
        return Err(failed(
            "that emulator has an unusual name and was not started",
        ));
    }
    match device.state {
        DeviceState::Stopped => {}
        DeviceState::Running => return Err(failed(format!("{} is already running", device.name))),
        DeviceState::Starting => {
            return Err(failed(format!("{} is already starting", device.name)));
        }
        DeviceState::Stopping => return Err(failed(format!("{} is shutting down", device.name))),
    }
    let emulator = emulator_path(sys).ok_or(PlatformError::ToolMissing {
        tool: "emulator",
        hint: EMULATOR_HINT,
    })?;
    sys.spawn(&emulator.display().to_string(), &["-avd", &device.id])
        .map_err(|error| match error {
            RunError::NotFound => PlatformError::ToolMissing {
                tool: "emulator",
                hint: EMULATOR_HINT,
            },
            other => failed(format!("could not start {}: {other}", device.name)),
        })
}

/// Shuts down the running emulator for `device`.
pub fn stop(sys: &dyn System, device: &Device) -> Result<(), PlatformError> {
    require_android(device)?;
    let serial = device
        .serial
        .as_deref()
        .filter(|serial| device.is_running() && valid_serial(serial))
        .ok_or_else(|| failed(format!("{} is not running", device.name)))?;
    let adb = adb_path(sys).ok_or(PlatformError::ToolMissing {
        tool: "adb",
        hint: ADB_HINT,
    })?;
    let adb = adb.display().to_string();
    // The console port may have been reused since the last poll: only kill
    // the emulator that is still running this AVD.
    let named = sys
        .run(&adb, &["-s", serial, "emu", "avd", "name"])
        .map_err(|error| failed(format!("adb: {error}")))?;
    if !named.success || named.stdout.lines().next().map(str::trim) != Some(device.id.as_str()) {
        return Err(failed(format!(
            "{} is no longer running on {serial}: refresh and try again",
            device.name
        )));
    }
    let output = sys
        .run(&adb, &["-s", serial, "emu", "kill"])
        .map_err(|error| failed(format!("adb: {error}")))?;
    if output.success {
        return Ok(());
    }
    let reason = output
        .stderr
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("adb exited with an error");
    Err(failed(reason.trim().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lf_mide_system::fake::FakeSystem;

    const ONE_EMULATOR: &str = include_str!("../fixtures/adb_devices_one_emulator.txt");
    const MIXED: &str = include_str!("../fixtures/adb_devices_mixed.txt");
    const NOISY_AVDS: &str = include_str!("../fixtures/emulator_list_avds_noisy.txt");
    const AVD_INI: &str = include_str!("../fixtures/avd.ini");
    const AVD_CONFIG: &str = include_str!("../fixtures/avd_config.ini");

    fn sdk() -> FakeSystem {
        FakeSystem::new()
            .env("ANDROID_HOME", "/sdk")
            .home("/Users/test")
            .dir("/sdk", &[])
            .file("/sdk/emulator/emulator", "")
            .file("/sdk/platform-tools/adb", "")
    }

    /// An SDK with two AVDs, one of them running as emulator-5554.
    fn ready() -> FakeSystem {
        sdk()
            .ok("/sdk/emulator/emulator -list-avds", NOISY_AVDS)
            .ok("/sdk/platform-tools/adb devices -l", ONE_EMULATOR)
            .ok(
                "/sdk/platform-tools/adb -s emulator-5554 emu avd name",
                "Medium_Phone_API_36\nOK\n",
            )
            .file("/Users/test/.android/avd/Medium_Phone_API_36.ini", AVD_INI)
            .file(
                "/Users/test/.android/avd/Medium_Phone.avd/config.ini",
                AVD_CONFIG,
            )
    }

    // --- finding the SDK -------------------------------------------------

    #[test]
    fn finds_the_sdk_from_the_environment_then_the_default_location() {
        let from_home = find_sdk(&sdk()).unwrap();
        let from_root = find_sdk(
            &FakeSystem::new()
                .env("ANDROID_SDK_ROOT", "/r")
                .dir("/r", &[]),
        )
        .unwrap();
        let skipping_missing = find_sdk(
            &FakeSystem::new()
                .env("ANDROID_HOME", "/gone")
                .env("ANDROID_SDK_ROOT", "/r")
                .dir("/r", &[]),
        )
        .unwrap();
        let default = find_sdk(
            &FakeSystem::new()
                .home("/Users/a")
                .dir("/Users/a/Library/Android/sdk", &[]),
        )
        .unwrap();

        assert_eq!(
            (from_home.root, from_home.source),
            (PathBuf::from("/sdk"), "ANDROID_HOME")
        );
        assert_eq!(from_root.source, "ANDROID_SDK_ROOT");
        assert_eq!(skipping_missing.root, PathBuf::from("/r"));
        assert_eq!(default.source, "default location");
        assert!(find_sdk(&FakeSystem::new()).is_none());
    }

    #[test]
    fn sdk_info_reports_paths_and_versions() {
        let sys = sdk()
            .ok(
                "/sdk/platform-tools/adb version",
                "Android Debug Bridge version 1.0.41\nVersion 36.0.0-13206524\n",
            )
            .ok(
                "/sdk/emulator/emulator -version",
                "Android emulator version 36.1.9.0 (build_id 13823996)\n",
            );

        let info = sdk_info(&sys);

        assert_eq!(info.root, Some(PathBuf::from("/sdk")));
        assert_eq!(info.source.as_deref(), Some("ANDROID_HOME"));
        assert_eq!(
            info.adb,
            Some(ToolInfo {
                path: PathBuf::from("/sdk/platform-tools/adb"),
                version: Some("1.0.41".into())
            })
        );
        assert_eq!(info.emulator.unwrap().version.as_deref(), Some("36.1.9"));
    }

    #[test]
    fn sdk_info_falls_back_to_path_and_copes_with_nothing_installed() {
        let on_path = FakeSystem::new()
            .env("PATH", "/opt/bin")
            .file("/opt/bin/adb", "")
            .ok(
                "/opt/bin/adb version",
                "Android Debug Bridge version 1.0.41\n",
            );

        assert_eq!(
            sdk_info(&on_path).adb.unwrap().path,
            PathBuf::from("/opt/bin/adb")
        );
        assert_eq!(sdk_info(&FakeSystem::new()), AndroidSdk::default());
    }

    // --- parsers (real output) -------------------------------------------

    #[test]
    fn parses_adb_devices_from_real_output() {
        let devices = parse_adb_devices(ONE_EMULATOR);

        assert_eq!(
            devices,
            vec![AdbDevice {
                serial: "emulator-5554".into(),
                state: "device".into(),
                model: Some("sdk_gphone64_arm64".into())
            }]
        );
    }

    #[test]
    fn parses_every_adb_state_and_skips_noise() {
        let noisy = format!(
            "* daemon not running; starting now at tcp:5037\n* daemon started successfully\n{MIXED}"
        );

        let devices = parse_adb_devices(&noisy);

        let summary: Vec<_> = devices
            .iter()
            .map(|d| (d.serial.as_str(), d.state.as_str()))
            .collect();
        assert_eq!(
            summary,
            vec![
                ("emulator-5554", "device"),
                ("emulator-5556", "offline"),
                ("R58M12ABCDE", "device"),
                ("0123456789ABCDEF", "unauthorized")
            ]
        );
        assert_eq!(devices[2].model.as_deref(), Some("SM_G973F"));
        assert_eq!(devices[1].model, None);
        assert!(parse_adb_devices("").is_empty());
        assert!(parse_adb_devices("List of devices attached\n\n").is_empty());
    }

    #[test]
    fn parses_avd_names_and_skips_log_lines() {
        assert_eq!(
            parse_avd_names(NOISY_AVDS),
            vec!["Medium_Phone_API_36", "Pixel_8_API_35"]
        );
        assert!(parse_avd_names("").is_empty());
    }

    #[test]
    fn only_plausible_avd_names_are_accepted() {
        for good in ["Pixel_8", "Medium_Phone_API_36", "a.b-c_d", "x1"] {
            assert!(valid_avd_name(good), "{good}");
        }
        for bad in [
            "",
            "-wipe-data",
            ".hidden",
            "..",
            "a b",
            "a/b",
            "a;b",
            "a\nb",
            "é",
            "$(id)",
        ] {
            assert!(!valid_avd_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn avd_detail_is_the_api_level_and_abi() {
        assert_eq!(avd_detail(AVD_CONFIG), "API 36 · arm64-v8a");
        assert_eq!(
            avd_detail(
                "image.sysdir.1 = system-images/android-34/default/x86_64/\nabi.type = x86_64\n"
            ),
            "API 34 · x86_64"
        );
        assert_eq!(
            avd_detail("image.sysdir.1 = system-images/android-35/google_apis/arm64-v8a/\n"),
            "API 35"
        );
        assert_eq!(avd_detail("nothing useful"), "");
        assert_eq!(avd_detail(""), "");
    }

    // --- listing -----------------------------------------------------------

    #[test]
    fn lists_avds_marking_the_running_one_with_its_serial() {
        let devices = list_devices(&ready()).unwrap();

        assert_eq!(devices.len(), 2);
        assert_eq!(
            devices[0],
            Device {
                platform: Platform::Android,
                id: "Medium_Phone_API_36".into(),
                name: "Medium_Phone_API_36".into(),
                detail: "API 36 · arm64-v8a".into(),
                state: DeviceState::Running,
                serial: Some("emulator-5554".into()),
            }
        );
        assert_eq!(
            (
                devices[1].id.as_str(),
                devices[1].state,
                devices[1].serial.clone()
            ),
            ("Pixel_8_API_35", DeviceState::Stopped, None)
        );
    }

    #[test]
    fn physical_phones_and_offline_emulators_are_not_listed_as_running_avds() {
        let offline_and_phone = "List of devices attached\nemulator-5556          offline transport_id:4\nR58M12ABCDE            device usb:1-1 model:SM_G973F transport_id:7\n\n";
        let sys = ready().ok("/sdk/platform-tools/adb devices -l", offline_and_phone);

        let devices = list_devices(&sys).unwrap();

        // The only emulator is offline and the phone is not an emulator: nothing runs.
        assert!(
            devices.iter().all(|d| d.state == DeviceState::Stopped),
            "{devices:?}"
        );
        assert_eq!(devices.len(), 2, "phones are never listed");
    }

    #[test]
    fn a_missing_or_stuck_adb_still_lists_the_avds_as_stopped() {
        let no_adb = sdk().ok("/sdk/emulator/emulator -list-avds", NOISY_AVDS);
        let stuck = sdk()
            .ok("/sdk/emulator/emulator -list-avds", NOISY_AVDS)
            .timeout("/sdk/platform-tools/adb devices -l");

        for sys in [no_adb, stuck] {
            let devices = list_devices(&sys).unwrap();
            assert_eq!(devices.len(), 2);
            assert!(devices.iter().all(|d| d.state == DeviceState::Stopped));
        }
    }

    #[test]
    fn no_emulator_tool_is_an_error_with_a_hint() {
        let err = list_devices(&FakeSystem::new().env("PATH", "/bin")).unwrap_err();

        assert!(
            matches!(
                err,
                PlatformError::ToolMissing {
                    tool: "emulator",
                    ..
                }
            ),
            "{err:?}"
        );
        assert!(err.to_string().contains("android-commandlinetools"));
    }

    #[test]
    fn a_failing_emulator_tool_is_reported() {
        let sys = sdk().fails("/sdk/emulator/emulator -list-avds", "boom");

        let err = list_devices(&sys).unwrap_err();

        assert!(
            matches!(&err, PlatformError::Failed(m) if m.contains("boom")),
            "{err:?}"
        );
    }

    #[test]
    fn avds_with_odd_names_are_ignored_and_missing_config_gives_no_detail() {
        let sys = sdk()
            .ok(
                "/sdk/emulator/emulator -list-avds",
                "Good_One\n-wipe-data\nbad/name\n",
            )
            .ok(
                "/sdk/platform-tools/adb devices -l",
                "List of devices attached\n\n",
            );

        let devices = list_devices(&sys).unwrap();

        assert_eq!(devices.len(), 1);
        assert_eq!(
            (devices[0].id.as_str(), devices[0].detail.as_str()),
            ("Good_One", "")
        );
    }

    // --- boot and stop -----------------------------------------------------

    fn avd(state: DeviceState, serial: Option<&str>) -> Device {
        Device {
            platform: Platform::Android,
            id: "Pixel_8_API_35".into(),
            name: "Pixel_8_API_35".into(),
            detail: String::new(),
            state,
            serial: serial.map(String::from),
        }
    }

    #[test]
    fn boot_starts_the_emulator_detached() {
        let sys = sdk();

        boot(&sys, &avd(DeviceState::Stopped, None)).unwrap();

        assert_eq!(
            sys.spawned(),
            vec!["/sdk/emulator/emulator -avd Pixel_8_API_35"]
        );
    }

    #[test]
    fn boot_refuses_what_it_should_not_start() {
        let sys = sdk();
        let running = boot(&sys, &avd(DeviceState::Running, Some("emulator-5554")));
        let starting = boot(&sys, &avd(DeviceState::Starting, None));
        let odd = boot(
            &sys,
            &Device {
                id: "-wipe-data".into(),
                ..avd(DeviceState::Stopped, None)
            },
        );
        let ios = boot(
            &sys,
            &Device {
                platform: Platform::Ios,
                ..avd(DeviceState::Stopped, None)
            },
        );

        assert!(
            matches!(&running, Err(PlatformError::Failed(m)) if m.contains("already")),
            "{running:?}"
        );
        assert!(starting.is_err() && odd.is_err() && ios.is_err());
        assert!(sys.spawned().is_empty(), "nothing was started");
    }

    #[test]
    fn boot_reports_a_missing_emulator_and_a_failed_start() {
        let missing = boot(
            &FakeSystem::new().env("PATH", "/bin"),
            &avd(DeviceState::Stopped, None),
        );
        let broken = sdk().spawn_fails(
            "/sdk/emulator/emulator -avd Pixel_8_API_35",
            lf_mide_system::RunError::Other("denied".into()),
        );

        assert!(matches!(
            missing,
            Err(PlatformError::ToolMissing {
                tool: "emulator",
                ..
            })
        ));
        assert!(
            matches!(boot(&broken, &avd(DeviceState::Stopped, None)), Err(PlatformError::Failed(m)) if m.contains("denied"))
        );
    }

    #[test]
    fn stop_asks_adb_to_kill_that_emulator() {
        let sys = sdk()
            .ok(
                "/sdk/platform-tools/adb -s emulator-5554 emu avd name",
                "Pixel_8_API_35\nOK\n",
            )
            .ok(
                "/sdk/platform-tools/adb -s emulator-5554 emu kill",
                "OK: killing emulator, bye bye\nOK\n",
            );

        assert_eq!(
            stop(&sys, &avd(DeviceState::Running, Some("emulator-5554"))),
            Ok(())
        );
    }

    #[test]
    fn stop_refuses_without_a_valid_running_emulator() {
        let sys = sdk();

        let stopped = stop(&sys, &avd(DeviceState::Stopped, None));
        let hostile = stop(
            &sys,
            &avd(DeviceState::Running, Some("emulator-5554; rm -rf ~")),
        );
        let no_adb = stop(
            &FakeSystem::new().env("PATH", "/bin"),
            &avd(DeviceState::Running, Some("emulator-5554")),
        );

        assert!(
            matches!(&stopped, Err(PlatformError::Failed(m)) if m.contains("not running")),
            "{stopped:?}"
        );
        assert!(hostile.is_err());
        assert!(matches!(
            no_adb,
            Err(PlatformError::ToolMissing { tool: "adb", .. })
        ));
    }

    #[test]
    fn stop_will_not_kill_an_emulator_that_now_runs_a_different_avd() {
        let sys = sdk()
            .ok(
                "/sdk/platform-tools/adb -s emulator-5554 emu avd name",
                "Other_AVD\nOK\n",
            )
            .ok("/sdk/platform-tools/adb -s emulator-5554 emu kill", "OK\n");

        let err = stop(&sys, &avd(DeviceState::Running, Some("emulator-5554"))).unwrap_err();

        assert!(matches!(err, PlatformError::Failed(m) if m.contains("no longer running")));
    }

    #[test]
    fn stop_reports_a_failed_kill() {
        let sys = sdk()
            .ok(
                "/sdk/platform-tools/adb -s emulator-5554 emu avd name",
                "Pixel_8_API_35\nOK\n",
            )
            .fails(
                "/sdk/platform-tools/adb -s emulator-5554 emu kill",
                "error: device offline",
            );

        let err = stop(&sys, &avd(DeviceState::Running, Some("emulator-5554"))).unwrap_err();

        assert!(matches!(err, PlatformError::Failed(m) if m.contains("offline")));
    }
}
