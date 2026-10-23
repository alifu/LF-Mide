//! Runs a program and hands its output over line by line while it runs (a
//! build, `logcat`), instead of collecting everything at the end.
//!
//! stdout and stderr are merged into one ordered stream. The channel is
//! bounded: when the consumer falls behind, the readers block, the pipe
//! fills and the child waits, so memory stays flat however chatty it is.

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::time::Duration;

use crate::{RunError, kill_group};

/// Lines waiting between the readers and the consumer.
const CHANNEL_LINES: usize = 2048;
/// Longest line kept; the rest of a longer one is dropped.
pub const MAX_LINE_BYTES: usize = 4096;
/// Most lines handed over by one [`Stream::next`] call.
pub const MAX_BATCH_LINES: usize = 256;
/// How long output may still arrive after the program has exited (a
/// background process it started may hold the pipes open).
const DRAIN_GRACE: Duration = Duration::from_millis(500);
/// Time a program gets to react to SIGTERM before it is killed.
const TERMINATE_GRACE: Duration = Duration::from_secs(3);
const READ_CHUNK: usize = 8192;

/// What to run.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StreamSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    /// Added to the inherited environment.
    pub env: Vec<(String, String)>,
}

/// How the program ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Exit {
    /// The exit code; `None` when it was ended by a signal.
    pub code: Option<i32>,
}

impl Exit {
    pub fn success(self) -> bool {
        self.code == Some(0)
    }
}

/// One step of consuming a [`Stream`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Output lines, in order.
    Lines(Vec<String>),
    /// The program has ended and all its output was delivered.
    Exited(Exit),
    /// Nothing happened within the wait.
    Idle,
}

enum Item {
    Line(String),
    Exit(Exit),
}

/// A running program.
pub struct Stream {
    rx: Receiver<Item>,
    stashed_exit: Option<Exit>,
    canceller: Canceller,
}

/// Stops a [`Stream`]'s program from any thread.
#[derive(Clone, Debug)]
pub struct Canceller {
    group: libc::pid_t,
    exited: Arc<AtomicBool>,
    cancelled: Arc<AtomicBool>,
}

impl Canceller {
    /// Kills the program and everything in its process group at once, even
    /// if the leader has already gone.
    pub fn kill_now(&self) {
        kill_group(self.group);
    }

    /// Asks the program (and what it started) to stop, then forces it after
    /// a few seconds. Does nothing once it has exited.
    pub fn cancel(&self) {
        if self.exited.load(Ordering::SeqCst) || self.cancelled.swap(true, Ordering::SeqCst) {
            return;
        }
        // SAFETY: plain signal to a process group we created.
        unsafe { libc::kill(-self.group, libc::SIGTERM) };
        let group = self.group;
        std::thread::spawn(move || {
            std::thread::sleep(TERMINATE_GRACE);
            // Even when the leader is gone, a member that ignored SIGTERM
            // keeps the group id alive; an empty group just answers ESRCH.
            kill_group(group);
        });
    }
}

