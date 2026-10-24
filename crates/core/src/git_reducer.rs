//! Reducer logic for git: the changed-files list, the diff overlay, the base
//! picker, blame, and keeping Neovim's gutter in step.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::diff_view::{DiffContent, DiffView};
use crate::git::{BaseRef, ChangeKind, FileChange, FileDiff};
use crate::layout::diff_viewport_rows;
use crate::nav::clean_relative;
use crate::output::{Job, OutputTab};
use crate::state::{AppState, Panel};

/// What was last pushed to Neovim for the current file. A new value means
/// the gutter is out of date.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarksKey {
    pub path: PathBuf,
    pub change: Option<(ChangeKind, u64)>,
    pub head: Option<String>,
    pub blame: bool,
}

fn say(state: AppState, message: impl Into<String>) -> AppState {
    AppState {
        status: Some(message.into()),
        ..state
    }
}

fn with_job(state: &AppState, job: Job) -> Vec<Job> {
    let mut jobs = state.pending_jobs.clone();
    jobs.push(job);
    jobs
}

fn change_of<'a>(state: &'a AppState, path: &Path) -> Option<&'a FileChange> {
    state.git_changes.iter().find(|c| c.path == path)
}

/// Queues a gutter/blame update if what Neovim shows for the current file
/// is out of date.
pub(crate) fn sync_marks(state: AppState) -> AppState {
    let (Some(path), Some(socket)) = (state.current_file.clone(), state.editor_socket.clone())
    else {
        return state;
    };
    if !state.git_loaded {
        return state;
    }
    let key = MarksKey {
        change: change_of(&state, &path).map(|c| (c.kind, c.stamp)),
        head: state.git_head.clone(),
        blame: state.blame_on,
        path: path.clone(),
    };
    if state.marks_key.as_ref() == Some(&key) {
        return state;
    }
    let pending_jobs = with_job(
        &state,
        Job::SyncMarks {
            socket,
            path,
            blame: state.blame_on,
        },
    );
    AppState {
        marks_key: Some(key),
        pending_jobs,
        ..state
    }
}

pub(crate) fn git_loaded(
    state: AppState,
    head: Option<String>,
    changes: Arc<Vec<FileChange>>,
) -> AppState {
    let last = changes.len().saturating_sub(1);
    // The list shifts when files appear or are committed: stay on the file
    // that was highlighted, not on the same row number.
    let selected = state
        .git_changes
        .get(state.git_selected)
        .and_then(|before| changes.iter().position(|c| c.path == before.path))
        .unwrap_or_else(|| state.git_selected.min(last));
    let mut pending_jobs = state.pending_jobs.clone();
    if let Some(socket) = state.editor_socket.clone() {
        // Which file is the user looking at now?
        pending_jobs.push(Job::FollowEditor { socket });
    }
    sync_marks(AppState {
        git_selected: selected,
        git_changes: changes,
        git_head: head,
        git_loaded: true,
        pending_jobs,
        ..state
    })
}

/// The gutter in Neovim could not be drawn (a new Neovim, a file that is not
/// open yet, unsaved edits): forget what was pushed so the next look tries
/// again.
pub(crate) fn marks_stale(state: AppState) -> AppState {
    AppState {
        marks_key: None,
        ..state
    }
}

pub(crate) fn show_git(state: AppState) -> AppState {
    AppState {
        focus: Panel::Bottom,
        output_tab: OutputTab::Git,
        ..state
    }
}

/// A file was shown in Neovim: it is the current one.
pub(crate) fn file_shown(state: AppState, path: PathBuf) -> AppState {
    sync_marks(AppState {
        current_file: Some(path),
        ..state
    })
}

/// Neovim reports its current file (an absolute path, already resolved).
pub(crate) fn editor_file_changed(state: AppState, absolute: Option<PathBuf>) -> AppState {
    let relative = absolute
        .as_deref()
        .and_then(|path| path.strip_prefix(&state.project.root).ok())
        .and_then(clean_relative);
    if relative == state.current_file {
        return state;
    }
    sync_marks(AppState {
        current_file: relative,
        ..state
    })
}

