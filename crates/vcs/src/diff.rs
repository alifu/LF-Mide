//! A file's differences from a base version.

use std::path::Path;

use git2::{Diff, DiffOptions, Oid, Patch, Tree};
use lf_mide_core::git::{
    BaseKind, BaseRef, DiffLine, FileDiff, Hunk, LineOrigin, MAX_DIFF_LINES, clean_text,
};

use crate::VcsError;
use crate::repo::ProjectRepo;

const CONTEXT_LINES: u32 = 3;
/// Files larger than this are not compared line by line (git treats them
/// like binary files), so one huge file cannot exhaust memory.
const MAX_DIFF_FILE_BYTES: i64 = 2 * 1024 * 1024;
const RECENT_COMMITS: usize = 15;
const MAX_BRANCHES: usize = 30;

/// The working copy of `relative` against `base`. A file that is new has
/// every line added; a deleted one has every line removed.
pub fn diff_file(
    project: &ProjectRepo,
    relative: &Path,
    base: &BaseRef,
) -> Result<FileDiff, VcsError> {
    let path = project
        .in_repo(relative)
        .ok_or_else(|| git2::Error::from_str("not a path inside the project"))?;
    let tree = base_tree(project, base)?;
    let mut options = DiffOptions::new();
    options
        .pathspec(&path)
        // The name is a file name, not a pattern: `a[1].kt` means itself.
        .disable_pathspec_match(true)
        .include_untracked(true)
        .show_untracked_content(true)
        .recurse_untracked_dirs(true)
        .context_lines(CONTEXT_LINES)
        .max_size(MAX_DIFF_FILE_BYTES);
    let diff = project
        .repo
        .diff_tree_to_workdir_with_index(tree.as_ref(), Some(&mut options))?;
    read_diff(&diff)
}

fn base_tree<'a>(project: &'a ProjectRepo, base: &BaseRef) -> Result<Option<Tree<'a>>, VcsError> {
    if base.spec.starts_with('-') {
        return Err(git2::Error::from_str("not a usable revision").into());
    }
    match project.repo.revparse_single(&base.spec) {
        Ok(object) => Ok(Some(object.peel_to_tree()?)),
        // A repository without commits: everything is new.
        Err(err) if base.kind == BaseKind::Head && err.code() == git2::ErrorCode::UnbornBranch => {
            Ok(None)
        }
        Err(err) if base.kind == BaseKind::Head && err.code() == git2::ErrorCode::NotFound => {
            Ok(None)
        }
        Err(err) => Err(err.into()),
    }
}

fn read_diff(diff: &Diff<'_>) -> Result<FileDiff, VcsError> {
    let mut result = FileDiff::default();
    let mut total = 0usize;
    for index in 0..diff.deltas().len() {
        let Some(patch) = Patch::from_diff(diff, index)? else {
            continue;
        };
        if patch.delta().flags().is_binary() {
            result.binary = true;
            continue;
        }
        for hunk_index in 0..patch.num_hunks() {
            let (header, line_count) = patch.hunk(hunk_index)?;
            let mut hunk = Hunk {
                old_start: header.old_start(),
                old_lines: header.old_lines(),
                new_start: header.new_start(),
                new_lines: header.new_lines(),
                lines: Vec::new(),
            };
            for line_index in 0..line_count {
                if total >= MAX_DIFF_LINES {
                    result.truncated = true;
                    break;
                }
                let line = patch.line_in_hunk(hunk_index, line_index)?;
                let Some(origin) = origin_of(line.origin()) else {
                    continue;
                };
                total += 1;
                hunk.lines.push(DiffLine {
                    origin,
                    text: clean_text(
                        String::from_utf8_lossy(line.content()).trim_end_matches(['\n', '\r']),
                    ),
                    old_no: line.old_lineno(),
                    new_no: line.new_lineno(),
                });
            }
            if !hunk.lines.is_empty() {
                result.hunks.push(hunk);
            }
            if result.truncated {
                return Ok(result);
            }
        }
    }
    Ok(result)
}

