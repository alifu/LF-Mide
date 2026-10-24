//! Which files differ from HEAD.

use std::path::PathBuf;
use std::time::UNIX_EPOCH;

use git2::{Status, StatusOptions};
use lf_mide_core::git::{ChangeKind, FileChange, MAX_CHANGES};

use crate::VcsError;
use crate::repo::ProjectRepo;

const INDEX_CHANGED: Status = Status::from_bits_truncate(
    Status::INDEX_NEW.bits()
        | Status::INDEX_MODIFIED.bits()
        | Status::INDEX_DELETED.bits()
        | Status::INDEX_RENAMED.bits()
        | Status::INDEX_TYPECHANGE.bits(),
);

/// The changed files of the project, sorted by path, and the id of HEAD.
pub fn changes(project: &ProjectRepo) -> Result<(Option<String>, Vec<FileChange>), VcsError> {
    let head = project
        .repo
        .head()
        .ok()
        .and_then(|head| head.target())
        .map(|id| id.to_string());
    let mut options = StatusOptions::new();
    options
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .include_ignored(false)
        .exclude_submodules(true);
    if !project.prefix().as_os_str().is_empty() {
        // A folder called `apps/[team]` is that folder, not a pattern.
        options
            .pathspec(project.prefix())
            .disable_pathspec_match(true);
    }
    let statuses = project.repo.statuses(Some(&mut options))?;
    let workdir = project
        .repo
        .workdir()
        .map(PathBuf::from)
        .unwrap_or_default();
    let mut found = Vec::new();
    for entry in statuses.iter() {
        let Ok(path) = entry.path() else { continue };
        let Some(relative) = project.in_project(std::path::Path::new(path)) else {
            continue;
        };
        let Some(kind) = kind_of(entry.status()) else {
            continue;
        };
        found.push(FileChange {
            stamp: stamp_of(&workdir.join(path)),
            path: relative,
            kind,
            staged: entry.status().intersects(INDEX_CHANGED),
        });
        if found.len() >= MAX_CHANGES {
            break;
        }
    }
    found.sort_by(|a, b| a.path.cmp(&b.path));
    Ok((head, found))
}

fn kind_of(status: Status) -> Option<ChangeKind> {
    if status.contains(Status::CONFLICTED) {
        return Some(ChangeKind::Conflicted);
    }
    if status.intersects(Status::WT_NEW) && !status.intersects(INDEX_CHANGED) {
        return Some(ChangeKind::Untracked);
    }
    if status.intersects(Status::INDEX_NEW) {
        return Some(ChangeKind::Added);
    }
    if status.intersects(Status::WT_DELETED | Status::INDEX_DELETED) {
        return Some(ChangeKind::Deleted);
    }
    if status.intersects(Status::WT_RENAMED | Status::INDEX_RENAMED) {
        return Some(ChangeKind::Renamed);
    }
    if status.intersects(
        Status::WT_MODIFIED
            | Status::INDEX_MODIFIED
            | Status::WT_TYPECHANGE
            | Status::INDEX_TYPECHANGE,
    ) {
        return Some(ChangeKind::Modified);
    }
    None
}

