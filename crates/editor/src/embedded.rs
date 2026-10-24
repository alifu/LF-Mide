//! A Neovim that draws inside LF-Mide: started with `--embed`, attached as
//! an `ext_linegrid` UI over its stdin/stdout, and also listening on a
//! socket so opening files, git signs and blame work as with a split pane.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use lf_mide_core::screen::Screen;
use nvim_rs::compat::tokio::Compat;
use nvim_rs::{Handler, Neovim, UiAttachOptions, Value};
use tokio::process::{ChildStdin, Command};
use tokio::sync::mpsc;

use crate::EditorError;
use crate::lsp::{LspServers, SETUP_LUA};
use crate::redraw::{Update, parse};

/// Keys and pastes waiting to be written. Typing never gets near this; a
/// full queue means Neovim has stopped reading.
const QUEUE: usize = 256;

/// Asks before quitting when buffers have unsaved changes: Neovim's own
/// "Save changes?" prompt, answered in the editor. Returns when the user
/// cancels; when Neovim quits, the call never returns.
const CONFIRM_QUIT_LUA: &str = "local ok, err = pcall(vim.cmd, 'confirm qa'); return ok";

/// What starting Neovim needs.
#[derive(Debug, Clone)]
pub struct Launch {
    pub program: OsString,
    /// Extra arguments before the socket options (tests pass `--clean`).
    pub args: Vec<OsString>,
    /// The project root; Neovim starts there.
    pub cwd: PathBuf,
    pub socket: PathBuf,
    pub width: u16,
    pub height: u16,
    /// Language servers to set up, if any.
    pub lsp: Option<LspServers>,
}

/// What the session reports, from its own tasks.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionEvent {
    /// Attached and set up: the socket answers and files can be opened.
    Ready { servers: usize },
    /// Neovim finished drawing; this is what it shows now.
    Screen(Arc<Screen>),
    /// Attaching failed; Neovim is being stopped.
    Failed(String),
    /// The user cancelled quitting in Neovim's prompt.
    QuitCancelled,
    /// The Neovim process has ended.
    Exited,
}

type Report = Arc<dyn Fn(SessionEvent) + Send + Sync>;

#[derive(Debug)]
enum Request {
    Input(String),
    Paste(String),
    Resize(u16, u16),
}

/// A running embedded Neovim. Dropping it stops the writer; [`terminate`]
/// ends the process.
///
/// [`terminate`]: Session::terminate
pub struct Session {
    nvim: Neovim<Compat<ChildStdin>>,
    commands: mpsc::Sender<Request>,
    pid: Option<u32>,
    report: Report,
}

impl Session {
    /// Starts Neovim and attaches to it. Returns once the process runs;
    /// [`SessionEvent::Ready`] or [`SessionEvent::Failed`] follows.
    pub async fn start(
        launch: Launch,
        report: impl Fn(SessionEvent) + Send + Sync + 'static,
    ) -> Result<Session, EditorError> {
        let report: Report = Arc::new(report);
        let handler = RedrawHandler {
            screen: Arc::new(Mutex::new(Screen::default())),
            report: Arc::clone(&report),
        };
        let mut command = Command::new(&launch.program);
        command
            .args(&launch.args)
            .arg("--embed")
            .arg("--listen")
            .arg(&launch.socket)
            .current_dir(&launch.cwd)
            // Anything Neovim prints would land on top of the TUI.
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let (nvim, io, mut child) = nvim_rs::create::tokio::new_child_cmd(&mut command, handler)
            .await
            .map_err(|error| EditorError::Start(error.to_string()))?;
        let pid = child.id();

        let exited = Arc::clone(&report);
        tokio::spawn(async move {
            // The RPC loop ends when Neovim closes its output.
            let _ = io.await;
            let _ = child.wait().await;
            exited(SessionEvent::Exited);
        });

        let (commands, queue) = mpsc::channel(QUEUE);
        tokio::spawn(write_loop(nvim.clone(), launch, queue, Arc::clone(&report)));
        Ok(Session {
            nvim,
            commands,
            pid,
            report,
        })
    }

    /// Sends keys in Neovim notation (see [`crate::keys`]).
    pub fn input(&self, keys: String) -> Result<(), EditorError> {
        self.send(Request::Input(keys))
    }

