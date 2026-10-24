//! Saving settings the user changes inside the app.

use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

use lf_mide_core::{ProjectPrefs, TerminalKind, set_terminal_in_config};
use lf_mide_system::{System, SystemRunner};

/// Mode for a config file we create.
const DEFAULT_MODE: u32 = 0o644;
/// Mode for per-project preferences: nobody else needs to read them.
const PRIVATE_MODE: u32 = 0o600;

/// Writes `terminal = "<kind>"` into the config file at `path`, keeping the
/// rest of the file. The file (and its directory) are created if missing.
/// The write is atomic: a crash can't leave a half-written config.
///
/// A symlinked config (dotfile managers) is updated in place and stays a
/// symlink; the file's permissions are kept.
pub fn save_terminal(path: &Path, kind: TerminalKind) -> io::Result<()> {
    write_atomically(path, DEFAULT_MODE, |existing| {
        set_terminal_in_config(existing, kind)
    })
}

/// Remembers the build target and chosen devices of one project.
pub fn save_prefs(path: &Path, prefs: &ProjectPrefs) -> io::Result<()> {
    write_atomically(path, PRIVATE_MODE, |_| prefs.to_toml())
}

/// The remembered choices, or none if there is no (readable) file. Only a
/// regular file of sensible size is read; damage is never an error.
pub fn load_prefs(path: &Path) -> ProjectPrefs {
    SystemRunner::new()
        .read_to_string(path)
        .map(|text| ProjectPrefs::parse(&text))
        .unwrap_or_default()
}

