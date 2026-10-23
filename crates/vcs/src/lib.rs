//! Git integration for LF-Mide.

use std::path::Path;

use git2::{ErrorCode, Repository, StatusOptions};
use lf_mide_core::BranchInfo;
use thiserror::Error;

const SHORT_ID_LEN: usize = 7;

#[derive(Debug, Error)]
#[error("git error: {0}")]
pub struct VcsError(#[from] git2::Error);

/// Reads the current branch and dirty state of the repository containing
/// `path`. Returns `Ok(None)` when `path` is not inside a git repository.
pub fn branch_info(path: &Path) -> Result<Option<BranchInfo>, VcsError> {
    let repo = match Repository::discover(path) {
        Ok(repo) => repo,
        Err(err) if err.code() == ErrorCode::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    let (name, detached) = head_name(&repo)?;
    let dirty = is_dirty(&repo)?;
    Ok(Some(BranchInfo {
        name,
        detached,
        dirty,
    }))
}

fn head_name(repo: &Repository) -> Result<(String, bool), VcsError> {
    match repo.head() {
        Ok(head) if repo.head_detached()? => {
            let id = head.peel_to_commit()?.id().to_string();
            Ok((id[..SHORT_ID_LEN].to_string(), true))
        }
        Ok(head) => Ok((head.shorthand()?.to_string(), false)),
        // No commits yet: HEAD points at a branch that doesn't exist.
        Err(err) if err.code() == ErrorCode::UnbornBranch => {
            let head = repo.find_reference("HEAD")?;
            let target = head.symbolic_target()?.unwrap_or("HEAD");
            Ok((target.trim_start_matches("refs/heads/").to_string(), false))
        }
        Err(err) => Err(err.into()),
    }
}

fn is_dirty(repo: &Repository) -> Result<bool, VcsError> {
    if repo.is_bare() {
        return Ok(false);
    }
    let mut opts = StatusOptions::new();
    opts.include_untracked(true)
        .include_ignored(false)
        .exclude_submodules(true);
    Ok(!repo.statuses(Some(&mut opts))?.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use git2::{RepositoryInitOptions, Signature};
    use std::fs;
    use tempfile::TempDir;

    fn init_repo() -> (TempDir, Repository) {
        let tmp = TempDir::new().unwrap();
        let mut opts = RepositoryInitOptions::new();
        opts.initial_head("main");
        let repo = Repository::init_opts(tmp.path(), &opts).unwrap();
        (tmp, repo)
    }

    fn commit_file(repo: &Repository, name: &str, content: &str) -> git2::Oid {
        let root = repo.workdir().unwrap().to_path_buf();
        fs::write(root.join(name), content).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new(name)).unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let sig = Signature::now("Test", "test@example.com").unwrap();
        let parents: Vec<_> = repo
            .head()
            .ok()
            .and_then(|h| h.peel_to_commit().ok())
            .into_iter()
            .collect();
        let parent_refs: Vec<_> = parents.iter().collect();
        repo.commit(Some("HEAD"), &sig, &sig, "commit", &tree, &parent_refs)
            .unwrap()
    }

    #[test]
    fn returns_none_outside_repository() {
        let tmp = TempDir::new().unwrap();

        assert_eq!(branch_info(tmp.path()).unwrap(), None);
    }

    #[test]
    fn reads_branch_name_of_clean_repo() {
        let (tmp, repo) = init_repo();
        commit_file(&repo, "a.txt", "a");

        let info = branch_info(tmp.path()).unwrap().unwrap();

        assert_eq!(
            info,
            BranchInfo {
                name: "main".into(),
                detached: false,
                dirty: false
            }
        );
    }

    #[test]
    fn reports_unborn_branch_before_first_commit() {
        let (tmp, _repo) = init_repo();

        let info = branch_info(tmp.path()).unwrap().unwrap();

        assert_eq!(info.name, "main");
        assert!(!info.detached);
    }

    #[test]
    fn marks_dirty_on_modified_or_untracked_files() {
        let (tmp, repo) = init_repo();
        commit_file(&repo, "a.txt", "a");
        fs::write(tmp.path().join("new.txt"), "n").unwrap();

        let info = branch_info(tmp.path()).unwrap().unwrap();

        assert!(info.dirty);
    }

    #[test]
    fn reports_short_commit_id_when_detached() {
        let (tmp, repo) = init_repo();
        let oid = commit_file(&repo, "a.txt", "a");
        repo.set_head_detached(oid).unwrap();

        let info = branch_info(tmp.path()).unwrap().unwrap();

        assert!(info.detached);
        assert_eq!(info.name, oid.to_string()[..SHORT_ID_LEN]);
    }

    #[test]
    fn works_from_subdirectory() {
        let (tmp, repo) = init_repo();
        commit_file(&repo, "a.txt", "a");
        fs::create_dir(tmp.path().join("sub")).unwrap();

        let info = branch_info(&tmp.path().join("sub")).unwrap();

        assert_eq!(info.map(|i| i.name), Some("main".into()));
    }
}