fn origin_of(origin: char) -> Option<LineOrigin> {
    match origin {
        ' ' => Some(LineOrigin::Context),
        '+' => Some(LineOrigin::Added),
        '-' => Some(LineOrigin::Removed),
        _ => None,
    }
}

/// What a diff can be taken against: HEAD, local branches, recent commits.
pub fn bases(project: &ProjectRepo) -> Result<Vec<BaseRef>, VcsError> {
    let mut found = vec![BaseRef::head()];
    let mut branches = Vec::new();
    for branch in project.repo.branches(Some(git2::BranchType::Local))? {
        let (branch, _) = branch?;
        if let Some(name) = branch.name()? {
            branches.push(name.to_string());
        }
    }
    branches.sort();
    found.extend(branches.into_iter().take(MAX_BRANCHES).map(|name| BaseRef {
        kind: BaseKind::Branch,
        label: clean_text(&name),
        spec: format!("refs/heads/{name}"),
    }));
    found.extend(recent_commits(project)?);
    Ok(found)
}

fn recent_commits(project: &ProjectRepo) -> Result<Vec<BaseRef>, VcsError> {
    let mut walk = project.repo.revwalk()?;
    if walk.push_head().is_err() {
        return Ok(Vec::new()); // no commits yet
    }
    let mut commits = Vec::new();
    for id in walk.take(RECENT_COMMITS) {
        let commit = project.repo.find_commit(id?)?;
        commits.push(BaseRef {
            kind: BaseKind::Commit,
            label: clean_text(&format!(
                "{} {}",
                short(commit.id()),
                commit.summary().ok().flatten().unwrap_or("")
            )),
            spec: commit.id().to_string(),
        });
    }
    Ok(commits)
}