/// Starts `spec`. The program gets its own process group so that
/// [`Canceller::cancel`] reaches everything it started.
pub fn start(spec: &StreamSpec) -> Result<Stream, RunError> {
    let mut command = Command::new(&spec.program);
    // One pipe for both outputs keeps their lines in the order written.
    let (reader, writer) = std::io::pipe().map_err(|err| RunError::Other(err.to_string()))?;
    let writer_for_errors = writer
        .try_clone()
        .map_err(|err| RunError::Other(err.to_string()))?;
    command
        .args(&spec.args)
        .envs(spec.env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::null())
        .stdout(writer)
        .stderr(writer_for_errors)
        .process_group(0);
    if let Some(cwd) = &spec.cwd {
        command.current_dir(cwd);
    }
    let spawned = command.spawn();
    // The child holds its own copies; ours must go or the pipe never closes.
    drop(command);
    let mut child = spawned.map_err(|err| match err.kind() {
        std::io::ErrorKind::NotFound => RunError::NotFound,
        _ => RunError::Other(err.to_string()),
    })?;
    let group = child.id() as libc::pid_t;
    let (tx, rx) = mpsc::sync_channel::<Item>(CHANNEL_LINES);
    let (done_tx, done_rx) = mpsc::channel::<()>();
    let progress = Arc::new(ReaderProgress::default());
    read_lines(reader, tx.clone(), done_tx, Arc::clone(&progress));
    let exited = Arc::new(AtomicBool::new(false));
    let waiter_exited = Arc::clone(&exited);
    std::thread::spawn(move || {
        let status = child.wait();
        waiter_exited.store(true, Ordering::SeqCst);
        wait_for_reader(&done_rx, &progress);
        let code = status.ok().and_then(|s| s.code());
        let _ = tx.send(Item::Exit(Exit { code }));
    });
    Ok(Stream {
        rx,
        stashed_exit: None,
        canceller: Canceller {
            group,
            exited,
            cancelled: Arc::new(AtomicBool::new(false)),
        },
    })
}

impl Stream {
    pub fn canceller(&self) -> Canceller {
        self.canceller.clone()
    }

    /// Waits up to `wait` for output, then returns what has arrived (up to
    /// [`MAX_BATCH_LINES`] lines) or the end of the program.
    pub fn next(&mut self, wait: Duration) -> Event {
        if let Some(exit) = self.stashed_exit.take() {
            return Event::Exited(exit);
        }
        let first = match self.rx.recv_timeout(wait) {
            Ok(item) => item,
            Err(RecvTimeoutError::Timeout) => return Event::Idle,
            Err(RecvTimeoutError::Disconnected) => return Event::Exited(Exit { code: None }),
        };
        let mut lines = match first {
            Item::Exit(exit) => return Event::Exited(exit),
            Item::Line(line) => vec![line],
        };
        while lines.len() < MAX_BATCH_LINES {
            match self.rx.try_recv() {
                Ok(Item::Line(line)) => lines.push(line),
                Ok(Item::Exit(exit)) => {
                    self.stashed_exit = Some(exit);
                    break;
                }
                Err(_) => break,
            }
        }
        Event::Lines(lines)
    }
}

/// What the reader thread is doing, as seen by the thread waiting for it.
#[derive(Default)]
struct ReaderProgress {
    /// Chunks read from the pipe so far.
    reads: AtomicU64,
    /// Waiting for room in the channel: the consumer is slow, not the pipe.
    sending: AtomicBool,
}

/// Waits until the reader has delivered everything the program wrote.
/// A reader stuck on a pipe that a background process keeps open (no data
/// for [`DRAIN_GRACE`]) is given up on; one that is still reading, or
/// waiting for a slow consumer, is not.
fn wait_for_reader(done: &mpsc::Receiver<()>, progress: &ReaderProgress) {
    let mut seen = progress.reads.load(Ordering::SeqCst);
    loop {
        match done.recv_timeout(DRAIN_GRACE) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) => {
                let now = progress.reads.load(Ordering::SeqCst);
                if !progress.sending.load(Ordering::SeqCst) && now == seen {
                    return;
                }
                seen = now;
            }
        }
    }
}

/// Reads `pipe` on its own thread and sends each line, until it closes or
/// the consumer is gone.
fn read_lines<R: Read + Send + 'static>(
    pipe: R,
    tx: SyncSender<Item>,
    done: mpsc::Sender<()>,
    progress: Arc<ReaderProgress>,
) {
    std::thread::spawn(move || {
        let mut lines = LineSplitter::default();
        let mut pipe = pipe;
        let mut chunk = [0u8; READ_CHUNK];
        'read: loop {
            let read = match pipe.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(read) => read,
            };
            progress.reads.fetch_add(1, Ordering::SeqCst);
            for line in lines.push(&chunk[..read]) {
                progress.sending.store(true, Ordering::SeqCst);
                let sent = tx.send(Item::Line(line));
                progress.sending.store(false, Ordering::SeqCst);
                if sent.is_err() {
                    break 'read;
                }
            }
        }
        if let Some(rest) = lines.finish() {
            let _ = tx.send(Item::Line(rest));
        }
        let _ = done.send(());
    });
}

