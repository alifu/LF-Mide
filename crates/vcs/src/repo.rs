//! Finding the repository for a project and keeping paths straight: the
//! project folder may be a subfolder of the repository.

use std::path::{Component, Path, PathBuf};

use git2::{ErrorCode, Repository};
use lf_mide_core::nav::clean_relative;

use crate::VcsError;

/// A repository and where the project sits inside it.
pub struct ProjectRepo {
    pub repo: Repository,
    /// The project folder relative to the repository's working directory
    /// (empty when they are the same).
    prefix: PathBuf,
    /// The repository's working directory, resolved.
    workdir: PathBuf,
}

impl ProjectRepo {
    /// `Ok(None)` when `root` is not inside a repository with a working
    /// directory.
    pub fn open(root: &Path) -> Result<Option<ProjectRepo>, VcsError> {
        let repo = match Repository::discover(root) {
            Ok(repo) => repo,
            Err(err) if err.code() == ErrorCode::NotFound => return Ok(None),
            Err(err) => return Err(err.into()),
        };
        let Some(workdir) = repo.workdir() else {
            return Ok(None);
        };
        let workdir = std::fs::canonicalize(workdir).unwrap_or_else(|_| workdir.to_path_buf());
        let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        // A project that is not under the working directory (a `core.worktree`
        // pointing elsewhere, say) is not described by this repository.
        let Ok(prefix) = root.strip_prefix(&workdir).map(Path::to_path_buf) else {
            return Ok(None);
        };
        Ok(Some(ProjectRepo {
            repo,
            prefix,
            workdir,
        }))
    }

    /// `relative` (to the project) as a path inside the repository, if it is
    /// a plain relative path.
    pub fn in_repo(&self, relative: &Path) -> Option<PathBuf> {
        clean_relative(relative).map(|clean| self.prefix.join(clean))
    }

    /// A repository path as a project-relative one, if it is inside the
    /// project folder.
    pub fn in_project(&self, repo_path: &Path) -> Option<PathBuf> {
        let inside = repo_path.strip_prefix(&self.prefix).ok()?;
        let plain = inside
            .components()
            .all(|c| matches!(c, Component::Normal(_)));
        (plain && !inside.as_os_str().is_empty()).then(|| inside.to_path_buf())
    }

    /// True when git ignores `relative` (a project-relative path): it is
    /// generated, not something to keep. Folders are tested as folders.
    pub fn is_ignored(&self, relative: &Path, is_dir: bool) -> bool {
        let Some(mut path) = self.in_repo(relative) else {
            return false;
        };
        if is_dir {
            path.push("");
        }
        self.repo.is_path_ignored(&path).unwrap_or(false)
    }

    /// True when git tracks any file at or under `relative`. Being matched
    /// by an ignore pattern does not make tracked files disposable. Names
    /// are compared without regard to case: on a case-insensitive disk
    /// `build` and `Build` are the same folder. When the index cannot be
    /// read the answer is the cautious one: yes.
    pub fn tracks_anything_under(&self, relative: &Path) -> bool {
        let Some(path) = self.in_repo(relative) else {
            return true;
        };
        let Ok(index) = self.repo.index() else {
            return true;
        };
        let exact = path.to_string_lossy().to_lowercase();
        let prefix = format!("{exact}/");
        index.iter().any(|entry| {
            let name = String::from_utf8_lossy(&entry.path).to_lowercase();
            name == exact || name.starts_with(&prefix)
        })
    }

    /// True when git tracks the file `relative` itself.
    pub fn tracks_file(&self, relative: &Path) -> bool {
        let Some(path) = self.in_repo(relative) else {
            return false;
        };
        let wanted = path.to_string_lossy().to_lowercase();
        self.repo.index().is_ok_and(|index| {
            index
                .iter()
                .any(|entry| String::from_utf8_lossy(&entry.path).to_lowercase() == wanted)
        })
    }

