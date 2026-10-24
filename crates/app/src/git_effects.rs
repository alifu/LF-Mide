//! Background git work: watching the repository, diffs, base lists, and
//! drawing signs and blame in Neovim.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use lf_mide_core::{Action, BaseRef, LineMark, MarkKind, blame_text, marks_from_diff};
use lf_mide_editor::{EditorError, Sign};
use lf_mide_vcs::ProjectRepo;
use tokio::sync::mpsc::UnboundedSender;

/// How often the repository is read.
pub const GIT_POLL: Duration = Duration::from_secs(3);
/// How long to wait for Neovim's socket for these background calls.
const EDITOR_PATIENCE: Duration = Duration::from_millis(500);

fn error_text(error: impl std::fmt::Display) -> String {
    error.to_string()
}

/// Reads the repository now and reports the result.
pub fn read_repository(root: &Path) -> Option<Action> {
    let project = match ProjectRepo::open(root) {
        Ok(Some(project)) => project,
        Ok(None) => return None,
        Err(error) => {
            tracing::warn!(?error, "reading git status failed");
            return None;
        }
    };
    match lf_mide_vcs::changes(&project) {
        Ok((head, changes)) => Some(Action::GitLoaded {
            head,
            changes: Arc::new(changes),
        }),
        Err(error) => {
            tracing::warn!(?error, "reading git status failed");
            None
        }
    }
}

/// Reads the repository, then waits [`GIT_POLL`], for as long as the app
/// runs. The wait comes after the read, so a slow `git status` on a big
/// repository never turns into back-to-back scans.
pub fn spawn_git_watch(root: PathBuf, tx: UnboundedSender<Action>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let path = root.clone();
            let read = tokio::task::spawn_blocking(move || read_repository(&path)).await;
            if let Ok(Some(action)) = read
                && tx.send(action).is_err()
            {
                return;
            }
            tokio::time::sleep(GIT_POLL).await;
        }
    })
}

/// Reads the repository once, now, on a thread.
pub fn spawn_git_refresh(root: PathBuf, tx: UnboundedSender<Action>) {
    std::thread::spawn(move || {
        if let Some(action) = read_repository(&root) {
            let _ = tx.send(action);
        }
    });
}

pub fn spawn_load_diff(
    root: PathBuf,
    id: u64,
    path: PathBuf,
    base: BaseRef,
    tx: UnboundedSender<Action>,
) {
    std::thread::spawn(move || {
        let result = ProjectRepo::open(&root)
            .map_err(error_text)
            .and_then(|found| found.ok_or_else(|| "not a git repository".to_string()))
            .and_then(|project| lf_mide_vcs::diff_file(&project, &path, &base).map_err(error_text))
            .map(Arc::new);
        let _ = tx.send(Action::DiffLoaded { id, result });
    });
}

pub fn spawn_load_bases(root: PathBuf, tx: UnboundedSender<Action>) {
    std::thread::spawn(move || {
        let bases = ProjectRepo::open(&root)
            .ok()
            .flatten()
            .and_then(|project| lf_mide_vcs::bases(&project).ok());
        if let Some(bases) = bases {
            let _ = tx.send(Action::BasesLoaded(Arc::new(bases)));
        }
    });
}

fn sign_of(kind: MarkKind) -> Sign {
    match kind {
        MarkKind::Added => Sign::Added,
        MarkKind::Changed => Sign::Changed,
        MarkKind::DeletedBelow => Sign::DeletedBelow,
        MarkKind::DeletedAbove => Sign::DeletedAbove,
    }
}

/// Signs and blame text, as sent to Neovim, and why blame is missing.
struct Marks {
    signs: Vec<(u32, Sign)>,
    blame: Vec<(u32, String)>,
    blame_problem: Option<String>,
}

