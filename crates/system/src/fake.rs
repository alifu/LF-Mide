//! In-memory [`System`] for tests.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use crate::{Output, RunError, System};

#[derive(Default)]
pub struct FakeSystem {
    commands: HashMap<String, Result<Output, RunError>>,
    env: HashMap<String, String>,
    files: BTreeMap<PathBuf, String>,
    dirs: BTreeMap<PathBuf, Vec<String>>,
    home: Option<PathBuf>,
    /// (link, target): any path under `link` canonicalizes under `target`.
    symlinks: Vec<(PathBuf, PathBuf)>,
}

fn key(program: &str, args: &[&str]) -> String {
    format!("{program} {}", args.join(" ")).trim().to_string()
}

impl FakeSystem {
    pub fn new() -> Self {
        Self::default()
    }

    /// A command that succeeds with `stdout`. `cmd` is `"program arg arg"`.
    pub fn ok(mut self, cmd: &str, stdout: &str) -> Self {
        let output = Output {
            success: true,
            stdout: stdout.into(),
            stderr: String::new(),
        };
        self.commands.insert(cmd.into(), Ok(output));
        self
    }

    /// A command that succeeds but prints to stderr (like `java -version`).
    pub fn ok_stderr(mut self, cmd: &str, stderr: &str) -> Self {
        let output = Output {
            success: true,
            stdout: String::new(),
            stderr: stderr.into(),
        };
        self.commands.insert(cmd.into(), Ok(output));
        self
    }

    /// A command that runs but exits non-zero.
    pub fn exits_nonzero(mut self, cmd: &str) -> Self {
        let output = Output {
            success: false,
            stdout: String::new(),
            stderr: String::new(),
        };
        self.commands.insert(cmd.into(), Ok(output));
        self
    }

    /// A command that runs, exits non-zero and prints `stderr`.
    pub fn fails(mut self, cmd: &str, stderr: &str) -> Self {
        let output = Output {
            success: false,
            stdout: String::new(),
            stderr: stderr.into(),
        };
        self.commands.insert(cmd.into(), Ok(output));
        self
    }

    pub fn run_error(mut self, cmd: &str, error: RunError) -> Self {
        self.commands.insert(cmd.into(), Err(error));
        self
    }

    pub fn timeout(mut self, cmd: &str) -> Self {
        self.commands.insert(cmd.into(), Err(RunError::TimedOut));
        self
    }

    pub fn env(mut self, key: &str, value: &str) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    pub fn home(mut self, home: &str) -> Self {
        self.home = Some(PathBuf::from(home));
        self
    }

    pub fn file(mut self, path: &str, content: &str) -> Self {
        self.files.insert(PathBuf::from(path), content.into());
        self
    }

    pub fn symlink(mut self, link: &str, target: &str) -> Self {
        self.symlinks
            .push((PathBuf::from(link), PathBuf::from(target)));
        self
    }

    pub fn dir(mut self, path: &str, entries: &[&str]) -> Self {
        let names = entries.iter().map(|s| s.to_string()).collect();
        self.dirs.insert(PathBuf::from(path), names);
        self
    }
}

impl System for FakeSystem {
    fn run(&self, program: &str, args: &[&str]) -> Result<Output, RunError> {
        self.commands
            .get(&key(program, args))
            .cloned()
            .unwrap_or(Err(RunError::NotFound))
    }

    fn env(&self, key: &str) -> Option<String> {
        self.env.get(key).cloned()
    }

    fn exists(&self, path: &Path) -> bool {
        self.files.contains_key(path) || self.dirs.contains_key(path)
    }

    fn is_dir(&self, path: &Path) -> bool {
        self.dirs.contains_key(path)
    }

    fn list_dir(&self, path: &Path) -> Vec<String> {
        let mut names = self.dirs.get(path).cloned().unwrap_or_default();
        names.sort();
        names
    }

    fn read_to_string(&self, path: &Path) -> Option<String> {
        self.files.get(path).cloned()
    }

    fn canonicalize(&self, path: &Path) -> Option<PathBuf> {
        let resolved = self
            .symlinks
            .iter()
            .find_map(|(link, target)| path.strip_prefix(link).ok().map(|rest| target.join(rest)))
            .unwrap_or_else(|| path.to_path_buf());
        // Like a real filesystem, the parents of known files exist too.
        let known = self
            .files
            .keys()
            .chain(self.dirs.keys())
            .any(|known| known.starts_with(&resolved));
        known.then_some(resolved)
    }

    fn home(&self) -> Option<PathBuf> {
        self.home.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn which_searches_path_in_order() {
        let sys = FakeSystem::new()
            .env("PATH", "/a/bin:/b/bin")
            .file("/b/bin/nvim", "")
            .file("/a/bin/other", "");

        assert_eq!(sys.which("nvim"), Some(PathBuf::from("/b/bin/nvim")));
        assert_eq!(sys.which("missing"), None);
    }

    #[test]
    fn which_ignores_relative_path_entries() {
        let only_relative = FakeSystem::new()
            .env("PATH", ".::bin")
            .file("./java", "")
            .file("bin/java", "");
        let both = FakeSystem::new()
            .env("PATH", ".:/opt/bin")
            .file("./java", "")
            .file("/opt/bin/java", "");

        assert_eq!(only_relative.which("java"), None);
        assert_eq!(both.which("java"), Some(PathBuf::from("/opt/bin/java")));
    }

    #[test]
    fn unknown_commands_are_not_found() {
        assert_eq!(
            FakeSystem::new().run("nvim", &["--version"]),
            Err(RunError::NotFound)
        );
    }
}