    /// True when `relative`, or a folder above it inside the repository, is
    /// a different repository: a submodule (a gitlink in the index) or a
    /// nested clone (a `.git` entry). What this repository says about files
    /// in there means nothing. Unreadable answers count as "yes".
    pub fn inside_nested_repo(&self, relative: &Path) -> bool {
        let Some(path) = self.in_repo(relative) else {
            return true;
        };
        let mut current = self.workdir.clone();
        for component in path.components() {
            current.push(component);
            // Present, or not readable: either way it cannot be ruled out.
            match std::fs::symlink_metadata(current.join(".git")) {
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                _ => return true,
            }
        }
        let Ok(index) = self.repo.index() else {
            return true;
        };
        let wanted = path.to_string_lossy().to_lowercase();
        index.iter().any(|entry| {
            const GITLINK: u32 = 0o160_000;
            if entry.mode != GITLINK {
                return false;
            }
            let name = String::from_utf8_lossy(&entry.path).to_lowercase();
            wanted == name || wanted.starts_with(&format!("{name}/"))
        })
    }

    /// The repository's working directory (resolved).
    pub fn workdir(&self) -> &Path {
        &self.workdir
    }

    pub fn prefix(&self) -> &Path {
        &self.prefix
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use std::fs;
    use std::path::Path;

    use git2::{Oid, Repository, RepositoryInitOptions, Signature, Time};
    use tempfile::TempDir;

    pub fn init_repo() -> (TempDir, Repository) {
        let tmp = TempDir::new().unwrap();
        let mut opts = RepositoryInitOptions::new();
        opts.initial_head("main");
        let repo = Repository::init_opts(tmp.path(), &opts).unwrap();
        (tmp, repo)
    }

    /// Writes `files`, commits them as `author` at `when` (unix seconds).
    pub fn commit_as(
        repo: &Repository,
        files: &[(&str, &str)],
        message: &str,
        author: &str,
        when: i64,
    ) -> Oid {
        let root = repo.workdir().unwrap().to_path_buf();
        let mut index = repo.index().unwrap();
        for (name, content) in files {
            let path = root.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, content).unwrap();
            index.add_path(Path::new(name)).unwrap();
        }
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let sig = Signature::new(author, "a@example.com", &Time::new(when, 0)).unwrap();
        let parents: Vec<_> = repo
            .head()
            .ok()
            .and_then(|h| h.peel_to_commit().ok())
            .into_iter()
            .collect();
        let parent_refs: Vec<_> = parents.iter().collect();
        repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parent_refs)
            .unwrap()
    }

    pub fn commit(repo: &Repository, files: &[(&str, &str)], message: &str) -> Oid {
        commit_as(repo, files, message, "Test", 1_700_000_000)
    }

    pub fn write(repo: &Repository, name: &str, content: &str) {
        let path = repo.workdir().unwrap().join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    pub fn read_lines(count: u32) -> String {
        (1..=count).map(|n| format!("line {n}\n")).collect()
    }

    #[allow(dead_code)]
    pub fn path(tmp: &TempDir) -> &Path {
        tmp.path()
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;

    #[test]
    fn a_folder_outside_any_repository_has_none() {
        let tmp = tempfile::TempDir::new().unwrap();

        assert!(ProjectRepo::open(tmp.path()).unwrap().is_none());
    }

    #[test]
    fn the_project_may_be_a_subfolder_of_the_repository() {
        let (tmp, repo) = init_repo();
        commit(&repo, &[("app/a.txt", "a\n")], "init");

        let found = ProjectRepo::open(&tmp.path().join("app")).unwrap().unwrap();

        assert_eq!(found.prefix(), Path::new("app"));
        assert_eq!(
            found.in_repo(Path::new("a.txt")),
            Some(PathBuf::from("app/a.txt"))
        );
        assert_eq!(
            found.in_project(Path::new("app/x/b.txt")),
            Some(PathBuf::from("x/b.txt"))
        );
        assert_eq!(
            found.in_project(Path::new("other/b.txt")),
            None,
            "outside the project"
        );
    }

    #[test]
    fn git_ignored_folders_and_files_are_recognised() {
        let (tmp, repo) = init_repo();
        commit(
            &repo,
            &[(".gitignore", "build/\n*.log\n"), ("src/a.kt", "a\n")],
            "init",
        );
        write(&repo, "build/out.bin", "x");
        write(&repo, "src/build/keep.kt", "tracked-looking");
        let found = ProjectRepo::open(tmp.path()).unwrap().unwrap();

        assert!(found.is_ignored(Path::new("build"), true));
        assert!(
            found.is_ignored(Path::new("src/build"), true),
            "the pattern matches at any depth"
        );
        assert!(found.is_ignored(Path::new("x.log"), false));
        assert!(!found.is_ignored(Path::new("src"), true));
        assert!(!found.is_ignored(Path::new("../outside"), true));
    }

    #[test]
    fn tracked_files_under_a_folder_are_noticed_even_if_a_pattern_ignores_it() {
        let (tmp, repo) = init_repo();
        commit(
            &repo,
            &[("tools/build/run.sh", "x\n"), ("src/a.kt", "a\n")],
            "init",
        );
        write(&repo, ".gitignore", "build/\n");
        write(&repo, "src/build/out.bin", "generated");
        let found = ProjectRepo::open(tmp.path()).unwrap().unwrap();

        assert!(
            found.is_ignored(Path::new("tools/build"), true),
            "the pattern matches"
        );
        assert!(found.tracks_anything_under(Path::new("tools/build")));
        assert!(found.tracks_anything_under(Path::new("tools")));
        assert!(!found.tracks_anything_under(Path::new("src/build")));
        assert!(
            found.tracks_anything_under(Path::new("../x")),
            "unknown paths are not disposable"
        );
    }

    #[test]
    fn folders_that_differ_only_in_case_count_as_tracked() {
        let (tmp, repo) = init_repo();
        commit(&repo, &[("Build/run.sh", "x\n")], "init");
        let found = ProjectRepo::open(tmp.path()).unwrap().unwrap();

        assert!(found.tracks_anything_under(Path::new("build")));
        assert!(found.tracks_anything_under(Path::new("BUILD")));
        assert!(found.tracks_file(Path::new("build/RUN.sh")));
        assert!(
            !found.tracks_file(Path::new("build")),
            "a folder is not a tracked file"
        );
    }

    #[test]
    fn a_nested_clone_or_a_submodule_is_a_different_repository() {
        let (tmp, repo) = init_repo();
        commit(&repo, &[("src/a.kt", "a\n")], "init");
        std::fs::create_dir_all(tmp.path().join("vendor/lib/build")).unwrap();
        std::fs::create_dir_all(tmp.path().join("vendor/lib/.git")).unwrap();
        // A submodule is a gitlink entry in the index.
        let mut index = repo.index().unwrap();
        index
            .add(&git2::IndexEntry {
                ctime: git2::IndexTime::new(0, 0),
                mtime: git2::IndexTime::new(0, 0),
                dev: 0,
                ino: 0,
                mode: 0o160_000,
                uid: 0,
                gid: 0,
                file_size: 0,
                id: git2::Oid::from_str("0123456789012345678901234567890123456789").unwrap(),
                flags: 0,
                flags_extended: 0,
                path: b"mods/sub".to_vec(),
            })
            .unwrap();
        index.write().unwrap();
        let found = ProjectRepo::open(tmp.path()).unwrap().unwrap();

        assert!(found.inside_nested_repo(Path::new("vendor/lib/build")));
        assert!(found.inside_nested_repo(Path::new("vendor/lib")));
        assert!(found.inside_nested_repo(Path::new("mods/sub/build")));
        assert!(!found.inside_nested_repo(Path::new("src/build")));
        assert!(
            !found.inside_nested_repo(Path::new("vendor")),
            "only the clone itself and what is in it"
        );
        assert!(found.inside_nested_repo(Path::new("../x")));
    }

    #[test]
    fn a_project_the_repository_does_not_contain_has_no_repository() {
        let (tmp, repo) = init_repo();
        commit(&repo, &[("a.txt", "a\n")], "init");
        let elsewhere = tempfile::TempDir::new().unwrap();
        let mut config = repo.config().unwrap();
        config
            .set_str("core.worktree", elsewhere.path().to_str().unwrap())
            .unwrap();

        assert!(ProjectRepo::open(tmp.path()).unwrap().is_none());
    }

    #[test]
    fn paths_that_could_escape_are_refused() {
        let (tmp, repo) = init_repo();
        commit(&repo, &[("a.txt", "a\n")], "init");
        let found = ProjectRepo::open(tmp.path()).unwrap().unwrap();

        assert_eq!(found.in_repo(Path::new("../x")), None);
        assert_eq!(found.in_repo(Path::new("/etc/passwd")), None);
        assert_eq!(found.in_repo(Path::new("")), None);
        assert_eq!(found.in_project(Path::new("")), None);
    }
}
