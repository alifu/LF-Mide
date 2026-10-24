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
        let prefix = root
            .strip_prefix(&workdir)
            .map(Path::to_path_buf)
            .unwrap_or_default();
        Ok(Some(ProjectRepo { repo, prefix }))
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
