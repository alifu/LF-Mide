//! Who last changed each line of a file.

use std::collections::HashMap;
use std::path::Path;

use git2::{BlameOptions, Oid};
use lf_mide_core::git::{BlameLine, clean_text};

use crate::VcsError;
use crate::diff::short;
use crate::repo::ProjectRepo;

/// Blame is skipped for files with more lines than this.
pub const MAX_BLAME_LINES: usize = 20_000;
/// Files larger than this (bytes) are not read for blame.
const MAX_BLAME_BYTES: u64 = 4 * 1024 * 1024;

/// Per-line blame of the working copy of `relative`: lines that are
/// committed name their commit, lines edited since do not. `Ok(None)` for a
/// file git does not know (new, untracked).
pub fn blame_file(
    project: &ProjectRepo,
    relative: &Path,
    workdir_file: &Path,
) -> Result<Option<Vec<BlameLine>>, VcsError> {
    let path = project
        .in_repo(relative)
        .ok_or_else(|| git2::Error::from_str("not a path inside the project"))?;
    // A tracked symlink points somewhere else; reading it would read outside
    // the project. Git blames the link itself, which says nothing useful.
    let meta = std::fs::symlink_metadata(workdir_file)
        .map_err(|e| git2::Error::from_str(&e.to_string()))?;
    if meta.file_type().is_symlink() {
        return Ok(None);
    }
    if !meta.is_file() || meta.len() > MAX_BLAME_BYTES {
        return Err(git2::Error::from_str("file is too large to blame").into());
    }
    let contents =
        std::fs::read(workdir_file).map_err(|e| git2::Error::from_str(&e.to_string()))?;
    let line_count = contents.iter().filter(|b| **b == b'\n').count()
        + usize::from(contents.last().is_some_and(|b| *b != b'\n'));
    if line_count > MAX_BLAME_LINES {
        return Err(git2::Error::from_str("file has too many lines to blame").into());
    }
    if project.repo.head().is_err() {
        return Ok(None); // no commits yet: nothing to blame
    }
    let committed = match project
        .repo
        .blame_file(&path, Some(&mut BlameOptions::new()))
    {
        Ok(blame) => blame,
        Err(err)
            if matches!(
                err.code(),
                git2::ErrorCode::NotFound | git2::ErrorCode::UnbornBranch
            ) =>
        {
            return Ok(None);
        }
        Err(err) => return Err(err.into()),
    };
    // Re-blame the text as it is on disk now, so edits made since the last
    // commit show up as "not committed yet" at the right lines.
    let blame = committed.blame_buffer(&contents)?;
    let mut summaries: HashMap<Oid, String> = HashMap::new();
    let mut lines = Vec::with_capacity(line_count);
    for hunk in blame.iter() {
        let id = hunk.final_commit_id();
        let signature = hunk.final_signature();
        let (commit, author, summary) = if id.is_zero() {
            (None, "You".to_string(), String::new())
        } else {
            let summary = summaries
                .entry(id)
                .or_insert_with(|| {
                    project
                        .repo
                        .find_commit(id)
                        .ok()
                        .and_then(|c| c.summary().ok().flatten().map(str::to_string))
                        .unwrap_or_default()
                })
                .clone();
            (
                Some(short(id)),
                signature
                    .as_ref()
                    .and_then(|s| s.name().ok())
                    .unwrap_or("unknown")
                    .to_string(),
                summary,
            )
        };
        let first = u32::try_from(hunk.final_start_line()).unwrap_or(u32::MAX);
        for offset in 0..u32::try_from(hunk.lines_in_hunk()).unwrap_or(0) {
            lines.push(BlameLine {
                line: first.saturating_add(offset),
                commit: commit.clone(),
                author: clean_text(&author),
                time: signature.as_ref().map_or(0, |s| s.when().seconds()),
                summary: clean_text(&summary),
            });
        }
    }
    Ok(Some(lines))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::testing::*;

    fn blame(tmp: &tempfile::TempDir, name: &str) -> Option<Vec<BlameLine>> {
        let project = ProjectRepo::open(tmp.path()).unwrap().unwrap();
        blame_file(&project, Path::new(name), &tmp.path().join(name)).unwrap()
    }

    #[test]
    fn each_line_names_the_commit_that_last_changed_it() {
        let (tmp, repo) = init_repo();
        commit_as(
            &repo,
            &[("a.txt", "one\ntwo\nthree\n")],
            "first",
            "Ada",
            1_000,
        );
        commit_as(
            &repo,
            &[("a.txt", "one\nTWO\nthree\n")],
            "shout two",
            "Bob",
            2_000,
        );

        let lines = blame(&tmp, "a.txt").unwrap();

        let who: Vec<(&str, i64)> = lines.iter().map(|l| (l.author.as_str(), l.time)).collect();
        assert_eq!(who, [("Ada", 1_000), ("Bob", 2_000), ("Ada", 1_000)]);
        assert_eq!(lines.iter().map(|l| l.line).collect::<Vec<_>>(), [1, 2, 3]);
        assert_eq!(lines[1].summary, "shout two");
        assert_eq!(lines[1].commit.as_deref().map(str::len), Some(7));
    }

    #[test]
    fn lines_edited_since_the_last_commit_are_not_committed_yet() {
        let (tmp, repo) = init_repo();
        commit_as(
            &repo,
            &[("a.txt", "one\ntwo\nthree\n")],
            "first",
            "Ada",
            1_000,
        );
        write(&repo, "a.txt", "one\nnew top\ntwo\nthree\nand more\n");

        let lines = blame(&tmp, "a.txt").unwrap();

        let committed: Vec<bool> = lines.iter().map(|l| l.commit.is_some()).collect();
        assert_eq!(committed, [true, false, true, true, false]);
        assert_eq!(lines[1].author, "You");
        assert_eq!(lines.len(), 5);
    }

    #[test]
    fn a_file_git_does_not_know_has_no_blame() {
        let (tmp, repo) = init_repo();
        commit(&repo, &[("a.txt", "a\n")], "init");
        write(&repo, "new.txt", "x\n");

        assert_eq!(blame(&tmp, "new.txt"), None);
    }

    #[test]
    fn blame_in_a_repository_without_commits_is_none() {
        let (tmp, repo) = init_repo();
        write(&repo, "a.txt", "x\n");

        assert_eq!(blame(&tmp, "a.txt"), None);
    }

    #[test]
    fn a_symlink_is_never_followed_for_blame() {
        let (tmp, repo) = init_repo();
        commit(&repo, &[("a.txt", "a\n")], "init");
        let outside = tempfile::TempDir::new().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "top\nsecret\n").unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("secret.txt"),
            tmp.path().join("link.txt"),
        )
        .unwrap();

        assert_eq!(blame(&tmp, "link.txt"), None);
    }

    #[test]
    fn a_file_without_a_final_newline_counts_its_last_line() {
        let (tmp, repo) = init_repo();
        commit(&repo, &[("a.txt", "one\ntwo")], "init");

        assert_eq!(blame(&tmp, "a.txt").unwrap().len(), 2);
    }

    #[test]
    fn huge_files_and_escaping_paths_are_refused() {
        let (tmp, repo) = init_repo();
        commit(
            &repo,
            &[("big.txt", &read_lines(MAX_BLAME_LINES as u32 + 10))],
            "init",
        );
        let project = ProjectRepo::open(tmp.path()).unwrap().unwrap();

        let big = blame_file(&project, Path::new("big.txt"), &tmp.path().join("big.txt"));
        let escape = blame_file(&project, Path::new("../x"), &tmp.path().join("big.txt"));

        assert!(big.is_err() && escape.is_err());
    }

    #[test]
    fn author_names_with_control_characters_are_cleaned() {
        let (tmp, repo) = init_repo();
        commit_as(&repo, &[("a.txt", "x\n")], "init", "Eve\u{1b}[31m", 1_000);

        let lines = blame(&tmp, "a.txt").unwrap();

        assert!(!lines[0].author.chars().any(char::is_control));
    }
}