/// Size and modification time folded into one number; 0 if unreadable.
fn stamp_of(path: &std::path::Path) -> u64 {
    let Ok(meta) = std::fs::metadata(path) else {
        return 0;
    };
    let nanos = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos() as u64);
    nanos.rotate_left(17) ^ meta.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::testing::*;
    use std::path::Path;

    fn changed(root: &Path) -> Vec<(String, ChangeKind, bool)> {
        let project = ProjectRepo::open(root).unwrap().unwrap();
        changes(&project)
            .unwrap()
            .1
            .into_iter()
            .map(|c| (c.path.display().to_string(), c.kind, c.staged))
            .collect()
    }

    #[test]
    fn a_clean_repository_has_no_changes_but_a_head() {
        let (tmp, repo) = init_repo();
        let id = commit(&repo, &[("a.txt", "a\n")], "init");
        let project = ProjectRepo::open(tmp.path()).unwrap().unwrap();

        let (head, list) = changes(&project).unwrap();

        assert!(list.is_empty());
        assert_eq!(head, Some(id.to_string()));
    }

    #[test]
    fn every_kind_of_change_is_listed_sorted_by_path() {
        let (tmp, repo) = init_repo();
        commit(
            &repo,
            &[
                ("keep.txt", "k\n"),
                ("edit.txt", "e\n"),
                ("gone.txt", "g\n"),
            ],
            "init",
        );
        write(&repo, "edit.txt", "changed\n");
        std::fs::remove_file(tmp.path().join("gone.txt")).unwrap();
        write(&repo, "new.txt", "n\n");
        write(&repo, "dir/deep/other.txt", "o\n");

        let list = changed(tmp.path());

        assert_eq!(
            list,
            [
                (
                    "dir/deep/other.txt".to_string(),
                    ChangeKind::Untracked,
                    false
                ),
                ("edit.txt".to_string(), ChangeKind::Modified, false),
                ("gone.txt".to_string(), ChangeKind::Deleted, false),
                ("new.txt".to_string(), ChangeKind::Untracked, false),
            ]
        );
    }

    #[test]
    fn staged_files_are_marked_staged() {
        let (tmp, repo) = init_repo();
        commit(&repo, &[("a.txt", "a\n")], "init");
        write(&repo, "a.txt", "changed\n");
        write(&repo, "b.txt", "b\n");
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("a.txt")).unwrap();
        index.add_path(Path::new("b.txt")).unwrap();
        index.write().unwrap();

        let list = changed(tmp.path());

        assert_eq!(
            list,
            [
                ("a.txt".to_string(), ChangeKind::Modified, true),
                ("b.txt".to_string(), ChangeKind::Added, true),
            ]
        );
    }

    #[test]
    fn ignored_files_are_left_out() {
        let (tmp, repo) = init_repo();
        commit(
            &repo,
            &[(".gitignore", "build/\n"), ("a.txt", "a\n")],
            "init",
        );
        write(&repo, "build/out.txt", "x\n");

        assert!(changed(tmp.path()).is_empty());
    }

    #[test]
    fn a_project_folder_sees_only_its_own_changes_with_project_relative_paths() {
        let (tmp, repo) = init_repo();
        commit(
            &repo,
            &[("app/a.txt", "a\n"), ("other/b.txt", "b\n")],
            "init",
        );
        write(&repo, "app/a.txt", "changed\n");
        write(&repo, "other/b.txt", "changed\n");

        let list = changed(&tmp.path().join("app"));

        assert_eq!(list, [("a.txt".to_string(), ChangeKind::Modified, false)]);
    }

    #[test]
    fn a_project_folder_with_pattern_characters_is_taken_literally() {
        let (tmp, repo) = init_repo();
        commit(
            &repo,
            &[("apps/[team]/a.txt", "a\n"), ("apps/t/b.txt", "b\n")],
            "init",
        );
        write(&repo, "apps/[team]/a.txt", "changed\n");
        write(&repo, "apps/t/b.txt", "changed\n");

        let list = changed(&tmp.path().join("apps/[team]"));

        assert_eq!(list, [("a.txt".to_string(), ChangeKind::Modified, false)]);
    }

    #[test]
    fn editing_a_changed_file_again_changes_its_stamp() {
        let (tmp, repo) = init_repo();
        commit(&repo, &[("a.txt", "a\n")], "init");
        write(&repo, "a.txt", "one\n");
        let project = ProjectRepo::open(tmp.path()).unwrap().unwrap();
        let before = changes(&project).unwrap().1[0].stamp;

        write(&repo, "a.txt", "one and more\n");
        let after = changes(&project).unwrap().1[0].stamp;

        assert_ne!(before, after);
    }

    #[test]
    fn a_repository_without_commits_lists_everything_as_untracked() {
        let (tmp, repo) = init_repo();
        write(&repo, "a.txt", "a\n");

        let project = ProjectRepo::open(tmp.path()).unwrap().unwrap();
        let (head, list) = changes(&project).unwrap();

        assert_eq!(head, None);
        assert_eq!(list.len(), 1);
    }
}