pub(crate) fn toggle_blame(state: AppState) -> AppState {
    let blame_on = !state.blame_on;
    let message = if blame_on { "Blame on" } else { "Blame off" };
    sync_marks(AppState {
        blame_on,
        status: Some(message.to_string()),
        ..state
    })
}

fn start_diff(state: AppState, path: PathBuf) -> AppState {
    let id = state.last_job_id + 1;
    let mut pending_jobs = with_job(
        &state,
        Job::LoadDiff {
            id,
            path: path.clone(),
            base: state.base.clone(),
        },
    );
    if state.bases.is_empty() {
        pending_jobs.push(Job::LoadBases);
    }
    AppState {
        diff: Some(DiffView::new(path, state.base.clone(), id)),
        last_job_id: id,
        pending_jobs,
        ..state
    }
}

pub(crate) fn open_diff_selected(state: AppState) -> AppState {
    match state
        .git_changes
        .get(state.git_selected)
        .map(|c| c.path.clone())
    {
        Some(path) => start_diff(state, path),
        None => say(state, "No changed file selected"),
    }
}

pub(crate) fn open_diff_current(state: AppState) -> AppState {
    match state.current_file.clone() {
        Some(path) => start_diff(state, path),
        None => say(
            state,
            "Open a file in the editor first (leader+e), then press leader+=",
        ),
    }
}

pub(crate) fn diff_loaded(
    state: AppState,
    id: u64,
    result: Result<Arc<FileDiff>, String>,
) -> AppState {
    let Some(view) = state.diff.as_ref().filter(|v| v.id == id) else {
        return state;
    };
    let content = match result {
        Ok(diff) => DiffContent::Ready(diff),
        Err(message) => DiffContent::Failed(message),
    };
    let diff = DiffView {
        content,
        scroll: 0,
        ..view.clone()
    }
    .with_cache();
    AppState {
        diff: Some(diff),
        ..state
    }
}

fn max_scroll(view: &DiffView, terminal_height: u16) -> usize {
    view.rows()
        .saturating_sub(usize::from(diff_viewport_rows(terminal_height)))
}

pub(crate) fn diff_scroll(state: AppState, delta: i32) -> AppState {
    let Some(view) = state.diff.clone() else {
        return state;
    };
    let max = max_scroll(&view, state.terminal_height) as i64;
    let scroll = (view.scroll as i64 + i64::from(delta)).clamp(0, max) as usize;
    AppState {
        diff: Some(DiffView { scroll, ..view }),
        ..state
    }
}

/// Jump to the next (`delta > 0`) or previous hunk.
pub(crate) fn diff_hunk(state: AppState, delta: i32) -> AppState {
    let Some(view) = state.diff.clone() else {
        return state;
    };
    if view.diff().is_none() {
        return state;
    }
    let starts = view.hunk_starts();
    let target = if delta >= 0 {
        starts.iter().copied().find(|start| *start > view.scroll)
    } else {
        starts
            .iter()
            .rev()
            .copied()
            .find(|start| *start < view.scroll)
    };
    let Some(target) = target else {
        return say(
            state,
            if delta >= 0 {
                "Last change"
            } else {
                "First change"
            },
        );
    };
    let scroll = target.min(max_scroll(&view, state.terminal_height));
    if scroll == view.scroll {
        // The last change starts below what the view can scroll to.
        return say(
            state,
            if delta >= 0 {
                "Last change"
            } else {
                "First change"
            },
        );
    }
    AppState {
        diff: Some(DiffView { scroll, ..view }),
        ..state
    }
}

