//! A Neovim that draws inside LF-Mide: started with `--embed`, attached as
//! an `ext_linegrid` UI over its stdin/stdout, and also listening on a
//! socket so opening files, git signs and blame work as with a split pane.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use lf_mide_core::screen::Screen;
use lf_mide_core::{EditorMouse, MouseAction, MouseButton};
use nvim_rs::compat::tokio::Compat;
use nvim_rs::{Handler, Neovim, UiAttachOptions, Value};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{mpsc, watch};

use crate::EditorError;
use crate::lsp::{LspServers, SETUP_LUA};
use crate::redraw::{Update, parse};

/// Keys and pastes waiting to be written. Typing never gets near this; a
/// full queue means Neovim has stopped reading.
const QUEUE: usize = 256;

/// After a failed quit call, how long to wait for Neovim to end before
/// treating the call as refused rather than as Neovim quitting.
const QUIT_SETTLE: Duration = Duration::from_secs(1);

/// Neovim's own "Save changes?" prompt, answered in the editor. Returns
/// `{ true }` when the user cancels and `{ false, message }` when Neovim
/// refuses (a write failed, a job still runs); when Neovim quits, the call
/// never returns.
const CONFIRM_QUIT_LUA: &str = r#"
local ok, err = pcall(vim.cmd, 'confirm qa')
return { ok, ok and '' or tostring(err) }
"#;

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
    /// `lsp_error` says why language servers could not be set up.
    Ready {
        servers: usize,
        lsp_error: Option<String>,
    },
    /// Neovim finished drawing; this is what it shows now.
    Screen(Arc<Screen>),
    /// Attaching failed; Neovim is being stopped.
    Failed(String),
    /// Neovim is still running after being asked to quit: the user said
    /// Cancel (`reason` is `None`), or Neovim refused and says why.
    QuitCancelled { reason: Option<String> },
    /// The Neovim process has ended.
    Exited,
}

type Report = Arc<dyn Fn(SessionEvent) + Send + Sync>;

#[derive(Debug)]
enum Request {
    Input(String),
    Paste(String),
    Resize(u16, u16),
    ConfirmQuit,
    Mouse(EditorMouse),
}

/// A running embedded Neovim. Dropping it stops the writer; [`terminate`]
/// ends the process.
///
/// [`terminate`]: Session::terminate
pub struct Session {
    commands: mpsc::Sender<Request>,
    /// Asks the process watcher to send SIGTERM.
    stop: mpsc::UnboundedSender<()>,
    exited: watch::Receiver<bool>,
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
            // Only a backstop: shutting down waits for the SIGTERM to work.
            .kill_on_drop(true);
        let (nvim, io, child) = nvim_rs::create::tokio::new_child_cmd(&mut command, handler)
            .await
            .map_err(|error| EditorError::Start(error.to_string()))?;

        let (stop, stop_requests) = mpsc::unbounded_channel();
        let (exited_tx, exited) = watch::channel(false);
        tokio::spawn(watch_process(
            child,
            io,
            stop_requests,
            exited_tx,
            Arc::clone(&report),
        ));
        let (commands, queue) = mpsc::channel(QUEUE);
        let writer = Writer {
            nvim,
            report,
            stop: stop.clone(),
            exited: exited.clone(),
        };
        tokio::spawn(writer.run(launch, queue));
        Ok(Session {
            commands,
            stop,
            exited,
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

    /// Sends a mouse event to Neovim, at a cell of its grid.
    pub fn mouse(&self, mouse: EditorMouse) -> Result<(), EditorError> {
        self.send(Request::Mouse(mouse))
    }

    /// Quits Neovim, asking about unsaved changes in Neovim itself, after
    /// any keys sent before. [`SessionEvent::QuitCancelled`] follows if it
    /// is still running afterwards; the question is answered with normal
    /// keys through [`Session::input`].
    pub fn confirm_quit(&self) -> Result<(), EditorError> {
        self.send(Request::ConfirmQuit)
    }

    /// Asks the process to end now. Neovim keeps its swap files, so unsaved
    /// changes can be recovered next time.
    pub fn terminate(&self) {
        // Nothing listens once the process has ended, which is fine.
        let _ = self.stop.send(());
    }

    /// Waits up to `patience` for the process to end; true if it has.
    pub async fn wait_exit(&self, patience: Duration) -> bool {
        let mut exited = self.exited.clone();
        tokio::time::timeout(patience, exited.wait_for(|done| *done))
            .await
            .is_ok_and(|seen| seen.is_ok())
    }

    fn send(&self, request: Request) -> Result<(), EditorError> {
        self.commands
            .try_send(request)
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => EditorError::Busy,
                mpsc::error::TrySendError::Closed(_) => {
                    EditorError::NotRunning("the embedded editor".to_string())
                }
            })
    }
}