/// The signs and blame text for `path`, computed from the repository. A file
/// that cannot be blamed (too big, unreadable) still gets its signs.
fn marks_for(root: &Path, path: &Path, blame: bool) -> Result<Marks, String> {
    let project = ProjectRepo::open(root)
        .map_err(error_text)?
        .ok_or_else(|| "not a git repository".to_string())?;
    let diff = lf_mide_vcs::diff_file(&project, path, &BaseRef::head()).map_err(error_text)?;
    let signs = marks_from_diff(&diff)
        .into_iter()
        .map(|LineMark { line, kind }| (line, sign_of(kind)))
        .collect();
    if !blame {
        return Ok(Marks {
            signs,
            blame: Vec::new(),
            blame_problem: None,
        });
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
    let (texts, blame_problem) = match lf_mide_vcs::blame_file(&project, path, &root.join(path)) {
        Ok(lines) => (
            lines
                .unwrap_or_default()
                .iter()
                .map(|line| (line.line, blame_text(line, now)))
                .collect(),
            None,
        ),
        Err(error) => (Vec::new(), Some(error_text(error))),
    };
    Ok(Marks {
        signs,
        blame: texts,
        blame_problem,
    })
}

/// Draws the signs (and blame) for `path` in Neovim. Only the newest request
/// is applied: `latest` counts requests, and one that is no longer the newest
/// is dropped before it does any work. When the picture cannot be drawn right
/// now (the file is not open, has unsaved edits, Neovim is slow) the reducer
/// is told, so the next look tries again.
pub fn spawn_sync_marks(
    root: PathBuf,
    socket: PathBuf,
    path: PathBuf,
    blame: bool,
    latest: Arc<AtomicU64>,
    tx: UnboundedSender<Action>,
) {
    let mine = latest.fetch_add(1, Ordering::SeqCst) + 1;
    tokio::spawn(async move {
        let absolute = root.join(&path);
        let try_again = || {
            let _ = tx.send(Action::MarksStale);
        };
        if latest.load(Ordering::SeqCst) != mine {
            return;
        }
        // Marks come from the file on disk; on a buffer with unsaved edits
        // they would land on the wrong lines.
        match lf_mide_editor::buffer_modified(&socket, &absolute, EDITOR_PATIENCE).await {
            Ok(Some(false)) => {}
            Ok(_) => return try_again(),
            Err(EditorError::NotRunning(_)) => {
                let _ = tx.send(Action::EditorLost);
                return;
            }
            Err(_) => return try_again(),
        }
        let computed = tokio::task::spawn_blocking({
            let (root, path) = (root.clone(), path.clone());
            move || marks_for(&root, &path, blame)
        })
        .await;
        let marks = match computed {
            Ok(Ok(marks)) => marks,
            Ok(Err(message)) => {
                let _ = tx.send(Action::Status(format!("git marks: {message}")));
                return;
            }
            Err(_) => return,
        };
        if latest.load(Ordering::SeqCst) != mine {
            return;
        }
        if let Some(problem) = &marks.blame_problem {
            let _ = tx.send(Action::Status(format!("blame unavailable: {problem}")));
        }
        match lf_mide_editor::set_git_marks(
            &socket,
            &absolute,
            &marks.signs,
            &marks.blame,
            EDITOR_PATIENCE,
        )
        .await
        {
            Ok(true) => {}
            Ok(false) | Err(EditorError::TimedOut) => try_again(),
            Err(EditorError::NotRunning(_)) => {
                let _ = tx.send(Action::EditorLost);
            }
            Err(error) => {
                let _ = tx.send(Action::Status(format!("git marks: {error}")));
                try_again();
            }
        }
    });
}

/// Asks Neovim which file is current and reports it (resolved, so it can be
/// compared with the project root).
pub fn spawn_follow_editor(socket: PathBuf, tx: UnboundedSender<Action>) {
    tokio::spawn(async move {
        match lf_mide_editor::current_file(&socket, EDITOR_PATIENCE).await {
            Ok(file) => {
                let resolved = file.map(|path| std::fs::canonicalize(&path).unwrap_or(path));
                let _ = tx.send(Action::EditorFileChanged(resolved));
            }
            Err(EditorError::NotRunning(_)) => {
                let _ = tx.send(Action::EditorLost);
            }
            Err(_) => {}
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .args([
                "-c",
                "user.name=T",
                "-c",
                "user.email=t@e",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(dir)
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    }

    fn repo_with_edit() -> tempfile::TempDir {
        let tmp = tempfile::TempDir::new().unwrap();
        git(tmp.path(), &["init", "-q", "-b", "main"]);
        std::fs::write(tmp.path().join("a.kt"), "one\ntwo\nthree\n").unwrap();
        git(tmp.path(), &["add", "."]);
        git(tmp.path(), &["commit", "-q", "-m", "first"]);
        std::fs::write(tmp.path().join("a.kt"), "one\nTWO\nthree\nfour\n").unwrap();
        tmp
    }

    #[test]
    fn marks_come_from_the_diff_and_blame_from_the_log() {
        let tmp = repo_with_edit();

        let Marks { signs, blame, .. } = marks_for(tmp.path(), Path::new("a.kt"), true).unwrap();

        assert_eq!(signs, [(2, Sign::Changed), (4, Sign::Added)]);
        assert_eq!(blame.len(), 4);
        assert!(blame[0].1.contains("T · ") && blame[0].1.ends_with(char::is_alphanumeric));
        assert_eq!(blame[1].1, "Not committed yet");
        assert_eq!(blame[3].1, "Not committed yet");
    }

    #[test]
    fn blame_is_left_out_unless_asked_for() {
        let tmp = repo_with_edit();

        let Marks { signs, blame, .. } = marks_for(tmp.path(), Path::new("a.kt"), false).unwrap();

        assert_eq!(signs.len(), 2);
        assert!(blame.is_empty());
    }

    #[test]
    fn reading_a_repository_reports_changes_and_a_folder_without_one_reports_nothing() {
        let tmp = repo_with_edit();
        let plain = tempfile::TempDir::new().unwrap();

        let Some(Action::GitLoaded { head, changes }) = read_repository(tmp.path()) else {
            panic!("expected a repository");
        };

        assert!(head.is_some());
        assert_eq!(changes.len(), 1);
        assert!(read_repository(plain.path()).is_none());
    }

    #[test]
    fn a_file_that_cannot_be_blamed_still_gets_its_signs() {
        let tmp = repo_with_edit();
        let big: String = (0..lf_mide_vcs::MAX_BLAME_LINES + 100)
            .map(|n| format!("l{n}\n"))
            .collect();
        std::fs::write(tmp.path().join("big.kt"), &big).unwrap();
        git(tmp.path(), &["add", "big.kt"]);
        git(tmp.path(), &["commit", "-q", "-m", "big"]);
        std::fs::write(tmp.path().join("big.kt"), format!("{big}more\n")).unwrap();

        let marks = marks_for(tmp.path(), Path::new("big.kt"), true).unwrap();

        assert_eq!(marks.signs.len(), 1);
        assert!(marks.blame.is_empty());
        assert!(marks.blame_problem.is_some());
    }

    #[test]
    fn a_missing_file_is_an_error_not_a_panic() {
        let tmp = repo_with_edit();

        assert!(marks_for(tmp.path(), Path::new("../outside.kt"), true).is_err());
    }
}
