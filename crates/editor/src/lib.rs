//! Talking to the Neovim we launched, over its `--listen` socket.

pub mod embedded;
pub mod keys;
pub mod lsp;
pub mod redraw;

use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::time::{Duration, Instant};

use nvim_rs::compat::tokio::Compat;
use nvim_rs::create::tokio::new_path;
use nvim_rs::error::LoopError;
use nvim_rs::rpc::handler::Dummy;
use nvim_rs::{Neovim, Value};
use thiserror::Error;
use tokio::io::WriteHalf;
use tokio::net::UnixStream;
use tokio::task::JoinHandle;

/// How long a single RPC call may take.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(10);
/// Background polls give up sooner, so a stuck socket does not pile up tasks.
pub const BACKGROUND_TIMEOUT: Duration = Duration::from_secs(2);
const RETRY_INTERVAL: Duration = Duration::from_millis(100);

/// Makes the file current and moves the cursor. The name is only ever data
/// here (`bufadd`), never part of an Ex command, so no character in it can
/// mean anything special.
const OPEN_LUA: &str = r#"
local path, line, column = ...
local buffer = vim.fn.bufadd(path)
vim.api.nvim_set_current_buf(buffer)
if line > 0 then
  local last = math.max(vim.api.nvim_buf_line_count(0), 1)
  local row = math.min(line, last)
  local text = vim.api.nvim_buf_get_lines(0, row - 1, row, false)[1] or ''
  pcall(vim.api.nvim_win_set_cursor, 0, { row, math.min(column, #text) })
  vim.cmd('normal! zz')
end
return vim.api.nvim_buf_get_name(0)
"#;

/// Draws git signs and blame text on a file's buffer. Everything arrives as
/// data (arguments), never as part of the code, so commit messages and author
/// names cannot do anything but be displayed. The buffer is found by exact
/// name: `bufnr()` would treat the path as a pattern.
const GIT_MARKS_LUA: &str = r#"
local path, marks, blame = ...
local target
for _, b in ipairs(vim.api.nvim_list_bufs()) do
  if vim.api.nvim_buf_is_loaded(b) and vim.api.nvim_buf_get_name(b) == path then
    target = b
    break
  end
end
if not target then return false end
local ns = vim.api.nvim_create_namespace('lf_mide_git')
vim.api.nvim_buf_clear_namespace(target, ns, 0, -1)
for name, color in pairs({ LfMideAdd = '#5faf5f', LfMideChange = '#d7af5f', LfMideDelete = '#d75f5f' }) do
  vim.api.nvim_set_hl(0, name, { fg = color, default = true })
end
vim.api.nvim_set_hl(0, 'LfMideBlame', { link = 'Comment', default = true })
local signs = {
  [1] = { '▎', 'LfMideAdd' },
  [2] = { '▎', 'LfMideChange' },
  [3] = { '▁', 'LfMideDelete' },
  [4] = { '▔', 'LfMideDelete' },
}
local last = vim.api.nvim_buf_line_count(target)
for _, m in ipairs(marks) do
  local row, sign = m[1], signs[m[2]]
  if sign and row >= 1 and row <= last then
    pcall(vim.api.nvim_buf_set_extmark, target, ns, row - 1, 0,
      { sign_text = sign[1], sign_hl_group = sign[2] })
  end
end
for _, entry in ipairs(blame) do
  local row = entry[1]
  if row >= 1 and row <= last then
    pcall(vim.api.nvim_buf_set_extmark, target, ns, row - 1, 0,
      { virt_text = { { '   ' .. entry[2], 'LfMideBlame' } }, virt_text_pos = 'eol' })
  end
end
return true
"#;

/// Whether the buffer named `path` has changes that are not saved yet:
/// true or false, or nil when that file is not open.
const BUFFER_MODIFIED_LUA: &str = r#"
local path = ...
for _, b in ipairs(vim.api.nvim_list_bufs()) do
  if vim.api.nvim_buf_is_loaded(b) and vim.api.nvim_buf_get_name(b) == path then
    return vim.bo[b].modified
  end
end
return nil
"#;

/// The file shown in the current window, or '' for anything that is not a
/// plain file buffer (help, terminal, nameless).
const CURRENT_FILE_LUA: &str = r#"
if vim.bo.buftype ~= '' then return '' end
return vim.api.nvim_buf_get_name(0)
"#;

/// How a line is marked in the gutter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sign {
    Added = 1,
    Changed = 2,
    DeletedBelow = 3,
    DeletedAbove = 4,
}

#[derive(Debug, Error)]
pub enum EditorError {
    /// Nothing is listening on the socket (not started yet, or closed).
    #[error("Neovim isn't running on {0}")]
    NotRunning(String),
    /// Neovim answered, but could not do what we asked (for example a
    /// modified buffer that can't be abandoned).
    #[error("Neovim: {0}")]
    Rejected(String),
    #[error("Neovim did not answer in time")]
    TimedOut,
    /// The program could not be started.
    #[error("Cannot start Neovim: {0}")]
    Start(String),
}

/// Shows `path` (an absolute path) in the Neovim listening on `socket`, with
/// the cursor on `line` (1-based, clamped to the file) and byte `column`
/// (0-based, clamped to the line) when a line is given. Keeps trying to
/// connect for up to `patience`, since the socket appears a moment after the
/// pane opens. Returns the name of the buffer that is now current.
pub async fn open_file(
    socket: &Path,
    path: &Path,
    line: Option<u32>,
    column: u32,
    patience: Duration,
) -> Result<String, EditorError> {
    let (nvim, io) = connect(socket, patience).await?;
    let line = i64::from(line.unwrap_or(0));
    let args = vec![
        // Raw bytes: exact for any file name, valid UTF-8 or not.
        Value::Binary(path.as_os_str().as_bytes().to_vec()),
        Value::from(line),
        Value::from(i64::from(column)),
    ];
    let outcome = tokio::time::timeout(CALL_TIMEOUT, nvim.exec_lua(OPEN_LUA, args)).await;
    io.abort();
    match outcome {
        Err(_) => Err(EditorError::TimedOut),
        Ok(Err(error)) => Err(EditorError::Rejected(error.to_string())),
        Ok(Ok(name)) => Ok(name.as_str().unwrap_or_default().to_string()),
    }
}

/// Replaces the git signs and blame text on the buffer named `path` (an
/// absolute path) with `signs` and `blame` (`(line, text)`, 1-based lines).
/// Empty lists clear them. `Ok(false)` when that file is not open in Neovim.
pub async fn set_git_marks(
    socket: &Path,
    path: &Path,
    signs: &[(u32, Sign)],
    blame: &[(u32, String)],
    patience: Duration,
) -> Result<bool, EditorError> {
    let (nvim, io) = connect(socket, patience).await?;
    let signs = signs
        .iter()
        .map(|(line, sign)| {
            Value::Array(vec![
                Value::from(i64::from(*line)),
                Value::from(*sign as i64),
            ])
        })
        .collect();
    let blame = blame
        .iter()
        .map(|(line, text)| {
            Value::Array(vec![
                Value::from(i64::from(*line)),
                Value::from(text.as_str()),
            ])
        })
        .collect();
    let args = vec![
        Value::Binary(path.as_os_str().as_bytes().to_vec()),
        Value::Array(signs),
        Value::Array(blame),
    ];
    let outcome =
        tokio::time::timeout(BACKGROUND_TIMEOUT, nvim.exec_lua(GIT_MARKS_LUA, args)).await;
    io.abort();
    match outcome {
        Err(_) => Err(EditorError::TimedOut),
        Ok(Err(error)) => Err(EditorError::Rejected(error.to_string())),
        Ok(Ok(applied)) => Ok(applied.as_bool().unwrap_or(false)),
    }
}

/// `Some(true)` when the open buffer `path` has unsaved changes,
/// `Some(false)` when it is saved, `None` when that file is not open.
pub async fn buffer_modified(
    socket: &Path,
    path: &Path,
    patience: Duration,
) -> Result<Option<bool>, EditorError> {
    let (nvim, io) = connect(socket, patience).await?;
    let args = vec![Value::Binary(path.as_os_str().as_bytes().to_vec())];
    let outcome =
        tokio::time::timeout(BACKGROUND_TIMEOUT, nvim.exec_lua(BUFFER_MODIFIED_LUA, args)).await;
    io.abort();
    match outcome {
        Err(_) => Err(EditorError::TimedOut),
        Ok(Err(error)) => Err(EditorError::Rejected(error.to_string())),
        Ok(Ok(state)) => Ok(state.as_bool()),
    }
}

/// The absolute path of the file in the current window, if it is a file.
pub async fn current_file(
    socket: &Path,
    patience: Duration,
) -> Result<Option<std::path::PathBuf>, EditorError> {
    let (nvim, io) = connect(socket, patience).await?;
    let outcome =
        tokio::time::timeout(BACKGROUND_TIMEOUT, nvim.exec_lua(CURRENT_FILE_LUA, vec![])).await;
    io.abort();
    match outcome {
        Err(_) => Err(EditorError::TimedOut),
        Ok(Err(error)) => Err(EditorError::Rejected(error.to_string())),
        Ok(Ok(name)) => Ok(name
            .as_str()
            .filter(|name| !name.is_empty())
            .map(std::path::PathBuf::from)),
    }
}

/// Connects, retrying every [`RETRY_INTERVAL`] for up to `patience`.
async fn connect(
    socket: &Path,
    patience: Duration,
) -> Result<
    (
        Neovim<Compat<WriteHalf<UnixStream>>>,
        JoinHandle<Result<(), Box<LoopError>>>,
    ),
    EditorError,
> {
    let deadline = Instant::now() + patience;
    loop {
        match new_path(socket, Dummy::new()).await {
            Ok(connected) => return Ok(connected),
            Err(_) if Instant::now() < deadline => tokio::time::sleep(RETRY_INTERVAL).await,
            Err(_) => return Err(EditorError::NotRunning(socket.display().to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Child, Command, Stdio};
    use std::time::Instant;

    /// A real headless Neovim listening on a socket in a temp dir.
    struct Server {
        child: Child,
        dir: tempfile::TempDir,
    }

    impl Server {
        fn socket(&self) -> std::path::PathBuf {
            self.dir.path().join("nvim.sock")
        }
    }

    impl Server {
        fn child_alive(&mut self) -> bool {
            self.child.try_wait().ok().flatten().is_none()
        }
    }

    impl Drop for Server {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    fn nvim_available() -> bool {
        Command::new("nvim")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    }

    fn start_in(dir: tempfile::TempDir) -> Server {
        let socket = dir.path().join("nvim.sock");
        let child = Command::new("nvim")
            .args(["--headless", "--clean", "--listen"])
            .arg(&socket)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let started = Instant::now();
        while !socket.exists() && started.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(20));
        }
        Server { child, dir }
    }

    fn start() -> Server {
        start_in(tempfile::TempDir::new().unwrap())
    }

    /// Runs Lua in the server and returns its result, for checking state.
    async fn lua(socket: &Path, code: &str) -> nvim_rs::Value {
        let handler = nvim_rs::rpc::handler::Dummy::new();
        let (nvim, _io) = nvim_rs::create::tokio::new_path(socket, handler)
            .await
            .unwrap();
        nvim.exec_lua(code, vec![]).await.unwrap()
    }

    const NOW: Duration = Duration::from_secs(3);

    /// Every extmark in the git namespace of the current buffer as text:
    /// `row:sign_text:virt_text`.
    async fn marks_in_current_buffer(socket: &Path) -> Vec<String> {
        let value = lua(
            socket,
            r#"
local ns = vim.api.nvim_create_namespace('lf_mide_git')
local out = {}
for _, m in ipairs(vim.api.nvim_buf_get_extmarks(0, ns, 0, -1, { details = true })) do
  local d = m[4]
  local virt = d.virt_text and d.virt_text[1][1] or ''
  table.insert(out, (m[2] + 1) .. ':' .. (d.sign_text or ''):gsub('%s+$', '') .. ':' .. virt)
end
return table.concat(out, '\n')
"#,
        )
        .await;
        value
            .as_str()
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    async fn open_sample(server: &Server) -> std::path::PathBuf {
        let file = server.dir.path().join("sample.kt");
        std::fs::write(&file, "one\ntwo\nthree\nfour\n").unwrap();
        open_file(&server.socket(), &file, None, 0, NOW)
            .await
            .unwrap();
        std::fs::canonicalize(&file).unwrap()
    }

    #[tokio::test]
    async fn signs_and_blame_appear_on_the_right_lines_of_the_open_file() {
        if !nvim_available() {
            return;
        }
        let server = start();
        let file = open_sample(&server).await;

        let applied = set_git_marks(
            &server.socket(),
            &file,
            &[
                (1, Sign::Added),
                (3, Sign::Changed),
                (4, Sign::DeletedBelow),
            ],
            &[(2, "Ada · 3 days ago · a1b2c3d".to_string())],
            NOW,
        )
        .await
        .unwrap();

        assert!(applied);
        let marks = marks_in_current_buffer(&server.socket()).await;
        assert_eq!(marks.len(), 4, "{marks:?}");
        assert!(marks.contains(&"1:▎:".to_string()), "{marks:?}");
        assert!(marks.contains(&"3:▎:".to_string()));
        assert!(marks.contains(&"4:▁:".to_string()));
        assert!(
            marks
                .iter()
                .any(|m| m.starts_with("2::") && m.contains("Ada · 3 days ago · a1b2c3d"))
        );
    }

    #[tokio::test]
    async fn a_second_call_replaces_the_first_and_empty_lists_clear() {
        if !nvim_available() {
            return;
        }
        let server = start();
        let file = open_sample(&server).await;
        set_git_marks(
            &server.socket(),
            &file,
            &[(1, Sign::Added), (2, Sign::Added)],
            &[],
            NOW,
        )
        .await
        .unwrap();

        set_git_marks(&server.socket(), &file, &[(4, Sign::Changed)], &[], NOW)
            .await
            .unwrap();
        let replaced = marks_in_current_buffer(&server.socket()).await;
        set_git_marks(&server.socket(), &file, &[], &[], NOW)
            .await
            .unwrap();
        let cleared = marks_in_current_buffer(&server.socket()).await;

        assert_eq!(replaced, ["4:▎:"]);
        assert!(cleared.is_empty());
    }

    #[tokio::test]
    async fn lines_beyond_the_file_and_unknown_signs_are_ignored() {
        if !nvim_available() {
            return;
        }
        let server = start();
        let file = open_sample(&server).await;

        set_git_marks(
            &server.socket(),
            &file,
            &[(0, Sign::Added), (99, Sign::Added), (2, Sign::Added)],
            &[(500, "far".into())],
            NOW,
        )
        .await
        .unwrap();

        assert_eq!(marks_in_current_buffer(&server.socket()).await, ["2:▎:"]);
    }

    #[tokio::test]
    async fn a_file_that_is_not_open_is_reported_not_an_error() {
        if !nvim_available() {
            return;
        }
        let server = start();
        open_sample(&server).await;
        let other = server.dir.path().join("closed.kt");
        std::fs::write(&other, "x\n").unwrap();

        let applied = set_git_marks(&server.socket(), &other, &[(1, Sign::Added)], &[], NOW)
            .await
            .unwrap();

        assert!(!applied);
    }

    #[tokio::test]
    async fn hostile_blame_text_is_only_ever_displayed() {
        if !nvim_available() {
            return;
        }
        let mut server = start();
        let file = open_sample(&server).await;
        let hostile = "x']] vim.cmd('qa!') --\" ; :q! `rm -rf ~` %s".to_string();

        let applied = set_git_marks(&server.socket(), &file, &[], &[(1, hostile.clone())], NOW)
            .await
            .unwrap();

        assert!(applied);
        let marks = marks_in_current_buffer(&server.socket()).await;
        assert!(marks[0].contains(&hostile), "{marks:?}");
        assert!(server.child_alive());
    }

    #[tokio::test]
    async fn only_the_exact_file_name_matches_not_a_pattern() {
        if !nvim_available() {
            return;
        }
        let server = start();
        let file = open_sample(&server).await;
        let pattern = file.with_file_name("*.kt");

        let applied = set_git_marks(&server.socket(), &pattern, &[(1, Sign::Added)], &[], NOW)
            .await
            .unwrap();

        assert!(!applied);
    }

    #[tokio::test]
    async fn unsaved_edits_are_reported_per_buffer() {
        if !nvim_available() {
            return;
        }
        let server = start();
        let file = open_sample(&server).await;
        let other = server.dir.path().join("closed.kt");
        std::fs::write(&other, "x\n").unwrap();

        let saved = buffer_modified(&server.socket(), &file, NOW).await.unwrap();
        lua(
            &server.socket(),
            "vim.api.nvim_buf_set_lines(0, 0, 1, false, { 'edited' })",
        )
        .await;
        let edited = buffer_modified(&server.socket(), &file, NOW).await.unwrap();
        let not_open = buffer_modified(&server.socket(), &other, NOW)
            .await
            .unwrap();

        assert_eq!((saved, edited, not_open), (Some(false), Some(true), None));
    }

    #[tokio::test]
    async fn the_current_file_is_reported_and_other_buffers_are_not() {
        if !nvim_available() {
            return;
        }
        let server = start();
        let nothing = current_file(&server.socket(), NOW).await.unwrap();
        let file = open_sample(&server).await;
        let named = current_file(&server.socket(), NOW).await.unwrap();
        lua(&server.socket(), "vim.cmd('help')").await;
        let help = current_file(&server.socket(), NOW).await.unwrap();

        assert_eq!(nothing, None);
        assert_eq!(named.map(|p| std::fs::canonicalize(p).unwrap()), Some(file));
        assert_eq!(help, None, "help is not a file");
    }

    #[tokio::test]
    async fn marks_and_current_file_report_a_missing_server() {
        let missing = Path::new("/nonexistent/nvim.sock");

        let marks = set_git_marks(
            missing,
            Path::new("/x"),
            &[],
            &[],
            Duration::from_millis(100),
        )
        .await;
        let current = current_file(missing, Duration::from_millis(100)).await;

        assert!(matches!(marks, Err(EditorError::NotRunning(_))));
        assert!(matches!(current, Err(EditorError::NotRunning(_))));
    }

    #[tokio::test]
    async fn opens_a_file_and_reports_the_buffer_name() {
        if !nvim_available() {
            return;
        }
        let server = start();
        let file = server.dir.path().join("hello.txt");
        std::fs::write(&file, "one\ntwo\nthree\n").unwrap();

        let name = open_file(&server.socket(), &file, None, 0, NOW)
            .await
            .unwrap();

        assert_eq!(
            std::fs::canonicalize(name).unwrap(),
            std::fs::canonicalize(&file).unwrap()
        );
    }

    #[tokio::test]
    async fn puts_the_cursor_on_the_requested_line_and_column() {
        if !nvim_available() {
            return;
        }
        let server = start();
        let file = server.dir.path().join("lines.txt");
        std::fs::write(&file, "aaaa\nbbbb\ncccccc\ndddd\n").unwrap();

        open_file(&server.socket(), &file, Some(3), 4, NOW)
            .await
            .unwrap();

        let cursor = lua(
            &server.socket(),
            "local c = vim.api.nvim_win_get_cursor(0); return c[1] * 100 + c[2]",
        )
        .await;
        assert_eq!(cursor.as_i64(), Some(304), "line 3, byte column 4");
    }

    #[tokio::test]
    async fn out_of_range_positions_are_clamped_not_errors() {
        if !nvim_available() {
            return;
        }
        let server = start();
        let file = server.dir.path().join("short.txt");
        std::fs::write(&file, "ab\ncd\n").unwrap();

        open_file(&server.socket(), &file, Some(999), 999, NOW)
            .await
            .unwrap();

        let cursor = lua(&server.socket(), "return vim.api.nvim_win_get_cursor(0)[1]").await;
        assert!(cursor.as_i64().is_some_and(|line| (1..=2).contains(&line)));
        let again = open_file(&server.socket(), &file, Some(0), 0, NOW).await;
        assert!(again.is_ok(), "line 0 means no line");
    }

    #[tokio::test]
    async fn opens_files_with_names_that_are_special_to_vim_and_the_shell() {
        if !nvim_available() {
            return;
        }
        let server = start();
        let names = [
            "a b.txt",
            "100%.txt",
            "x#y.txt",
            "q'uote.txt",
            "semi;colon.txt",
            "pipe|name.txt",
            "[brackets]{x}.txt",
            "-dash.txt",
            "$HOME.txt",
            "back`tick`.txt",
            "é日本.txt",
            "tab\there.txt",
            "new\nline.txt",
            "%:h.txt",
            "<cword>.txt",
            "~tilde.txt",
            "*.txt",
            "a\\b.txt",
        ];
        for name in names {
            let file = server.dir.path().join(name);
            std::fs::write(&file, "x\n").unwrap();

            let opened = open_file(&server.socket(), &file, None, 0, NOW)
                .await
                .unwrap();

            let expected = std::fs::canonicalize(&file).unwrap();
            assert_eq!(
                std::fs::canonicalize(&opened).unwrap(),
                expected,
                "for {name:?}"
            );
        }
        // Nothing was created or run as a side effect of the odd names.
        let entries = std::fs::read_dir(server.dir.path()).unwrap().count();
        assert_eq!(
            entries,
            names.len() + 1,
            "the files plus the socket, nothing else"
        );
    }

    #[tokio::test]
    async fn switches_away_from_a_modified_buffer() {
        if !nvim_available() {
            return;
        }
        let server = start();
        let first = server.dir.path().join("first.txt");
        let second = server.dir.path().join("second.txt");
        std::fs::write(&first, "1\n").unwrap();
        std::fs::write(&second, "2\n").unwrap();
        open_file(&server.socket(), &first, None, 0, NOW)
            .await
            .unwrap();
        lua(
            &server.socket(),
            "vim.api.nvim_buf_set_lines(0, 0, -1, false, {'edited'}); return 1",
        )
        .await;

        let result = open_file(&server.socket(), &second, None, 0, NOW).await;

        assert!(result.is_ok(), "{result:?}");
        let modified = lua(&server.socket(), "return vim.fn.getbufinfo({bufloaded = 1})[1] and #vim.fn.getbufinfo({bufmodified = 1}) or 0").await;
        assert_eq!(
            modified.as_i64(),
            Some(1),
            "the edited first buffer is kept, not discarded"
        );
    }

    #[tokio::test]
    async fn reports_a_missing_server_after_the_patience_runs_out() {
        let dir = tempfile::TempDir::new().unwrap();
        let started = Instant::now();

        let result = open_file(
            &dir.path().join("nope.sock"),
            Path::new("/x"),
            None,
            0,
            Duration::from_millis(300),
        )
        .await;

        assert!(
            matches!(result, Err(EditorError::NotRunning(_))),
            "{result:?}"
        );
        assert!(started.elapsed() >= Duration::from_millis(250), "it waited");
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[tokio::test]
    async fn a_stale_non_socket_file_counts_as_not_running() {
        let dir = tempfile::TempDir::new().unwrap();
        let stale = dir.path().join("nvim.sock");
        std::fs::write(&stale, "").unwrap();

        let result = open_file(&stale, Path::new("/x"), None, 0, Duration::from_millis(200)).await;

        assert!(
            matches!(result, Err(EditorError::NotRunning(_))),
            "{result:?}"
        );
    }

    #[tokio::test]
    async fn waits_for_a_server_that_is_still_starting() {
        if !nvim_available() {
            return;
        }
        let dir = tempfile::TempDir::new().unwrap();
        let socket = dir.path().join("nvim.sock");
        let file = dir.path().join("late.txt");
        std::fs::write(&file, "x\n").unwrap();
        let starter = std::thread::spawn({
            let dir_path = dir.path().to_path_buf();
            move || {
                std::thread::sleep(Duration::from_millis(600));
                Command::new("nvim")
                    .args(["--headless", "--clean", "--listen"])
                    .arg(dir_path.join("nvim.sock"))
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .unwrap()
            }
        });

        let result = open_file(&socket, &file, None, 0, Duration::from_secs(8)).await;

        let mut child = starter.join().unwrap();
        let _ = child.kill();
        let _ = child.wait();
        assert!(result.is_ok(), "{result:?}");
    }

    #[tokio::test]
    async fn opening_a_missing_file_still_names_the_new_buffer() {
        if !nvim_available() {
            return;
        }
        let server = start();

        let name = open_file(
            &server.socket(),
            &server.dir.path().join("new.txt"),
            None,
            0,
            NOW,
        )
        .await
        .unwrap();

        assert!(name.ends_with("new.txt"));
    }
}
