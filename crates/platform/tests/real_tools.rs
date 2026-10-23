//! Runs against the real `adb`, `emulator`, `xcrun` on this machine.
//! Read-only: lists and inspects, never boots or stops anything.
//!
//! `cargo test -p lf-mide-platform -- --ignored --nocapture`

use lf_mide_core::Platform;
use lf_mide_platform::{ios, list_all, sdk_info};
use lf_mide_system::SystemRunner;

#[test]
#[ignore = "needs the Android SDK and Xcode installed"]
fn lists_the_devices_this_machine_has() {
    let sys = SystemRunner::new();

    let listing = list_all(&sys);

    for device in &listing.devices {
        println!(
            "{:?} | {} | {} | {} | {:?} | {:?}",
            device.platform, device.id, device.name, device.detail, device.state, device.serial
        );
    }
    println!("errors: {:?}", listing.errors);
    let ios_count = listing
        .devices
        .iter()
        .filter(|d| d.platform == Platform::Ios)
        .count();
    if !listing.errors.contains_key(&Platform::Ios) {
        assert!(
            ios_count > 0,
            "simctl worked but no iOS simulators were found"
        );
        assert!(
            listing
                .devices
                .iter()
                .filter(|d| d.platform == Platform::Ios)
                .all(|d| ios::valid_udid(&d.id))
        );
    }
    if !listing.errors.contains_key(&Platform::Android) {
        assert!(
            listing
                .devices
                .iter()
                .any(|d| d.platform == Platform::Android)
        );
    }
}

#[test]
#[ignore = "needs the Android SDK and Xcode installed"]
fn reports_the_installed_sdks() {
    let info = sdk_info(&SystemRunner::new());

    println!("{info:#?}");
    assert!(info.android.root.is_some() || info.ios.developer_dir.is_some());
    if let Some(adb) = &info.android.adb {
        assert!(adb.version.is_some(), "adb ran but no version was parsed");
    }
    if info.ios.developer_dir.is_some() {
        assert!(
            info.ios
                .version
                .as_deref()
                .is_some_and(|v| v.starts_with("Xcode"))
        );
    }
}