/// Owns the process: reports when it ends, and is the only one to signal
/// it, so a pid can never be signalled after it was reaped and reused.
async fn watch_process(
    mut child: Child,
    mut io: tokio::task::JoinHandle<Result<(), Box<nvim_rs::error::LoopError>>>,
    mut stop_requests: mpsc::UnboundedReceiver<()>,
    exited: watch::Sender<bool>,
    report: Report,
) {
    let mut io_done = false;
    loop {
        tokio::select! {
            outcome = &mut io, if !io_done => {
                io_done = true;
                if let Ok(Err(error)) = outcome {
                    tracing::debug!(%error, "the Neovim connection ended");
                }
            }
            status = child.wait() => {
                if let Err(error) = status {
                    tracing::warn!(%error, "cannot wait for Neovim");
                }
                break;
            }
            Some(()) = stop_requests.recv() => signal_term(&child),
        }
    }
    // Let the last redraws through first, so Exited is the final event.
    if !io_done {
        let _ = tokio::time::timeout(QUIT_SETTLE, &mut io).await;
    }
    let _ = exited.send(true);
    report(SessionEvent::Exited);
}

fn signal_term(child: &Child) {
    // `id` is None once the child has been reaped.
    let Some(pid) = child.id().and_then(|pid| libc::pid_t::try_from(pid).ok()) else {
        return;
    };
    // SAFETY: `kill` has no memory effects, and the child has not been
    // reaped (this task is the only one that waits for it), so the pid is
    // still ours.
    if unsafe { libc::kill(pid, libc::SIGTERM) } != 0 {
        tracing::debug!(error = %std::io::Error::last_os_error(), "cannot stop Neovim");
    }
}

/// Attaches, sets up language servers, then writes queued requests in order.
struct Writer {
    nvim: Neovim<Compat<ChildStdin>>,
    report: Report,
    stop: mpsc::UnboundedSender<()>,
    exited: watch::Receiver<bool>,
}

impl Writer {
    async fn run(self, launch: Launch, mut queue: mpsc::Receiver<Request>) {
        let mut options = UiAttachOptions::new();
        options.set_rgb(true).set_linegrid_external(true);
        if let Err(error) = self
            .nvim
            .ui_attach(i64::from(launch.width), i64::from(launch.height), &options)
            .await
        {
            (self.report)(SessionEvent::Failed(format!(
                "Neovim did not start: {error}"
            )));
            let _ = self.stop.send(());
            return;
        }
        // Clicks and the wheel reach Neovim only with the mouse on. A failure
        // only costs the mouse, so it is logged.
        if let Err(error) = self.nvim.command("set mouse=a").await {
            tracing::warn!(%error, "cannot turn on the mouse in Neovim");
        }
        let (servers, lsp_error) = self.set_up_lsp(launch.lsp.as_ref()).await;
        (self.report)(SessionEvent::Ready { servers, lsp_error });

        while let Some(request) = queue.recv().await {
            let outcome = match request {
                Request::Input(keys) => self.nvim.input(&keys).await.map(drop),
                Request::Paste(text) => self
                    .nvim
                    .paste(&normalize_newlines(&text), false, -1)
                    .await
                    .map(drop),
                Request::Resize(width, height) => {
                    self.nvim
                        .ui_try_resize(i64::from(width), i64::from(height))
                        .await
                }
                Request::Mouse(mouse) => {
                    let (button, action) = nvim_names(mouse.action);
                    let (row, col) = (i64::from(mouse.row), i64::from(mouse.col));
                    self.nvim.input_mouse(button, action, "", 0, row, col).await
                }
                Request::ConfirmQuit => self.confirm_quit().await,
            };
            if let Err(error) = outcome {
                // Neovim is gone; the exit is reported by the process watcher.
                tracing::debug!(%error, "stopped writing to Neovim");
                return;
            }
        }
    }

