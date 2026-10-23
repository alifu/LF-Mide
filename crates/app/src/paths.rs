//! Where LF-Mide keeps its files.

use std::path::{Path, PathBuf};

use directories::{BaseDirs, ProjectDirs};
use lf_mide_core::config::PROJECT_CONFIG_FILE;
use lf_mide_core::prefs_file_name;

const APP_NAME: &str = "lf-mide";
const CONFIG_FILE: &str = "config.toml";

/// `$XDG_CONFIG_HOME/lf-mide/config.toml`, else `~/.config/lf-mide/config.toml`.
/// Terminal tools on macOS conventionally use `~/.config`, so we do too.
pub fn global_config() -> Option<PathBuf> {
    global_config_from(
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
        BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf()),
    )
}

fn global_config_from(xdg_config_home: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    let base = xdg_config_home
        .filter(|dir| dir.is_absolute())
        .or_else(|| home.map(|home| home.join(".config")))?;
    Some(base.join(APP_NAME).join(CONFIG_FILE))
}

/// Where this project's remembered build target and devices live:
/// `<config dir>/projects/<name>-<hash>.toml`, next to the global config and
/// outside the repository.
pub fn project_prefs(root: &Path, project_name: &str) -> Option<PathBuf> {
    Some(project_prefs_in(
        global_config()?.parent()?,
        root,
        project_name,
    ))
}

fn project_prefs_in(config_dir: &Path, root: &Path, project_name: &str) -> PathBuf {
    config_dir
        .join("projects")
        .join(prefs_file_name(root, project_name))
}

pub fn project_config(root: &Path) -> PathBuf {
    root.join(PROJECT_CONFIG_FILE)
}

/// `~/Library/Caches/lf-mide` on macOS. Logs go here, never to the terminal.
pub fn log_dir() -> Option<PathBuf> {
    ProjectDirs::from("", "", APP_NAME).map(|dirs| dirs.cache_dir().to_path_buf())
}

/// `<base>/lf-mide-<pid>/`, a directory only this user can enter, so no
/// other local user can sit on or replace the socket or launcher inside it.
/// Creates it, or reuses our own earlier one (clearing a stale socket).
pub fn editor_dir(base: &Path, pid: u32) -> std::io::Result<PathBuf> {
    use std::os::unix::fs::DirBuilderExt;

    let dir = base.join(format!("lf-mide-{pid}"));
    match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
            ensure_private(&dir)?;
            // A socket left by an earlier process with the same pid would
            // make `nvim --listen` fail.
            match std::fs::remove_file(dir.join("nvim.sock")) {
                Err(err) if err.kind() != std::io::ErrorKind::NotFound => return Err(err),
                _ => {}
            }
        }
        Err(err) => return Err(err),
    }
    Ok(dir)
}

/// Writes the launcher script into `dir` as an executable file only this
/// user can touch, replacing any earlier one (never following a symlink).
pub fn write_launcher(dir: &Path, script: &str) -> std::io::Result<PathBuf> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let path = dir.join("open-nvim.sh");
    // Remove first (this removes a symlink itself, not its target), then
    // create_new so we can never write through one.
    match std::fs::remove_file(&path) {
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => return Err(err),
        _ => {}
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o700)
        .open(&path)?;
    file.write_all(script.as_bytes())?;
    Ok(path)
}

