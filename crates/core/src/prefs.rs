//! Per-project choices that should survive a restart: the build target and
//! the emulator/simulator picked for each platform. They live in the user's
//! config folder, not in the project, so they never dirty the repository
//! and a cloned repository cannot set them.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::devices::Platform;

/// Longest device id accepted from a preferences file.
const MAX_ID_CHARS: usize = 200;
/// Longest readable part of a preferences file name.
const MAX_NAME_CHARS: usize = 40;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectPrefs {
    pub target: Option<Platform>,
    /// AVD name.
    pub android_device: Option<String>,
    /// Simulator UDID.
    pub ios_device: Option<String>,
}

#[derive(Serialize, Deserialize, Default)]
struct Raw {
    target: Option<String>,
    android_device: Option<String>,
    ios_device: Option<String>,
}

impl ProjectPrefs {
    /// Reads preferences leniently: anything unreadable or implausible is
    /// dropped, never an error. A damaged file must not stop the IDE.
    pub fn parse(text: &str) -> ProjectPrefs {
        let raw: Raw = toml::from_str(text).unwrap_or_default();
        ProjectPrefs {
            target: raw.target.as_deref().and_then(Platform::from_config_name),
            android_device: raw.android_device.filter(|id| plausible_id(id)),
            ios_device: raw.ios_device.filter(|id| plausible_id(id)),
        }
    }

    pub fn to_toml(&self) -> String {
        let raw = Raw {
            target: self.target.map(|p| p.config_name().to_string()),
            android_device: self.android_device.clone(),
            ios_device: self.ios_device.clone(),
        };
        // Plain strings only, so serializing cannot fail.
        toml::to_string(&raw).unwrap_or_default()
    }

    pub fn device_for(&self, platform: Platform) -> Option<&str> {
        match platform {
            Platform::Android => self.android_device.as_deref(),
            Platform::Ios => self.ios_device.as_deref(),
        }
    }

    /// These preferences with `device` remembered for `platform` (`None`
    /// forgets it).
    pub fn with_device(&self, platform: Platform, device: Option<String>) -> ProjectPrefs {
        match platform {
            Platform::Android => ProjectPrefs {
                android_device: device,
                ..self.clone()
            },
            Platform::Ios => ProjectPrefs {
                ios_device: device,
                ..self.clone()
            },
        }
    }
}

/// File name for the preferences of the project at `root`: readable name
/// plus a hash of the whole path, so two projects with the same name in
/// different folders do not share a file. Safe to join onto a directory.
pub fn prefs_file_name(root: &Path, project_name: &str) -> String {
    use std::os::unix::ffi::OsStrExt;

    let readable: String = project_name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(MAX_NAME_CHARS)
        .collect();
    let readable = if readable.is_empty() {
        "project".to_string()
    } else {
        readable
    };
    format!(
        "{readable}-{:016x}.toml",
        fnv1a(root.as_os_str().as_bytes())
    )
}

/// 64-bit FNV-1a: tiny, stable across runs and versions (unlike the std
/// hasher), and good enough to tell project folders apart.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

fn plausible_id(id: &str) -> bool {
    !id.is_empty() && id.chars().count() <= MAX_ID_CHARS && !id.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn prefs() -> ProjectPrefs {
        ProjectPrefs {
            target: Some(Platform::Ios),
            android_device: Some("Medium_Phone_API_36".into()),
            ios_device: Some("23F0A11F-0DD8-4766-853D-987DE9852BAC".into()),
        }
    }

    #[test]
    fn preferences_round_trip_through_toml() {
        let text = prefs().to_toml();

        assert_eq!(ProjectPrefs::parse(&text), prefs());
        assert!(text.contains("target = \"ios\""));
    }

    #[test]
    fn empty_preferences_write_nothing_misleading_and_read_back_empty() {
        let text = ProjectPrefs::default().to_toml();

        assert_eq!(ProjectPrefs::parse(&text), ProjectPrefs::default());
    }

    #[test]
    fn damaged_or_unknown_content_becomes_defaults_not_errors() {
        assert_eq!(
            ProjectPrefs::parse("this is [not toml"),
            ProjectPrefs::default()
        );
        assert_eq!(ProjectPrefs::parse(""), ProjectPrefs::default());
        assert_eq!(ProjectPrefs::parse("target = \"windows\"\n").target, None);
        assert_eq!(ProjectPrefs::parse("target = 5\n"), ProjectPrefs::default());
        assert_eq!(
            ProjectPrefs::parse("surprise = 1\ntarget = \"android\"\n").target,
            Some(Platform::Android)
        );
    }

    #[test]
    fn implausible_device_ids_are_dropped() {
        let long = "x".repeat(MAX_ID_CHARS + 1);
        let text = format!("android_device = \"{long}\"\nios_device = \"a\\u001b[2Jb\"\n");

        let parsed = ProjectPrefs::parse(&text);

        assert_eq!(parsed.android_device, None, "too long");
        assert_eq!(parsed.ios_device, None, "control characters");
        assert_eq!(
            ProjectPrefs::parse("android_device = \"\"\n").android_device,
            None,
            "empty"
        );
    }

    #[test]
    fn ids_with_quotes_survive_a_round_trip() {
        let odd = ProjectPrefs {
            android_device: Some("My \"AVD\" \\ name".into()),
            ..ProjectPrefs::default()
        };

        assert_eq!(ProjectPrefs::parse(&odd.to_toml()), odd);
    }

    #[test]
    fn device_for_and_with_device_work_per_platform() {
        let p = prefs();

        assert_eq!(p.device_for(Platform::Android), Some("Medium_Phone_API_36"));
        assert_eq!(
            p.device_for(Platform::Ios),
            Some("23F0A11F-0DD8-4766-853D-987DE9852BAC")
        );
        let changed = p.with_device(Platform::Android, Some("Pixel_8".into()));
        assert_eq!(changed.android_device.as_deref(), Some("Pixel_8"));
        assert_eq!(
            changed.ios_device, p.ios_device,
            "the other platform is untouched"
        );
        assert_eq!(p.with_device(Platform::Ios, None).ios_device, None);
        assert_eq!(
            p.android_device.as_deref(),
            Some("Medium_Phone_API_36"),
            "the original is unchanged"
        );
    }

    #[test]
    fn file_names_are_safe_stable_and_distinguish_folders() {
        let a = prefs_file_name(&PathBuf::from("/work/a/pos"), "pos");
        let b = prefs_file_name(&PathBuf::from("/work/b/pos"), "pos");

        assert_eq!(
            a,
            prefs_file_name(&PathBuf::from("/work/a/pos"), "pos"),
            "stable"
        );
        assert_ne!(a, b, "same name, different folder");
        assert!(a.starts_with("pos-") && a.ends_with(".toml"), "{a}");
    }

    #[test]
    fn hostile_project_names_cannot_escape_the_directory() {
        for name in [
            "../../etc/passwd",
            "a/b",
            "..",
            "",
            "x\0y",
            "naïve ✓ name",
            "con\nsole",
        ] {
            let file = prefs_file_name(&PathBuf::from("/p"), name);

            assert!(
                !file.contains('/')
                    && !file.contains('\\')
                    && !file.contains('\0')
                    && !file.contains('\n'),
                "{name:?} -> {file:?}"
            );
            assert!(!file.starts_with('.'), "{file:?}");
            assert!(file.ends_with(".toml"));
            assert!(file.len() < 80, "{file:?}");
        }
    }
}