    /// Pastes text as one undoable change, whatever the mode.
    pub fn paste(&self, text: String) -> Result<(), EditorError> {
        self.send(Request::Paste(text))
    }

    pub fn resize(&self, width: u16, height: u16) -> Result<(), EditorError> {
        self.send(Request::Resize(width, height))
    }

    /// Quits Neovim, asking about unsaved changes in Neovim itself.
    /// [`SessionEvent::QuitCancelled`] follows if the user says no; the
    /// question is answered with normal keys through [`Session::input`].
    pub fn confirm_quit(&self) {
        let nvim = self.nvim.clone();
        let report = Arc::clone(&self.report);
        tokio::spawn(async move {
            // Leave whatever mode the editor is in first, so the prompt is
            // the only thing waiting for keys.
            let _ = nvim.input("<C-\\><C-n>").await;
            if let Ok(Value::Boolean(true)) = nvim.exec_lua(CONFIRM_QUIT_LUA, vec![]).await {
                report(SessionEvent::QuitCancelled);
            }
        });
    }

    /// Asks the process to end now. Neovim keeps its swap files, so unsaved
    /// changes can be recovered next time.
    pub fn terminate(&self) {
        let Some(pid) = self.pid.and_then(|pid| libc::pid_t::try_from(pid).ok()) else {
            return;
        };
        // SAFETY: `kill` has no memory effects; the pid is our own child,
        // which `kill_on_drop` reaps at the latest when the session goes.
        unsafe {
            libc::kill(pid, libc::SIGTERM);
        }
    }

    fn send(&self, command: Request) -> Result<(), EditorError> {
        self.commands
            .try_send(command)
            .map_err(|_| EditorError::TimedOut)
    }
}

/// Attaches, sets up language servers, then writes queued input in order.
async fn write_loop(
    nvim: Neovim<Compat<ChildStdin>>,
    launch: Launch,
    mut queue: mpsc::Receiver<Request>,
    report: Report,
) {
    let mut options = UiAttachOptions::new();
    options.set_rgb(true).set_linegrid_external(true);
    if let Err(error) = nvim
        .ui_attach(i64::from(launch.width), i64::from(launch.height), &options)
        .await
    {
        report(SessionEvent::Failed(format!(
            "Neovim did not start: {error}"
        )));
        return;
    }
    let servers = match &launch.lsp {
        Some(servers) if !servers.is_empty() => nvim
            .exec_lua(SETUP_LUA, servers.setup_args())
            .await
            .ok()
            .and_then(|count| count.as_u64())
            .and_then(|count| usize::try_from(count).ok())
            .unwrap_or(0),
        _ => 0,
    };
    report(SessionEvent::Ready { servers });

    while let Some(command) = queue.recv().await {
        let outcome = match command {
            Request::Input(keys) => nvim.input(&keys).await.map(drop),
            Request::Paste(text) => nvim
                .paste(&normalize_newlines(&text), false, -1)
                .await
                .map(drop),
            Request::Resize(width, height) => {
                nvim.ui_try_resize(i64::from(width), i64::from(height))
                    .await
            }
        };
        if outcome.is_err() {
            // Neovim is gone; the exit is reported by the process watcher.
            return;
        }
    }
}

/// Terminals paste line breaks as `\r`; Neovim wants `\n`.
fn normalize_newlines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// Keeps the screen up to date from redraw notifications and reports a
/// snapshot after each flush. nvim-rs awaits each notification before
/// reading the next, so batches arrive in order.
#[derive(Clone)]
struct RedrawHandler {
    screen: Arc<Mutex<Screen>>,
    report: Report,
}

#[async_trait]
impl Handler for RedrawHandler {
    type Writer = Compat<ChildStdin>;

    async fn handle_notify(&self, name: String, args: Vec<Value>, _nvim: Neovim<Self::Writer>) {
        if name != "redraw" {
            return;
        }
        let mut screen = self
            .screen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for update in parse(&args) {
            match update {
                Update::Event(event) => *screen = std::mem::take(&mut *screen).apply(event),
                Update::Flush => (self.report)(SessionEvent::Screen(Arc::new(screen.clone()))),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pasted_line_breaks_become_newlines() {
        assert_eq!(normalize_newlines("a\r\nb\rc\nd"), "a\nb\nc\nd");
    }
}