pub(crate) fn diff_layout(state: AppState) -> AppState {
    let Some(view) = state.diff.clone() else {
        return state;
    };
    let hunk = view.current_hunk();
    let mut next = DiffView {
        side_by_side: !view.side_by_side,
        ..view
    }
    .with_cache();
    if let Some(hunk) = hunk {
        next.scroll = next
            .hunk_starts()
            .get(hunk)
            .copied()
            .unwrap_or(0)
            .min(max_scroll(&next, state.terminal_height));
    }
    AppState {
        diff: Some(next),
        ..state
    }
}

pub(crate) fn diff_close(state: AppState) -> AppState {
    AppState {
        diff: None,
        ..state
    }
}

pub(crate) fn bases_loaded(state: AppState, bases: Arc<Vec<BaseRef>>) -> AppState {
    AppState { bases, ..state }
}

pub(crate) fn picker_open(state: AppState) -> AppState {
    let Some(view) = state.diff.clone() else {
        return state;
    };
    let at = state
        .bases
        .iter()
        .position(|b| *b == view.base)
        .unwrap_or(0);
    // Branches and commits change while LF-Mide runs: read them afresh.
    let pending_jobs = with_job(&state, Job::LoadBases);
    AppState {
        diff: Some(DiffView {
            picker: Some(at),
            ..view
        }),
        pending_jobs,
        ..state
    }
}

pub(crate) fn picker_move(state: AppState, delta: i16) -> AppState {
    let Some(view) = state.diff.clone() else {
        return state;
    };
    let (Some(at), count) = (view.picker, state.bases.len()) else {
        return state;
    };
    if count == 0 {
        return state;
    }
    let next = (at as i64 + i64::from(delta)).clamp(0, count as i64 - 1) as usize;
    AppState {
        diff: Some(DiffView {
            picker: Some(next),
            ..view
        }),
        ..state
    }
}

pub(crate) fn picker_cancel(state: AppState) -> AppState {
    let Some(view) = state.diff.clone() else {
        return state;
    };
    AppState {
        diff: Some(DiffView {
            picker: None,
            ..view
        }),
        ..state
    }
}