pub(crate) fn short(id: Oid) -> String {
    id.to_string().chars().take(7).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::testing::*;
    use lf_mide_core::git::{hunk_lines, marks_from_diff};

    fn open(tmp: &tempfile::TempDir) -> ProjectRepo {
        ProjectRepo::open(tmp.path()).unwrap().unwrap()
    }

    fn texts(diff: &FileDiff, origin: LineOrigin) -> Vec<String> {
        diff.hunks
            .iter()
            .flat_map(|h| &h.lines)
            .filter(|l| l.origin == origin)
            .map(|l| l.text.clone())
            .collect()
    }

    #[test]
    fn an_unchanged_file_has_an_empty_diff() {
        let (tmp, repo) = init_repo();
        commit(&repo, &[("a.txt", &read_lines(5))], "init");

        let diff = diff_file(&open(&tmp), Path::new("a.txt"), &BaseRef::head()).unwrap();

        assert!(diff.is_empty());
    }

    #[test]
    fn edits_show_removed_and_added_lines_with_numbers() {
        let (tmp, repo) = init_repo();
        commit(&repo, &[("a.txt", &read_lines(10))], "init");
        write(
            &repo,
            "a.txt",
            &read_lines(10).replace("line 5\n", "FIVE\nextra\n"),
        );

        let diff = diff_file(&open(&tmp), Path::new("a.txt"), &BaseRef::head()).unwrap();

        assert_eq!(diff.hunks.len(), 1);
        assert_eq!(texts(&diff, LineOrigin::Removed), ["line 5"]);
        assert_eq!(texts(&diff, LineOrigin::Added), ["FIVE", "extra"]);
        let removed = diff.hunks[0]
            .lines
            .iter()
            .find(|l| l.origin == LineOrigin::Removed)
            .unwrap();
        let added = diff.hunks[0]
            .lines
            .iter()
            .find(|l| l.origin == LineOrigin::Added)
            .unwrap();
        assert_eq!((removed.old_no, removed.new_no), (Some(5), None));
        assert_eq!((added.old_no, added.new_no), (None, Some(5)));
        assert_eq!(hunk_lines(&diff), [5]);
    }

    #[test]
    fn distant_edits_make_separate_hunks_and_the_gutter_marks_follow() {
        let (tmp, repo) = init_repo();
        commit(&repo, &[("a.txt", &read_lines(40))], "init");
        let edited = read_lines(40)
            .replace("line 3\n", "THREE\n")
            .replace("line 30\n", "");
        write(&repo, "a.txt", &edited);

        let diff = diff_file(&open(&tmp), Path::new("a.txt"), &BaseRef::head()).unwrap();
        let marks: Vec<_> = marks_from_diff(&diff)
            .into_iter()
            .map(|m| (m.line, format!("{:?}", m.kind)))
            .collect();

        assert_eq!(diff.hunks.len(), 2);
        assert_eq!(
            marks,
            [(3, "Changed".to_string()), (29, "DeletedBelow".to_string())]
        );
    }

    #[test]
    fn a_new_untracked_file_is_all_added() {
        let (tmp, repo) = init_repo();
        commit(&repo, &[("a.txt", "a\n")], "init");
        write(&repo, "new.kt", "one\ntwo\n");

        let diff = diff_file(&open(&tmp), Path::new("new.kt"), &BaseRef::head()).unwrap();

        assert_eq!(texts(&diff, LineOrigin::Added), ["one", "two"]);
        assert!(texts(&diff, LineOrigin::Removed).is_empty());
    }

    #[test]
    fn a_deleted_file_is_all_removed() {
        let (tmp, repo) = init_repo();
        commit(&repo, &[("a.txt", "a\nb\n")], "init");
        std::fs::remove_file(tmp.path().join("a.txt")).unwrap();

        let diff = diff_file(&open(&tmp), Path::new("a.txt"), &BaseRef::head()).unwrap();

        assert_eq!(texts(&diff, LineOrigin::Removed), ["a", "b"]);
    }

    #[test]
    fn a_name_with_pattern_characters_means_exactly_that_file() {
        let (tmp, repo) = init_repo();
        commit(&repo, &[("a[1].kt", "x\n"), ("a1.kt", "y\n")], "init");
        write(&repo, "a[1].kt", "changed\n");
        write(&repo, "a1.kt", "also changed\n");

        let diff = diff_file(&open(&tmp), Path::new("a[1].kt"), &BaseRef::head()).unwrap();

        assert_eq!(
            texts(&diff, LineOrigin::Added),
            ["changed"],
            "only that file"
        );
    }

    #[test]
    fn a_binary_file_is_reported_not_dumped() {
        let (tmp, repo) = init_repo();
        std::fs::write(tmp.path().join("img.bin"), [0u8, 159, 146, 150, 0, 1]).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("img.bin")).unwrap();
        index.write().unwrap();
        commit(&repo, &[("a.txt", "a\n")], "init");
        std::fs::write(tmp.path().join("img.bin"), [0u8, 1, 2, 3, 0, 9]).unwrap();

        let diff = diff_file(&open(&tmp), Path::new("img.bin"), &BaseRef::head()).unwrap();

        assert!(diff.binary && diff.hunks.is_empty());
    }

    #[test]
    fn a_file_too_big_to_compare_is_reported_like_a_binary_one() {
        let (tmp, repo) = init_repo();
        commit(&repo, &[("a.txt", "a\n")], "init");
        write(&repo, "huge.txt", &"x".repeat(3 * 1024 * 1024));

        let diff = diff_file(&open(&tmp), Path::new("huge.txt"), &BaseRef::head()).unwrap();

        assert!(diff.binary && diff.hunks.is_empty());
    }

    #[test]
    fn a_diff_cut_exactly_at_a_hunk_boundary_has_no_empty_hunk() {
        let (tmp, repo) = init_repo();
        commit(&repo, &[("a.txt", "a\n")], "init");
        write(&repo, "big.txt", &read_lines(MAX_DIFF_LINES as u32));
        write(&repo, "big.txt", &read_lines(MAX_DIFF_LINES as u32 + 1));

        let diff = diff_file(&open(&tmp), Path::new("big.txt"), &BaseRef::head()).unwrap();

        assert!(diff.hunks.iter().all(|h| !h.lines.is_empty()));
    }

    #[test]
    fn a_huge_diff_is_cut_and_says_so() {
        let (tmp, repo) = init_repo();
        commit(&repo, &[("a.txt", "a\n")], "init");
        write(&repo, "big.txt", &read_lines(MAX_DIFF_LINES as u32 + 500));

        let diff = diff_file(&open(&tmp), Path::new("big.txt"), &BaseRef::head()).unwrap();

        assert!(diff.truncated);
        assert_eq!(
            diff.hunks.iter().map(|h| h.lines.len()).sum::<usize>(),
            MAX_DIFF_LINES
        );
    }

    #[test]
    fn a_repository_without_commits_diffs_against_nothing() {
        let (tmp, repo) = init_repo();
        write(&repo, "a.txt", "one\n");

        let diff = diff_file(&open(&tmp), Path::new("a.txt"), &BaseRef::head()).unwrap();

        assert_eq!(texts(&diff, LineOrigin::Added), ["one"]);
    }

    #[test]
    fn a_diff_against_an_older_commit_includes_what_was_committed_since() {
        let (tmp, repo) = init_repo();
        let first = commit(&repo, &[("a.txt", "one\n")], "first");
        commit(&repo, &[("a.txt", "one\ntwo\n")], "second");
        write(&repo, "a.txt", "one\ntwo\nthree\n");
        let base = BaseRef {
            kind: BaseKind::Commit,
            label: "first".into(),
            spec: first.to_string(),
        };

        let against_head = diff_file(&open(&tmp), Path::new("a.txt"), &BaseRef::head()).unwrap();
        let against_first = diff_file(&open(&tmp), Path::new("a.txt"), &base).unwrap();

        assert_eq!(texts(&against_head, LineOrigin::Added), ["three"]);
        assert_eq!(texts(&against_first, LineOrigin::Added), ["two", "three"]);
    }

    #[test]
    fn unusable_revisions_and_paths_are_errors() {
        let (tmp, repo) = init_repo();
        commit(&repo, &[("a.txt", "a\n")], "init");
        let project = open(&tmp);
        let dash = BaseRef {
            kind: BaseKind::Commit,
            label: "x".into(),
            spec: "--output=/tmp/x".into(),
        };
        let missing = BaseRef {
            kind: BaseKind::Commit,
            label: "x".into(),
            spec: "deadbeef".into(),
        };

        assert!(diff_file(&project, Path::new("a.txt"), &dash).is_err());
        assert!(diff_file(&project, Path::new("a.txt"), &missing).is_err());
        assert!(diff_file(&project, Path::new("../a.txt"), &BaseRef::head()).is_err());
        assert!(diff_file(&project, Path::new("/etc/passwd"), &BaseRef::head()).is_err());
    }

    #[test]
    fn terminal_escapes_in_file_content_do_not_reach_the_diff_text() {
        let (tmp, repo) = init_repo();
        commit(&repo, &[("a.txt", "a\n")], "init");
        write(&repo, "a.txt", "evil \u{1b}[31mred\u{7}\n");

        let diff = diff_file(&open(&tmp), Path::new("a.txt"), &BaseRef::head()).unwrap();

        assert!(
            texts(&diff, LineOrigin::Added)
                .iter()
                .all(|t| !t.chars().any(char::is_control))
        );
    }

    #[test]
    fn bases_list_head_then_branches_then_recent_commits() {
        let (tmp, repo) = init_repo();
        let first = commit(&repo, &[("a.txt", "1\n")], "first change");
        commit(&repo, &[("a.txt", "2\n")], "second change");
        repo.branch("feature", &repo.find_commit(first).unwrap(), false)
            .unwrap();

        let list = bases(&open(&tmp)).unwrap();

        let labels: Vec<&str> = list.iter().map(|b| b.label.as_str()).collect();
        assert_eq!(labels[0], "HEAD");
        assert_eq!(&labels[1..3], ["feature", "main"]);
        assert!(labels[3].ends_with("second change") && labels[4].ends_with("first change"));
        assert_eq!(list[1].spec, "refs/heads/feature");
        assert_eq!(list[4].spec, first.to_string());
    }

    #[test]
    fn a_repository_without_commits_still_offers_head() {
        let (tmp, _repo) = init_repo();

        let list = bases(&open(&tmp)).unwrap();

        assert_eq!(list.len(), 1);
    }
}
