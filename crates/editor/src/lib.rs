//! Talking to the Neovim we launched, over its `--listen` socket.

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