/// Diff the same file against the chosen base.
pub(crate) fn picker_select(state: AppState) -> AppState {
    let Some(view) = state.diff.clone() else {
        return state;
    };
    let Some(base) = view.picker.and_then(|at| state.bases.get(at)).cloned() else {
        return picker_cancel(state);
    };
    let state = AppState {
        base: base.clone(),
        ..state
    };
    let state = start_diff(state, view.path.clone());
    // Keep the layout the user chose.
    let diff = state.diff.clone().map(|v| DiffView {
        side_by_side: view.side_by_side,
        ..v
    });
    AppState { diff, ..state }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::Action;
    use crate::config::Config;
    use crate::git::{BaseKind, DiffLine, Hunk, LineOrigin};
    use crate::project::{Project, ProjectLayout};
    use crate::reducer::reduce;

    fn base_state() -> AppState {
        AppState::new(
            Project {
                root: PathBuf::from("/p"),
                name: "demo".into(),
                layout: ProjectLayout::ComposeApp,
            },
            Config::default(),
        )
    }

    fn with_editor(mut s: AppState) -> AppState {
        s.editor_socket = Some(PathBuf::from("/tmp/nvim.sock"));
        s
    }

    fn change(path: &str, kind: ChangeKind, stamp: u64) -> FileChange {
        FileChange {
            path: PathBuf::from(path),
            kind,
            staged: false,
            stamp,
        }
    }

    fn loaded(s: AppState, head: &str, changes: Vec<FileChange>) -> AppState {
        reduce(
            s,
            Action::GitLoaded {
                head: Some(head.into()),
                changes: Arc::new(changes),
            },
        )
    }

    fn sync_jobs(s: &AppState) -> Vec<&Job> {
        s.pending_jobs
            .iter()
            .filter(|j| matches!(j, Job::SyncMarks { .. }))
            .collect()
    }

    fn drained(mut s: AppState) -> AppState {
        s.pending_jobs.clear();
        s
    }

    // --- the changed-files list ---

    #[test]
    fn the_first_read_of_the_repository_is_remembered() {
        let s = loaded(
            base_state(),
            "abc",
            vec![change("a.kt", ChangeKind::Modified, 1)],
        );

        assert!(s.git_loaded);
        assert_eq!(s.git_head.as_deref(), Some("abc"));
        assert_eq!(s.git_changes.len(), 1);
    }

    #[test]
    fn the_highlight_stays_inside_a_shrinking_list() {
        let mut s = loaded(
            base_state(),
            "abc",
            vec![
                change("a", ChangeKind::Modified, 1),
                change("b", ChangeKind::Modified, 1),
            ],
        );
        s.git_selected = 1;

        let s = loaded(s, "abc", vec![change("a", ChangeKind::Modified, 1)]);

        assert_eq!(s.git_selected, 0);
    }

    #[test]
    fn the_git_tab_moves_its_own_highlight() {
        let s = loaded(
            base_state(),
            "abc",
            vec![
                change("a", ChangeKind::Modified, 1),
                change("b", ChangeKind::Added, 1),
            ],
        );
        let s = reduce(s, Action::ShowGit);

        let down = reduce(s.clone(), Action::OutputScroll(-1));
        let past = reduce(down.clone(), Action::OutputScroll(-9));
        let up = reduce(down, Action::OutputScroll(1));

        assert_eq!((s.focus, s.output_tab), (Panel::Bottom, OutputTab::Git));
        assert_eq!((past.git_selected, up.git_selected), (1, 0));
    }

    // --- keeping Neovim's gutter current ---

    #[test]
    fn nothing_is_pushed_without_an_editor_a_current_file_or_a_first_read() {
        let mut no_editor = base_state();
        no_editor.current_file = Some(PathBuf::from("a.kt"));
        let no_file = with_editor(base_state());
        let mut unread = with_editor(base_state());
        unread.current_file = Some(PathBuf::from("a.kt"));

        assert!(sync_jobs(&loaded(no_editor, "h", vec![])).is_empty());
        assert!(sync_jobs(&loaded(no_file, "h", vec![])).is_empty());
        assert!(sync_jobs(&reduce(unread, Action::ToggleBlame)).is_empty());
    }

    #[test]
    fn the_first_read_with_a_file_open_pushes_once_and_not_again_while_nothing_changes() {
        let mut s = with_editor(base_state());
        s.current_file = Some(PathBuf::from("a.kt"));
        let changes = vec![change("a.kt", ChangeKind::Modified, 5)];

        let first = loaded(s, "h1", changes.clone());
        let again = loaded(drained(first.clone()), "h1", changes);

        assert_eq!(sync_jobs(&first).len(), 1);
        assert!(matches!(
            sync_jobs(&first)[0],
            Job::SyncMarks { path, blame: false, .. } if path == Path::new("a.kt")
        ));
        assert!(sync_jobs(&again).is_empty());
    }

    #[test]
    fn a_further_edit_a_new_commit_or_the_blame_switch_each_push_again() {
        let mut s = with_editor(base_state());
        s.current_file = Some(PathBuf::from("a.kt"));
        let first = drained(loaded(
            s,
            "h1",
            vec![change("a.kt", ChangeKind::Modified, 5)],
        ));

        let edited = loaded(
            first.clone(),
            "h1",
            vec![change("a.kt", ChangeKind::Modified, 6)],
        );
        let committed = loaded(first.clone(), "h2", vec![]);
        let blamed = reduce(first.clone(), Action::ToggleBlame);
        let unrelated = loaded(
            first,
            "h1",
            vec![
                change("a.kt", ChangeKind::Modified, 5),
                change("other.kt", ChangeKind::Added, 1),
            ],
        );

        assert_eq!(sync_jobs(&edited).len(), 1);
        assert_eq!(sync_jobs(&committed).len(), 1);
        assert!(matches!(
            sync_jobs(&blamed)[0],
            Job::SyncMarks { blame: true, .. }
        ));
        assert!(
            sync_jobs(&unrelated).is_empty(),
            "another file changed, not this one"
        );
    }

    #[test]
    fn showing_a_file_makes_it_current_and_pushes_its_marks() {
        let s = loaded(
            with_editor(base_state()),
            "h",
            vec![change("a.kt", ChangeKind::Modified, 1)],
        );

        let s = reduce(drained(s), Action::FileOpened(PathBuf::from("a.kt")));

        assert_eq!(s.current_file, Some(PathBuf::from("a.kt")));
        assert_eq!(sync_jobs(&s).len(), 1);
    }

    #[test]
    fn the_editors_own_file_switches_are_followed_inside_the_project_only() {
        let s = loaded(with_editor(base_state()), "h", vec![]);

        let inside = reduce(
            drained(s.clone()),
            Action::EditorFileChanged(Some(PathBuf::from("/p/src/B.kt"))),
        );
        let outside = reduce(
            drained(inside.clone()),
            Action::EditorFileChanged(Some(PathBuf::from("/etc/hosts"))),
        );
        let none = reduce(drained(inside.clone()), Action::EditorFileChanged(None));
        let same = reduce(
            drained(inside.clone()),
            Action::EditorFileChanged(Some(PathBuf::from("/p/src/B.kt"))),
        );

        assert_eq!(inside.current_file, Some(PathBuf::from("src/B.kt")));
        assert_eq!(sync_jobs(&inside).len(), 1);
        assert_eq!(outside.current_file, None);
        assert_eq!(none.current_file, None);
        assert!(same.pending_jobs.is_empty());
    }

    #[test]
    fn each_read_asks_the_editor_which_file_is_current_only_when_there_is_one() {
        let with = loaded(with_editor(base_state()), "h", vec![]);
        let without = loaded(base_state(), "h", vec![]);

        assert!(
            with.pending_jobs
                .iter()
                .any(|j| matches!(j, Job::FollowEditor { .. }))
        );
        assert!(
            !without
                .pending_jobs
                .iter()
                .any(|j| matches!(j, Job::FollowEditor { .. }))
        );
    }

    #[test]
    fn blame_starts_as_configured_and_toggles() {
        let on = Config {
            show_blame: true,
            ..Config::default()
        };
        let s = AppState::new(base_state().project, on);

        assert!(s.blame_on);
        assert!(!base_state().blame_on);
        let toggled = reduce(base_state(), Action::ToggleBlame);
        assert!(toggled.blame_on);
        assert_eq!(toggled.status.as_deref(), Some("Blame on"));
        assert_eq!(
            reduce(toggled, Action::ToggleBlame).status.as_deref(),
            Some("Blame off")
        );
    }

    // --- the diff overlay ---

    fn hunk(old: u32, new: u32, lines: Vec<DiffLine>) -> Hunk {
        Hunk {
            old_start: old,
            old_lines: 0,
            new_start: new,
            new_lines: 0,
            lines,
        }
    }

    fn line(origin: LineOrigin, old: Option<u32>, new: Option<u32>) -> DiffLine {
        DiffLine {
            origin,
            text: "t".into(),
            old_no: old,
            new_no: new,
        }
    }

    fn long_diff() -> FileDiff {
        // Ten rows in the first hunk, then a second one.
        let mut first = vec![line(LineOrigin::Context, Some(1), Some(1))];
        for n in 2..=9 {
            first.push(line(LineOrigin::Added, None, Some(n)));
        }
        FileDiff {
            hunks: vec![
                hunk(1, 1, first),
                hunk(
                    80,
                    90,
                    vec![
                        line(LineOrigin::Removed, Some(80), None),
                        line(LineOrigin::Context, Some(81), Some(90)),
                    ],
                ),
            ],
            ..FileDiff::default()
        }
    }

    fn open_diff() -> AppState {
        let mut s = base_state();
        s.terminal_height = 9; // a viewport of 3 rows
        s.current_file = Some(PathBuf::from("a.kt"));
        let s = reduce(s, Action::OpenDiffCurrent);
        let id = s.diff.as_ref().unwrap().id;
        reduce(
            drained(s),
            Action::DiffLoaded {
                id,
                result: Ok(Arc::new(long_diff())),
            },
        )
    }

    #[test]
    fn opening_a_diff_queues_the_load_and_the_base_list() {
        let mut s = base_state();
        s.current_file = Some(PathBuf::from("a.kt"));

        let s = reduce(s, Action::OpenDiffCurrent);

        let view = s.diff.as_ref().unwrap();
        assert_eq!(
            (view.path.as_path(), view.id, &view.content),
            (Path::new("a.kt"), 1, &DiffContent::Loading)
        );
        assert!(
            matches!(&s.pending_jobs[0], Job::LoadDiff { id: 1, base, .. } if *base == BaseRef::head())
        );
        assert!(s.pending_jobs.contains(&Job::LoadBases));
    }

    #[test]
    fn a_diff_needs_a_file_to_diff() {
        let none = reduce(base_state(), Action::OpenDiffCurrent);
        let nothing_selected = reduce(base_state(), Action::OpenDiffSelected);

        assert!(none.diff.is_none() && none.status.unwrap().contains("Open a file"));
        assert!(nothing_selected.diff.is_none());
    }

    #[test]
    fn the_diff_of_the_highlighted_changed_file_opens() {
        let s = loaded(
            base_state(),
            "h",
            vec![
                change("a.kt", ChangeKind::Modified, 1),
                change("b.kt", ChangeKind::Added, 1),
            ],
        );
        let s = reduce(reduce(s, Action::ShowGit), Action::OutputScroll(-1));

        let s = reduce(s, Action::OpenDiffSelected);

        assert_eq!(s.diff.unwrap().path, PathBuf::from("b.kt"));
    }

    #[test]
    fn results_for_an_older_request_are_ignored() {
        let s = open_diff();
        let stale = reduce(
            s.clone(),
            Action::DiffLoaded {
                id: 99,
                result: Err("late".into()),
            },
        );

        assert_eq!(stale.diff, s.diff);
    }

    #[test]
    fn a_failed_diff_says_why() {
        let mut s = base_state();
        s.current_file = Some(PathBuf::from("a.kt"));
        let s = reduce(s, Action::OpenDiffCurrent);

        let s = reduce(
            s,
            Action::DiffLoaded {
                id: 1,
                result: Err("not a repository".into()),
            },
        );

        assert_eq!(
            s.diff.unwrap().content,
            DiffContent::Failed("not a repository".into())
        );
    }

    #[test]
    fn scrolling_stays_within_the_diff() {
        let s = open_diff(); // 10 + 3 = 13 rows, 3 visible

        let down = reduce(s.clone(), Action::DiffScroll(4));
        let end = reduce(s.clone(), Action::DiffScroll(500));
        let up = reduce(end.clone(), Action::DiffScroll(-500));

        assert_eq!(down.diff.unwrap().scroll, 4);
        assert_eq!(end.diff.unwrap().scroll, 13 - 3);
        assert_eq!(up.diff.unwrap().scroll, 0);
    }

    #[test]
    fn hunks_are_jumped_to_and_the_ends_are_announced() {
        let s = open_diff();

        let next = reduce(s.clone(), Action::DiffHunk(1));
        let none_after = reduce(next.clone(), Action::DiffHunk(1));
        let back = reduce(next.clone(), Action::DiffHunk(-1));
        let none_before = reduce(s, Action::DiffHunk(-1));

        assert_eq!(next.diff.as_ref().unwrap().scroll, 10);
        assert_eq!(none_after.status.as_deref(), Some("Last change"));
        assert_eq!(back.diff.unwrap().scroll, 0);
        assert_eq!(none_before.status.as_deref(), Some("First change"));
    }

    #[test]
    fn switching_layout_keeps_the_hunk_in_view() {
        let s = open_diff();
        let at_second = reduce(s, Action::DiffHunk(1));

        let side = reduce(at_second, Action::DiffLayout);

        let view = side.diff.unwrap();
        assert!(view.side_by_side);
        assert_eq!(view.current_hunk(), Some(1));
    }

    #[test]
    fn closing_removes_the_overlay_and_so_does_opening_a_file_from_it() {
        let s = open_diff();
        let mut with_editor = s.clone();
        with_editor.editor_socket = Some(PathBuf::from("/s"));

        let closed = reduce(s, Action::DiffClose);
        let opened = reduce(
            with_editor,
            Action::OpenFile {
                path: PathBuf::from("a.kt"),
                line: Some(3),
            },
        );

        assert!(closed.diff.is_none() && opened.diff.is_none());
    }

    // --- choosing what to diff against ---

    fn bases() -> Arc<Vec<BaseRef>> {
        Arc::new(vec![
            BaseRef::head(),
            BaseRef {
                kind: BaseKind::Branch,
                label: "main".into(),
                spec: "refs/heads/main".into(),
            },
            BaseRef {
                kind: BaseKind::Commit,
                label: "abc1234 Fix".into(),
                spec: "abc1234".into(),
            },
        ])
    }

    #[test]
    fn picking_a_base_reloads_the_same_file_against_it_and_keeps_the_layout() {
        let s = reduce(open_diff(), Action::DiffLayout);
        let s = reduce(s, Action::BasesLoaded(bases()));
        let s = reduce(s, Action::BasePickerOpen);
        assert_eq!(s.diff.as_ref().unwrap().picker, Some(0));

        let s = reduce(
            reduce(s, Action::BasePickerMove(2)),
            Action::BasePickerSelect,
        );

        let view = s.diff.as_ref().unwrap();
        assert_eq!(view.base.label, "abc1234 Fix");
        assert_eq!(
            (view.picker, &view.content, view.side_by_side),
            (None, &DiffContent::Loading, true)
        );
        assert_eq!(s.base.spec, "abc1234");
        assert!(
            matches!(s.pending_jobs.last(), Some(Job::LoadDiff { base, .. }) if base.spec == "abc1234")
        );
    }

    #[test]
    fn the_picker_moves_within_the_list_and_cancels() {
        let s = reduce(
            reduce(open_diff(), Action::BasesLoaded(bases())),
            Action::BasePickerOpen,
        );

        let far = reduce(s.clone(), Action::BasePickerMove(50));
        let back = reduce(far.clone(), Action::BasePickerMove(-50));
        let cancelled = reduce(far.clone(), Action::BasePickerCancel);

        assert_eq!(far.diff.unwrap().picker, Some(2));
        assert_eq!(back.diff.unwrap().picker, Some(0));
        assert_eq!(cancelled.diff.unwrap().picker, None);
        assert_eq!(cancelled.base, BaseRef::head());
    }

    #[test]
    fn opening_the_picker_before_the_bases_arrive_asks_for_them() {
        let s = open_diff();

        let s = reduce(s, Action::BasePickerOpen);

        assert!(s.pending_jobs.contains(&Job::LoadBases));
    }

    #[test]
    fn a_new_neovim_a_lost_one_or_a_failed_push_make_the_next_look_push_again() {
        let mut s = with_editor(base_state());
        s.current_file = Some(PathBuf::from("a.kt"));
        let pushed = drained(loaded(
            s,
            "h",
            vec![change("a.kt", ChangeKind::Modified, 1)],
        ));
        let look = |state: AppState| {
            loaded(
                drained(state),
                "h",
                vec![change("a.kt", ChangeKind::Modified, 1)],
            )
        };

        assert!(
            sync_jobs(&look(pushed.clone())).is_empty(),
            "nothing changed, nothing to push"
        );
        let relaunched = reduce(
            pushed.clone(),
            Action::EditorOpened(PathBuf::from("/tmp/new.sock")),
        );
        let lost_then_back = reduce(
            reduce(pushed.clone(), Action::EditorLost),
            Action::EditorOpened(PathBuf::from("/tmp/new.sock")),
        );
        let stale = reduce(pushed, Action::MarksStale);

        assert_eq!(sync_jobs(&look(relaunched)).len(), 1);
        assert_eq!(sync_jobs(&look(lost_then_back)).len(), 1);
        assert_eq!(sync_jobs(&look(stale)).len(), 1);
    }

    #[test]
    fn the_highlight_stays_on_its_file_when_the_list_shifts() {
        let s = loaded(
            base_state(),
            "h",
            vec![
                change("b.kt", ChangeKind::Modified, 1),
                change("c.kt", ChangeKind::Modified, 1),
            ],
        );
        let s = reduce(reduce(s, Action::ShowGit), Action::OutputScroll(-1));
        assert_eq!(s.git_changes[s.git_selected].path, PathBuf::from("c.kt"));

        // A new file sorts above, and b.kt is committed.
        let s = loaded(
            s,
            "h2",
            vec![
                change("a.kt", ChangeKind::Untracked, 1),
                change("c.kt", ChangeKind::Modified, 1),
            ],
        );

        assert_eq!(s.git_changes[s.git_selected].path, PathBuf::from("c.kt"));
    }

    #[test]
    fn if_the_highlighted_file_is_gone_the_row_stays_inside_the_list() {
        let s = loaded(
            base_state(),
            "h",
            vec![
                change("a.kt", ChangeKind::Modified, 1),
                change("b.kt", ChangeKind::Modified, 1),
            ],
        );
        let s = reduce(reduce(s, Action::ShowGit), Action::OutputScroll(-1));

        let s = loaded(s, "h2", vec![change("z.kt", ChangeKind::Modified, 1)]);

        assert_eq!(s.git_selected, 0);
    }

    #[test]
    fn every_opening_of_the_picker_reads_the_branches_and_commits_again() {
        let s = reduce(open_diff(), Action::BasesLoaded(bases()));

        let opened = reduce(s, Action::BasePickerOpen);

        assert!(opened.pending_jobs.contains(&Job::LoadBases));
    }

    #[test]
    fn the_last_change_is_announced_even_when_it_starts_below_what_can_scroll() {
        let s = open_diff(); // the second hunk starts at row 10 but scrolling stops at 10
        let at_end = reduce(reduce(s, Action::DiffHunk(1)), Action::DiffScroll(1));

        let again = reduce(at_end, Action::DiffHunk(1));

        assert_eq!(again.status.as_deref(), Some("Last change"));
    }

    #[test]
    fn side_by_side_rows_are_built_once_and_agree_with_a_fresh_build() {
        let s = reduce(open_diff(), Action::DiffLayout);

        let view = s.diff.as_ref().unwrap();
        assert!(view.side_cache.is_some());
        let direct = crate::diff_view::row_count(view.diff().unwrap(), true);
        assert_eq!(view.rows(), direct);
        assert_eq!(
            view.hunk_starts(),
            crate::diff_view::hunk_rows(view.diff().unwrap(), true)
        );
        let back = reduce(s, Action::DiffLayout);
        assert!(back.diff.unwrap().side_cache.is_none());
    }

    #[test]
    fn the_picker_starts_on_the_base_in_use() {
        let mut s = reduce(open_diff(), Action::BasesLoaded(bases()));
        s.base = bases()[1].clone();
        if let Some(view) = s.diff.as_mut() {
            view.base = bases()[1].clone();
        }

        let s = reduce(s, Action::BasePickerOpen);

        assert_eq!(s.diff.unwrap().picker, Some(1));
    }
}