/// An existing directory is only reused if it is really ours and closed to
/// everyone else (not a symlink, not group/world accessible).
fn ensure_private(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::MetadataExt;

    let meta = std::fs::symlink_metadata(dir)?;
    // SAFETY: `geteuid` has no preconditions and cannot fail.
    let me = unsafe { libc::geteuid() };
    if meta.is_dir() && meta.mode() & 0o777 == 0o700 && meta.uid() == me {
        return Ok(());
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::PermissionDenied,
        format!("{} is not a private directory owned by you", dir.display()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_config_prefers_absolute_xdg_config_home() {
        let path = global_config_from(Some("/xdg".into()), Some("/home/u".into()));

        assert_eq!(path, Some(PathBuf::from("/xdg/lf-mide/config.toml")));
    }

    #[test]
    fn global_config_falls_back_to_home_dot_config() {
        let unset = global_config_from(None, Some("/home/u".into()));
        // The XDG spec says relative values must be ignored.
        let relative = global_config_from(Some("rel".into()), Some("/home/u".into()));

        let expected = Some(PathBuf::from("/home/u/.config/lf-mide/config.toml"));
        assert_eq!(unset, expected);
        assert_eq!(relative, expected);
    }

    #[test]
    fn global_config_is_none_without_any_base() {
        assert_eq!(global_config_from(None, None), None);
    }

    #[test]
    fn project_preferences_live_beside_the_global_config_not_in_the_project() {
        let path = project_prefs_in(
            Path::new("/home/u/.config/lf-mide"),
            Path::new("/work/pos"),
            "pos",
        );

        assert!(
            path.starts_with("/home/u/.config/lf-mide/projects"),
            "{path:?}"
        );
        assert!(!path.starts_with("/work/pos"));
        assert!(
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("pos-")
        );
        assert_ne!(
            path,
            project_prefs_in(
                Path::new("/home/u/.config/lf-mide"),
                Path::new("/work/other/pos"),
                "pos"
            )
        );
    }

    #[test]
    fn project_config_lives_in_project_root() {
        assert_eq!(
            project_config(Path::new("/p")),
            PathBuf::from("/p/.lfmide.toml")
        );
    }

    #[test]
    fn editor_dir_is_private_and_named_for_the_process() {
        use std::os::unix::fs::PermissionsExt;
        let base = tempfile::TempDir::new().unwrap();

        let dir = editor_dir(base.path(), 42).unwrap();

        assert_eq!(dir, base.path().join("lf-mide-42"));
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
    }

    #[test]
    fn editor_dir_reuses_our_own_directory_and_clears_a_stale_socket() {
        let base = tempfile::TempDir::new().unwrap();
        let first = editor_dir(base.path(), 7).unwrap();
        std::fs::write(first.join("nvim.sock"), "stale").unwrap();

        let second = editor_dir(base.path(), 7).unwrap();

        assert_eq!(first, second);
        assert!(!second.join("nvim.sock").exists());
    }

    #[test]
    fn editor_dir_refuses_directories_others_could_use() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let base = tempfile::TempDir::new().unwrap();
        let open_dir = base.path().join("lf-mide-1");
        std::fs::create_dir(&open_dir).unwrap();
        std::fs::set_permissions(&open_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        let elsewhere = base.path().join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        symlink(&elsewhere, base.path().join("lf-mide-2")).unwrap();

        assert!(
            editor_dir(base.path(), 1).is_err(),
            "group/world accessible"
        );
        assert!(editor_dir(base.path(), 2).is_err(), "symlink");
    }

    #[test]
    fn write_launcher_creates_an_executable_private_script() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::TempDir::new().unwrap();

        let path = write_launcher(dir.path(), "#!/bin/sh\necho hi\n").unwrap();

        assert_eq!(path, dir.path().join("open-nvim.sh"));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "#!/bin/sh\necho hi\n"
        );
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let out = std::process::Command::new(&path).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), "hi\n");
    }

    #[test]
    fn write_launcher_replaces_an_earlier_script() {
        let dir = tempfile::TempDir::new().unwrap();
        write_launcher(dir.path(), "old").unwrap();

        let path = write_launcher(dir.path(), "new").unwrap();

        assert_eq!(std::fs::read_to_string(path).unwrap(), "new");
    }

    #[test]
    fn write_launcher_does_not_write_through_a_planted_symlink() {
        let dir = tempfile::TempDir::new().unwrap();
        let victim = dir.path().join("victim.txt");
        std::fs::write(&victim, "precious").unwrap();
        let sub = dir.path().join("private");
        std::fs::create_dir(&sub).unwrap();
        std::os::unix::fs::symlink(&victim, sub.join("open-nvim.sh")).unwrap();

        let path = write_launcher(&sub, "#!/bin/sh\n").unwrap();

        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "precious");
        assert!(
            !std::fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }
}
