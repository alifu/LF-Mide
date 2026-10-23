//! Android and iOS tooling behind a common shape: what is installed, which
//! emulators/simulators exist and whether they are running, and how to start
//! and stop them.

pub mod android;
pub mod ios;

use std::collections::BTreeMap;

use lf_mide_core::{Device, Platform, SdkInfo};
use lf_mide_system::System;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PlatformError {
    #[error("`{tool}` was not found: {hint}")]
    ToolMissing {
        tool: &'static str,
        hint: &'static str,
    },
    #[error("{0}")]
    Failed(String),
}

/// Every device found, plus why a platform could not be listed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Listing {
    pub devices: Vec<Device>,
    pub errors: BTreeMap<Platform, String>,
}

/// Lists Android emulators and iOS simulators (the two are queried at the
/// same time). One platform failing does not hide the other.
pub fn list_all(sys: &dyn System) -> Listing {
    let (android, ios) = std::thread::scope(|scope| {
        let android = scope.spawn(|| android::list_devices(sys));
        let ios = ios::list_devices(sys);
        let android = android.join().unwrap_or_else(|_| {
            Err(PlatformError::Failed(
                "listing emulators crashed".to_string(),
            ))
        });
        (android, ios)
    });

    let mut listing = Listing::default();
    for (platform, result) in [(Platform::Android, android), (Platform::Ios, ios)] {
        match result {
            Ok(devices) => listing.devices.extend(devices),
            Err(error) => {
                listing.errors.insert(platform, error.to_string());
            }
        }
    }
    listing
}

/// What is installed for each platform.
pub fn sdk_info(sys: &dyn System) -> SdkInfo {
    SdkInfo {
        android: android::sdk_info(sys),
        ios: ios::xcode_info(sys),
        home: sys.home(),
    }
}

/// Starts `device` (an emulator or a simulator).
pub fn boot(sys: &dyn System, device: &Device) -> Result<(), PlatformError> {
    match device.platform {
        Platform::Android => android::boot(sys, device),
        Platform::Ios => ios::boot(sys, device),
    }
}

/// Stops `device`.
pub fn stop(sys: &dyn System, device: &Device) -> Result<(), PlatformError> {
    match device.platform {
        Platform::Android => android::stop(sys, device),
        Platform::Ios => ios::stop(sys, device),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lf_mide_core::DeviceState;
    use lf_mide_system::fake::FakeSystem;

    const SIMCTL: &str = include_str!("../fixtures/simctl_devices.json");

    fn ios_ready() -> FakeSystem {
        FakeSystem::new().ok("xcrun simctl list devices available --json", SIMCTL)
    }

    #[test]
    fn sdk_info_combines_both_platforms_and_knows_home() {
        let sys = FakeSystem::new()
            .home("/Users/test")
            .env("ANDROID_HOME", "/sdk")
            .dir("/sdk", &[])
            .ok(
                "xcode-select -p",
                "/Applications/Xcode.app/Contents/Developer\n",
            )
            .ok("xcodebuild -version", "Xcode 26.5\nBuild version 17F42\n");

        let info = sdk_info(&sys);

        assert_eq!(info.android.root, Some(std::path::PathBuf::from("/sdk")));
        assert_eq!(info.ios.version.as_deref(), Some("Xcode 26.5"));
        assert_eq!(info.home, Some(std::path::PathBuf::from("/Users/test")));
    }

    #[test]
    fn a_missing_platform_does_not_hide_the_other() {
        // No Android SDK at all, but simulators work.
        let listing = list_all(&ios_ready().env("PATH", "/bin"));

        assert!(listing.devices.iter().all(|d| d.platform == Platform::Ios));
        assert!(!listing.devices.is_empty());
        assert!(
            listing.errors[&Platform::Android].contains("emulator"),
            "{:?}",
            listing.errors
        );
        assert!(!listing.errors.contains_key(&Platform::Ios));
    }

    #[test]
    fn both_platforms_failing_is_reported_for_each() {
        let listing = list_all(&FakeSystem::new().env("PATH", "/bin"));

        assert!(listing.devices.is_empty());
        assert_eq!(listing.errors.len(), 2);
    }

    #[test]
    fn devices_from_both_platforms_are_combined() {
        let sys = ios_ready()
            .env("ANDROID_HOME", "/sdk")
            .dir("/sdk", &[])
            .file("/sdk/emulator/emulator", "")
            .ok("/sdk/emulator/emulator -list-avds", "Pixel_8\n");

        let listing = list_all(&sys);

        let platforms: Vec<Platform> = listing.devices.iter().map(|d| d.platform).collect();
        assert!(platforms.contains(&Platform::Android) && platforms.contains(&Platform::Ios));
        assert!(listing.errors.is_empty(), "{:?}", listing.errors);
        let pixel = listing.devices.iter().find(|d| d.id == "Pixel_8").unwrap();
        assert_eq!(pixel.state, DeviceState::Stopped);
    }
}
