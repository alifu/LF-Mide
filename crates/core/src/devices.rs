//! Build targets, emulators/simulators and SDK facts, as plain data.

use std::path::{Path, PathBuf};

/// The two platforms a Compose Multiplatform app builds for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Platform {
    Android,
    Ios,
}

impl Platform {
    pub const ALL: [Platform; 2] = [Platform::Android, Platform::Ios];

    pub fn label(self) -> &'static str {
        match self {
            Platform::Android => "Android",
            Platform::Ios => "iOS",
        }
    }

    /// The value stored in preference files.
    pub fn config_name(self) -> &'static str {
        match self {
            Platform::Android => "android",
            Platform::Ios => "ios",
        }
    }

    pub fn from_config_name(name: &str) -> Option<Platform> {
        Platform::ALL.into_iter().find(|p| p.config_name() == name)
    }

    pub fn other(self) -> Platform {
        match self {
            Platform::Android => Platform::Ios,
            Platform::Ios => Platform::Android,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceState {
    Running,
    Stopped,
    Starting,
    Stopping,
}

/// An Android emulator (AVD) or an iOS simulator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub platform: Platform,
    /// Stable identity: the AVD name, or the simulator UDID.
    pub id: String,
    /// What to show: the AVD name, or the simulator name.
    pub name: String,
    /// Extra facts: `API 36`, `iOS 26.0`.
    pub detail: String,
    pub state: DeviceState,
    /// `emulator-5554` while an AVD is running.
    pub serial: Option<String>,
}

impl Device {
    pub fn is_running(&self) -> bool {
        self.state == DeviceState::Running
    }
}

/// A command-line tool found on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolInfo {
    pub path: PathBuf,
    pub version: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AndroidSdk {
    pub root: Option<PathBuf>,
    /// Where `root` came from, e.g. `ANDROID_HOME`.
    pub source: Option<String>,
    pub adb: Option<ToolInfo>,
    pub emulator: Option<ToolInfo>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct XcodeSdk {
    pub developer_dir: Option<PathBuf>,
    /// e.g. `Xcode 26.5`.
    pub version: Option<String>,
    /// Whether `xcrun simctl` works.
    pub simctl: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SdkInfo {
    pub android: AndroidSdk,
    pub ios: XcodeSdk,
    /// The user's home directory, so paths can be shown as `~/...`.
    pub home: Option<PathBuf>,
}

/// `path` for display, with the home directory written as `~`.
pub fn display_path(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|home| path.strip_prefix(home).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_names_round_trip_and_flip() {
        for platform in Platform::ALL {
            assert_eq!(
                Platform::from_config_name(platform.config_name()),
                Some(platform)
            );
            assert_eq!(platform.other().other(), platform);
            assert_ne!(platform.other(), platform);
        }
        assert_eq!(Platform::Android.label(), "Android");
        assert_eq!(Platform::Ios.label(), "iOS");
        assert_eq!(Platform::from_config_name("windows"), None);
        assert_eq!(Platform::from_config_name(""), None);
    }

    #[test]
    fn display_path_writes_home_as_tilde_by_whole_components() {
        let home = Path::new("/Users/alif");

        assert_eq!(
            display_path(Path::new("/Users/alif/Library/Android/sdk"), Some(home)),
            "~/Library/Android/sdk"
        );
        assert_eq!(display_path(Path::new("/Users/alif"), Some(home)), "~");
        assert_eq!(
            display_path(Path::new("/Users/alifu/x"), Some(home)),
            "/Users/alifu/x",
            "a longer name is not inside home"
        );
        assert_eq!(display_path(Path::new("/opt/sdk"), Some(home)), "/opt/sdk");
        assert_eq!(
            display_path(Path::new("/Users/alif/x"), None),
            "/Users/alif/x"
        );
    }

    #[test]
    fn a_device_is_running_only_in_the_running_state() {
        let mut device = Device {
            platform: Platform::Android,
            id: "Pixel".into(),
            name: "Pixel".into(),
            detail: "API 36".into(),
            state: DeviceState::Stopped,
            serial: None,
        };
        assert!(!device.is_running());
        for state in [DeviceState::Starting, DeviceState::Stopping] {
            device.state = state;
            assert!(!device.is_running());
        }
        device.state = DeviceState::Running;
        assert!(device.is_running());
    }
}