    async fn set_up_lsp(&self, servers: Option<&LspServers>) -> (usize, Option<String>) {
        let Some(servers) = servers.filter(|servers| !servers.is_empty()) else {
            return (0, None);
        };
        match self.nvim.exec_lua(SETUP_LUA, servers.setup_args()).await {
            Ok(count) => (
                count
                    .as_u64()
                    .and_then(|count| usize::try_from(count).ok())
                    .unwrap_or(0),
                None,
            ),
            Err(error) => {
                tracing::warn!(%error, "cannot set up language servers");
                (0, Some(error.to_string()))
            }
        }
    }

    /// Leaves whatever mode the editor is in, then asks Neovim to quit. The
    /// question is answered with keys sent later, so the call is awaited on
    /// its own task (Neovim reads input while the prompt waits).
    async fn confirm_quit(&self) -> Result<(), Box<nvim_rs::error::CallError>> {
        self.nvim.input("<C-\\><C-n>").await?;
        let nvim = self.nvim.clone();
        let report = Arc::clone(&self.report);
        let mut exited = self.exited.clone();
        tokio::spawn(async move {
            let outcome = nvim.exec_lua(CONFIRM_QUIT_LUA, vec![]).await;
            let reason = match outcome {
                Ok(answer) => refusal(&answer),
                Err(error) => {
                    // Quitting closes the connection mid-call: only a call
                    // that failed while Neovim stays up is a refusal.
                    let quitting = tokio::time::timeout(QUIT_SETTLE, exited.wait_for(|done| *done))
                        .await
                        .is_ok();
                    if quitting {
                        return;
                    }
                    Some(error.to_string())
                }
            };
            report(SessionEvent::QuitCancelled { reason });
        });
        Ok(())
    }
}

/// `None` when the user cancelled, else Neovim's reason for not quitting.
fn refusal(answer: &Value) -> Option<String> {
    let parts = answer.as_array().map(Vec::as_slice).unwrap_or_default();
    if parts.first().and_then(Value::as_bool) == Some(true) {
        return None;
    }
    let message = parts
        .get(1)
        .and_then(Value::as_str)
        .unwrap_or("Neovim did not quit");
    // `[string ...]:353: nvim_exec2(), line 1: Vim(qall):E212: ...`: the
    // part after the command name is what the user needs.
    let short = message
        .rsplit_once("):")
        .map_or(message, |(_, rest)| rest)
        .trim();
    Some(short.to_string())
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

/// The button and action names `nvim_input_mouse` takes.
fn nvim_names(action: MouseAction) -> (&'static str, &'static str) {
    match action {
        MouseAction::Press(button) => (button_name(button), "press"),
        MouseAction::Drag(button) => (button_name(button), "drag"),
        MouseAction::Release(button) => (button_name(button), "release"),
        MouseAction::ScrollUp => ("wheel", "up"),
        MouseAction::ScrollDown => ("wheel", "down"),
    }
}

fn button_name(button: MouseButton) -> &'static str {
    match button {
        MouseButton::Left => "left",
        MouseButton::Right => "right",
        MouseButton::Middle => "middle",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cancel_has_no_reason_and_a_refusal_says_why() {
        let cancelled = Value::Array(vec![true.into(), "".into()]);
        let refused = Value::Array(vec![
            false.into(),
            "[string \"vim/_core/editor\"]:353: nvim_exec2(), line 1: Vim(qall):E212: Can't open file for writing".into(),
        ]);

        assert_eq!(refusal(&cancelled), None);
        assert_eq!(
            refusal(&refused).as_deref(),
            Some("E212: Can't open file for writing")
        );
        assert_eq!(refusal(&Value::Nil).as_deref(), Some("Neovim did not quit"));
    }

    #[test]
    fn pasted_line_breaks_become_newlines() {
        assert_eq!(normalize_newlines("a\r\nb\rc\nd"), "a\nb\nc\nd");
    }
}

#[cfg(test)]
mod mouse_tests {
    use super::*;

    #[test]
    fn mouse_events_are_named_the_way_neovim_expects() {
        assert_eq!(
            nvim_names(MouseAction::Press(MouseButton::Right)),
            ("right", "press")
        );
        assert_eq!(
            nvim_names(MouseAction::Drag(MouseButton::Left)),
            ("left", "drag")
        );
        assert_eq!(
            nvim_names(MouseAction::Release(MouseButton::Middle)),
            ("middle", "release")
        );
        assert_eq!(nvim_names(MouseAction::ScrollUp), ("wheel", "up"));
        assert_eq!(nvim_names(MouseAction::ScrollDown), ("wheel", "down"));
    }
}
