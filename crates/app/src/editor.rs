//! Getting everything ready to open Neovim in a split pane.

use std::path::{Path, PathBuf};

use lf_mide_terminal::{SplitSpec, launcher_script};

use crate::paths;

/// What [`prepare`] made ready.
#[derive(Debug, PartialEq, Eq)]
pub struct Prepared {
    pub spec: SplitSpec,
    /// The script the terminal is told to run.
    pub launcher: PathBuf,
    /// Where Neovim will listen.
    pub socket: PathBuf,
}

/// Creates the private folder and the launcher script for a Neovim session
/// in `project_root`. `base` is the temp directory, `pid` this process.
/// Errors are messages for the status bar.
pub fn prepare(project_root: &Path, base: &Path, pid: u32) -> Result<Prepared, String> {
    let dir = paths::editor_dir(base, pid)
        .map_err(|err| format!("Cannot prepare the Neovim folder: {err}"))?;
    let socket = dir.join("nvim.sock");
    let spec = SplitSpec::neovim(project_root, &socket)
        .map_err(|err| format!("Cannot open Neovim here: {err}"))?;
    let script = launcher_script(&spec).map_err(|err| format!("Cannot open Neovim here: {err}"))?;
    let launcher = paths::write_launcher(&dir, &script)
        .map_err(|err| format!("Cannot write the Neovim launcher: {err}"))?;
    Ok(Prepared {
        spec,
        launcher,
        socket,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn prepares_a_runnable_launcher_beside_the_socket() {
        let base = tempfile::TempDir::new().unwrap();
        let project = tempfile::TempDir::new().unwrap();

        let prepared = prepare(project.path(), base.path(), 5).unwrap();

        assert_eq!(prepared.socket, base.path().join("lf-mide-5/nvim.sock"));
        assert_eq!(
            prepared.launcher,
            base.path().join("lf-mide-5/open-nvim.sh")
        );
        assert_eq!(prepared.spec.cwd, project.path());
        let script = std::fs::read_to_string(&prepared.launcher).unwrap();
        assert!(script.contains("exec 'nvim' '--listen'"));
    }

    #[test]
    fn the_launcher_enters_a_hostile_project_folder() {
        let base = tempfile::TempDir::new().unwrap();
        let root = tempfile::TempDir::new().unwrap();
        let project = root.path().join("x\\'; touch pwned; #\u{15}\r");
        std::fs::create_dir(&project).unwrap();
        // Swap in `pwd` for nvim so the test can see where the script ran.
        let mut prepared = prepare(&project, base.path(), 6).unwrap();
        prepared.spec.program = "pwd".into();
        prepared.spec.args.clear();
        let script = lf_mide_terminal::launcher_script(&prepared.spec).unwrap();
        paths::write_launcher(prepared.launcher.parent().unwrap(), &script).unwrap();

        let out = Command::new(&prepared.launcher).output().unwrap();

        let printed = String::from_utf8_lossy(&out.stdout)
            .trim_end_matches('\n')
            .to_string();
        assert_eq!(
            std::fs::canonicalize(printed).unwrap(),
            std::fs::canonicalize(&project).unwrap()
        );
        assert!(!project.join("pwned").exists());
    }

    #[test]
    fn reports_a_folder_that_cannot_be_created() {
        let base = tempfile::TempDir::new().unwrap();
        let blocker = base.path().join("file");
        std::fs::write(&blocker, "").unwrap();

        let err = prepare(Path::new("/"), &blocker, 1).unwrap_err();

        assert!(err.contains("Neovim"), "{err}");
    }
}