/// Replaces the file at `path` with `new_content(existing text)`.
///
/// Atomic (temp file + rename, synced first). A symlink at `path` is
/// followed so the link survives; an existing file keeps its permissions and
/// a new one gets `new_mode`. The temp file is created with `create_new`
/// after removing any leftover, so a planted symlink is never written through.
fn write_atomically(
    path: &Path,
    new_mode: u32,
    new_content: impl FnOnce(&str) -> String,
) -> io::Result<()> {
    let target = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let existing = match fs::read_to_string(&target) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(err),
    };
    let updated = new_content(&existing);
    let mode = fs::metadata(&target)
        .map(|meta| meta.permissions().mode() & 0o777)
        .unwrap_or(new_mode);

    let dir = target.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "config path has no directory")
    })?;
    fs::create_dir_all(dir)?;
    let temp = dir.join(format!(".config.{}.tmp", std::process::id()));
    match fs::remove_file(&temp) {
        Err(err) if err.kind() != io::ErrorKind::NotFound => return Err(err),
        _ => {}
    }
    let written = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&temp)?;
        file.write_all(updated.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temp, &target)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&temp);
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;
    use lf_mide_core::Platform;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn creates_the_file_and_directory() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("lf-mide/config.toml");

        save_terminal(&path, TerminalKind::Iterm2).unwrap();

        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "terminal = \"iterm2\"\n"
        );
    }

    #[test]
    fn keeps_the_rest_of_the_file_and_replaces_in_place() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.toml");
        fs::write(&path, "# mine\nterminal = \"ghostty\"\nshow_perf = false\n").unwrap();

        save_terminal(&path, TerminalKind::Tmux).unwrap();

        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "# mine\nterminal = \"tmux\"\nshow_perf = false\n"
        );
    }

    #[test]
    fn leaves_no_temporary_files_behind() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("config.toml");

        save_terminal(&path, TerminalKind::Kitty).unwrap();
        save_terminal(&path, TerminalKind::Wezterm).unwrap();

        let names: Vec<_> = fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("config.toml")]);
    }

    #[test]
    fn reports_a_directory_that_cannot_be_created() {
        let tmp = TempDir::new().unwrap();
        let blocker = tmp.path().join("file");
        fs::write(&blocker, "").unwrap();

        let result = save_terminal(&blocker.join("config.toml"), TerminalKind::Tmux);

        assert!(result.is_err());
    }

    #[test]
    fn writes_through_a_symlinked_config_instead_of_replacing_the_link() {
        use std::os::unix::fs::symlink;
        let tmp = TempDir::new().unwrap();
        let dotfiles = tmp.path().join("dotfiles");
        fs::create_dir(&dotfiles).unwrap();
        let real = dotfiles.join("lf-mide.toml");
        fs::write(&real, "show_perf = false\n").unwrap();
        let link = tmp.path().join("config.toml");
        symlink(&real, &link).unwrap();

        save_terminal(&link, TerminalKind::Iterm2).unwrap();

        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            fs::read_to_string(&real).unwrap(),
            "show_perf = false\nterminal = \"iterm2\"\n"
        );
    }

    #[test]
    fn keeps_the_files_permissions_and_defaults_new_files_to_readable() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new().unwrap();
        let private = tmp.path().join("private.toml");
        fs::write(&private, "").unwrap();
        fs::set_permissions(&private, fs::Permissions::from_mode(0o600)).unwrap();
        let fresh = tmp.path().join("fresh.toml");

        save_terminal(&private, TerminalKind::Tmux).unwrap();
        save_terminal(&fresh, TerminalKind::Tmux).unwrap();

        let mode = |p: &std::path::Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&private), 0o600);
        assert_eq!(mode(&fresh), 0o644);
    }

    #[test]
    fn does_not_write_through_a_planted_temp_file_symlink() {
        use std::os::unix::fs::symlink;
        let tmp = TempDir::new().unwrap();
        let victim = tmp.path().join("victim.txt");
        fs::write(&victim, "precious").unwrap();
        symlink(
            &victim,
            tmp.path()
                .join(format!(".config.{}.tmp", std::process::id())),
        )
        .unwrap();

        save_terminal(&tmp.path().join("config.toml"), TerminalKind::Kitty).unwrap();

        assert_eq!(fs::read_to_string(&victim).unwrap(), "precious");
    }

    fn prefs() -> ProjectPrefs {
        ProjectPrefs {
            target: Some(Platform::Ios),
            android_device: Some("Medium_Phone_API_36".into()),
            ios_device: Some("23F0A11F-0DD8-4766-853D-987DE9852BAC".into()),
            android_variant: Some("freeDebug".into()),
            ios_variant: None,
        }
    }

    #[test]
    fn preferences_round_trip_through_a_new_private_file_in_new_folders() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("lf-mide/projects/pos-1234.toml");

        save_prefs(&path, &prefs()).unwrap();

        assert_eq!(load_prefs(&path), prefs());
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn saving_preferences_replaces_the_old_ones_and_leaves_no_temp_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("p.toml");
        save_prefs(&path, &prefs()).unwrap();

        save_prefs(
            &path,
            &ProjectPrefs {
                target: Some(Platform::Android),
                ..ProjectPrefs::default()
            },
        )
        .unwrap();

        assert_eq!(
            load_prefs(&path),
            ProjectPrefs {
                target: Some(Platform::Android),
                ..ProjectPrefs::default()
            }
        );
        assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 1);
    }

    #[test]
    fn missing_garbage_oversized_or_special_files_load_as_defaults() {
        let tmp = TempDir::new().unwrap();
        let garbage = tmp.path().join("garbage.toml");
        fs::write(&garbage, "][ not toml \u{0}").unwrap();
        let huge = tmp.path().join("huge.toml");
        fs::write(
            &huge,
            format!("android_device = \"{}\"\n", "x".repeat(200_000)),
        )
        .unwrap();

        assert_eq!(
            load_prefs(&tmp.path().join("nope.toml")),
            ProjectPrefs::default()
        );
        assert_eq!(load_prefs(&garbage), ProjectPrefs::default());
        assert_eq!(
            load_prefs(&huge),
            ProjectPrefs::default(),
            "the read is capped, so the value is cut off and rejected"
        );
        assert_eq!(
            load_prefs(tmp.path()),
            ProjectPrefs::default(),
            "a directory is not a preferences file"
        );
        assert_eq!(load_prefs(Path::new("/dev/zero")), ProjectPrefs::default());
    }

    #[test]
    fn an_existing_preferences_file_keeps_its_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("p.toml");
        fs::write(&path, "").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();

        save_prefs(&path, &prefs()).unwrap();

        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o640
        );
    }
}