/// Cuts a byte stream into lines of at most [`MAX_LINE_BYTES`].
#[derive(Default)]
struct LineSplitter {
    line: Vec<u8>,
    truncated: bool,
    /// The previous byte was `\r`.
    after_return: bool,
}

impl LineSplitter {
    fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        let mut done = Vec::new();
        for &byte in bytes {
            let redraw = std::mem::take(&mut self.after_return);
            if byte == b'\n' {
                done.push(self.take());
                continue;
            }
            if redraw {
                // A progress display redraws with `\r`: only the last state
                // of the line matters.
                self.line.clear();
                self.truncated = false;
            }
            if byte == b'\r' {
                self.after_return = true;
            } else if self.line.len() < MAX_LINE_BYTES {
                self.line.push(byte);
            } else {
                self.truncated = true;
            }
        }
        done
    }

    fn finish(&mut self) -> Option<String> {
        (!self.line.is_empty()).then(|| self.take())
    }

    fn take(&mut self) -> String {
        self.after_return = false;
        let bytes = std::mem::take(&mut self.line);
        let mut text = String::from_utf8_lossy(&bytes).into_owned();
        if std::mem::take(&mut self.truncated) {
            text.push('…');
        }
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn sh(script: &str) -> StreamSpec {
        StreamSpec {
            program: "sh".into(),
            args: vec!["-c".into(), script.into()],
            ..StreamSpec::default()
        }
    }

    /// Everything the program prints, and how it ended.
    fn collect(spec: &StreamSpec) -> (Vec<String>, Exit) {
        let mut stream = start(spec).unwrap();
        let mut all = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            match stream.next(Duration::from_millis(100)) {
                Event::Lines(lines) => all.extend(lines),
                Event::Exited(exit) => return (all, exit),
                Event::Idle => assert!(Instant::now() < deadline, "stream never ended: {all:?}"),
            }
        }
    }

    #[test]
    fn delivers_stdout_and_stderr_lines_in_order_then_the_exit() {
        let (lines, exit) = collect(&sh("echo one; echo two 1>&2; echo three; exit 3"));

        assert_eq!(lines, ["one", "two", "three"]);
        assert_eq!(exit, Exit { code: Some(3) });
        assert!(!exit.success());
    }

    #[test]
    fn a_last_line_without_a_newline_is_not_lost() {
        let (lines, exit) = collect(&sh("printf 'a\\nb'"));

        assert_eq!(lines, ["a", "b"]);
        assert!(exit.success());
    }

    #[test]
    fn a_missing_program_is_reported_at_start() {
        let spec = StreamSpec {
            program: "/definitely/not/here".into(),
            ..StreamSpec::default()
        };

        assert!(matches!(start(&spec), Err(RunError::NotFound)));
    }

    #[test]
    fn runs_in_the_given_directory_with_the_given_environment() {
        let dir = tempfile::tempdir().unwrap();
        let mut spec = sh("pwd; echo \"$LF_TEST_VALUE\"");
        spec.cwd = Some(dir.path().to_path_buf());
        spec.env = vec![("LF_TEST_VALUE".into(), "hello".into())];

        let (lines, _) = collect(&spec);

        assert_eq!(lines.len(), 2);
        assert!(lines[0].ends_with(dir.path().file_name().unwrap().to_str().unwrap()));
        assert_eq!(lines[1], "hello");
    }

    #[test]
    fn a_very_long_line_is_cut_and_marked() {
        let (lines, _) = collect(&sh(
            "head -c 20000 /dev/zero | tr '\\0' x; echo; echo after",
        ));

        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].chars().count(), MAX_LINE_BYTES + 1);
        assert!(lines[0].ends_with('…'));
        assert_eq!(lines[1], "after");
    }

    #[test]
    fn a_progress_redraw_keeps_only_the_last_state() {
        let (lines, _) = collect(&sh("printf '10%%\\r50%%\\r100%%\\nnext\\n'"));

        assert_eq!(lines, ["100%", "next"]);
    }

    #[test]
    fn a_long_progress_stream_without_newlines_keeps_following_the_redraws() {
        let mut splitter = LineSplitter::default();
        let mut input = Vec::new();
        for percent in 0..2000 {
            input.extend(format!("progress {percent}%\r").bytes());
        }
        input.extend(b"done\n");

        let lines = splitter.push(&input);

        assert_eq!(lines, ["done"]);
    }

    #[test]
    fn carriage_return_line_feed_ends_a_line_normally() {
        let mut splitter = LineSplitter::default();

        assert_eq!(splitter.push(b"one\r\ntwo\r\n"), ["one", "two"]);
    }

    #[test]
    fn invalid_utf8_is_replaced_not_fatal() {
        let (lines, _) = collect(&sh("printf 'ok\\377ok\\n'"));

        assert_eq!(lines, ["ok\u{fffd}ok"]);
    }

    #[test]
    fn lots_of_output_arrives_complete_and_in_batches() {
        let (lines, exit) = collect(&sh("seq 1 20000"));

        assert_eq!(lines.len(), 20000);
        assert_eq!(lines[0], "1");
        assert_eq!(lines[19999], "20000");
        assert!(exit.success());
    }

    #[test]
    fn cancel_stops_the_program_and_what_it_started() {
        let mut stream = start(&sh("sleep 600 & echo started; wait")).unwrap();
        let canceller = stream.canceller();
        assert!(matches!(
            stream.next(Duration::from_secs(5)),
            Event::Lines(_)
        ));

        canceller.cancel();

        let deadline = Instant::now() + Duration::from_secs(10);
        let exit = loop {
            match stream.next(Duration::from_millis(100)) {
                Event::Exited(exit) => break exit,
                _ => assert!(Instant::now() < deadline, "cancel did not stop it"),
            }
        };
        assert!(!exit.success());
    }

    #[test]
    fn cancel_after_the_program_ended_is_harmless() {
        let mut stream = start(&sh("echo hi")).unwrap();
        let canceller = stream.canceller();
        while !matches!(stream.next(Duration::from_millis(100)), Event::Exited(_)) {}

        canceller.cancel();
        canceller.cancel();
    }

    #[test]
    fn a_slow_consumer_still_gets_every_line_after_the_program_has_exited() {
        let mut stream = start(&sh("yes x | head -n 7000")).unwrap();
        // Short lines: it all fits in the pipe, so the program exits at once
        // while far more lines than the channel holds wait to be delivered.
        // Longer than the drain grace: the program is long gone by now and
        // its output is still waiting for room in the channel.
        std::thread::sleep(DRAIN_GRACE * 3);
        let mut all = Vec::new();

        let exit = loop {
            match stream.next(Duration::from_millis(100)) {
                Event::Lines(lines) => all.extend(lines),
                Event::Exited(exit) => break exit,
                Event::Idle => {}
            }
        };

        assert_eq!(all.len(), 7000);
        assert!(exit.success());
    }

    #[test]
    fn a_background_process_holding_the_pipe_does_not_hold_up_the_exit() {
        let started = Instant::now();

        let (lines, exit) = collect(&sh("(sleep 30 &) ; echo done"));

        assert_eq!(lines, ["done"]);
        assert!(exit.success());
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn a_signal_death_has_no_exit_code() {
        let (_, exit) = collect(&sh("kill -9 $$"));

        assert_eq!(exit, Exit { code: None });
    }
}
