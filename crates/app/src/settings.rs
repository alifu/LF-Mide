//! Saving settings the user changes inside the app.

use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

use lf_mide_core::{TerminalKind, set_terminal_in_config};

/// Mode for a config file we create.
const DEFAULT_MODE: u32 = 0o644;

/// Writes `terminal = "<kind>"` into the config file at `path`, keeping the
/// rest of the file. The file (and its directory) are created if missing.
/// The write is atomic: a crash can't leave a half-written config.
///
/// A symlinked config (dotfile managers) is updated in place and stays a
/// symlink; the file's permissions are kept.
pub fn save_terminal(path: &Path, kind: TerminalKind) -> io::Result<()> {
    let target = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let existing = match fs::read_to_string(&target) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(err),
    };
    let updated = set_terminal_in_config(&existing, kind);
    let mode = fs::metadata(&target)
        .map(|meta| meta.permissions().mode() & 0o777)
        .unwrap_or(DEFAULT_MODE);

    let dir = target.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "config path has no directory")
    })?;
    fs::create_dir_all(dir)?;
    let temp = dir.join(format!(".config.{}.tmp", std::process::id()));
    // Remove first (a planted symlink goes, not its target), then create_new
    // so we never write through one.
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
}
