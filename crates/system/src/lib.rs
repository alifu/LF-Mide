//! Everything LF-Mide observes or runs on the machine, behind one trait so
//! tests can substitute canned command output and files.

#[cfg(any(test, feature = "testing"))]
pub mod fake;
pub mod stream;

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(15);
/// Output beyond this is read and discarded so the child never blocks.
pub const MAX_OUTPUT_BYTES: usize = 1024 * 1024;
/// Repo files (properties) are small; refuse to slurp more than this.
pub const MAX_READ_BYTES: usize = 64 * 1024;
/// After the child exits, how long to wait for its pipes to close.
const PIPE_GRACE: Duration = Duration::from_millis(500);
const POLL_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    /// stdout and stderr joined, trimmed. Some tools (`java -version`) print
    /// their version to stderr.
    pub fn text(&self) -> String {
        format!("{}\n{}", self.stdout, self.stderr)
            .trim()
            .to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunError {
    NotFound,
    TimedOut,
    Other(String),
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunError::NotFound => f.write_str("program not found"),
            RunError::TimedOut => f.write_str("timed out"),
            RunError::Other(message) => f.write_str(message),
        }
    }
}

pub trait System: Sync {
    /// Runs `program` and waits for it, up to the implementation's timeout.
    fn run(&self, program: &str, args: &[&str]) -> Result<Output, RunError>;
    /// Like [`run`](Self::run), in the folder `dir`. Lets a command work on
    /// the files there by relative name, so a folder whose path is not valid
    /// text never has to be written out as an argument.
    fn run_in(&self, dir: &Path, program: &str, args: &[&str]) -> Result<Output, RunError> {
        let _ = (dir, program, args);
        Err(RunError::Other(
            "running in a folder is not supported".to_string(),
        ))
    }
    /// Starts `program` and returns at once, leaving it running on its own
    /// (an emulator, for example). Its output is discarded; it is not
    /// stopped when LF-Mide exits.
    fn spawn(&self, program: &str, args: &[&str]) -> Result<(), RunError>;
    fn env(&self, key: &str) -> Option<String>;
    fn exists(&self, path: &Path) -> bool;
    fn is_dir(&self, path: &Path) -> bool;
    /// Entry names (not paths) of a directory, sorted; empty if unreadable.
    fn list_dir(&self, path: &Path) -> Vec<String>;
    fn read_to_string(&self, path: &Path) -> Option<String>;
    /// `path` with symlinks (e.g. macOS `/var` → `/private/var`) resolved;
    /// `None` if it doesn't exist.
    fn canonicalize(&self, path: &Path) -> Option<PathBuf>;
    fn home(&self) -> Option<PathBuf>;

    /// Memory the process really holds (macOS "physical footprint", what
    /// Activity Monitor shows), in bytes. Unlike resident size it does not
    /// count shared pages once per process. `None` when unknown or not ours
    /// to ask about.
    fn footprint(&self, _pid: u32) -> Option<u64> {
        None
    }

    /// First match for `name` on `$PATH`.
    fn which(&self, name: &str) -> Option<PathBuf> {
        let path = self.env("PATH")?;
        // Relative entries (`.`, empty) would run whatever the current
        // directory contains.
        std::env::split_paths(&path)
            .filter(|dir| dir.is_absolute())
            .map(|dir| dir.join(name))
            .find(|candidate| self.exists(candidate))
    }
}

/// Reads one pipe on its own thread so a full pipe never blocks the child.
/// Keeps what was read even if the pipe never closes.
struct Capture {
    buffer: Arc<Mutex<Vec<u8>>>,
    closed: Receiver<()>,
}

impl Capture {
    fn start<R: Read + Send + 'static>(pipe: Option<R>) -> Self {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let (tx, closed) = mpsc::channel();
        let sink = Arc::clone(&buffer);
        std::thread::spawn(move || {
            if let Some(mut pipe) = pipe {
                let mut chunk = [0u8; 8192];
                while let Ok(read) = pipe.read(&mut chunk) {
                    if read == 0 {
                        break;
                    }
                    let mut bytes = sink.lock().unwrap_or_else(PoisonError::into_inner);
                    // Past the cap we keep reading (and dropping) so the child can finish.
                    let room = MAX_OUTPUT_BYTES.saturating_sub(bytes.len());
                    bytes.extend_from_slice(&chunk[..read.min(room)]);
                }
            }
            let _ = tx.send(());
        });
        Self { buffer, closed }
    }

    /// The text read so far, and whether the pipe reached EOF by `deadline`.
    fn finish(self, deadline: Instant) -> (String, bool) {
        let wait = deadline.saturating_duration_since(Instant::now());
        let closed = self.closed.recv_timeout(wait).is_ok();
        let bytes = self.buffer.lock().unwrap_or_else(PoisonError::into_inner);
        (String::from_utf8_lossy(&bytes).into_owned(), closed)
    }
}

/// SIGKILLs every process in `group` (ignored if it is already gone).
fn kill_group(group: libc::pid_t) {
    // SAFETY: `killpg` takes plain integers and has no memory-safety preconditions.
    unsafe {
        libc::killpg(group, libc::SIGKILL);
    }
}

/// Process groups of the commands `SystemRunner::run` is waiting for right
/// now, so that quitting can stop them instead of leaving them behind (a
/// search that was still running, say).
static RUNNING: Mutex<Vec<libc::pid_t>> = Mutex::new(Vec::new());
/// Set once the program is quitting: no new command may start, so nothing
/// can slip in after the sweep and be left running.
static SHUTTING_DOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Lets commands start again (only for tests of the shutdown itself).
#[doc(hidden)]
pub fn allow_commands_again() {
    SHUTTING_DOWN.store(false, std::sync::atomic::Ordering::SeqCst);
}

fn shutting_down() -> bool {
    SHUTTING_DOWN.load(std::sync::atomic::Ordering::SeqCst)
}
/// Grace between asking commands to stop and forcing them.
const TERMINATE_GRACE: Duration = Duration::from_millis(150);

/// A command that is running, listed in [`RUNNING`] until it is dropped.
struct RunningCommand {
    group: libc::pid_t,
}

impl RunningCommand {
    fn register(group: libc::pid_t) -> Self {
        RUNNING
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(group);
        Self { group }
    }
}

impl Drop for RunningCommand {
    fn drop(&mut self) {
        RUNNING
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|group| *group != self.group);
    }
}

/// How many commands started by [`SystemRunner::run`] are running now.
pub fn running_commands() -> usize {
    RUNNING.lock().unwrap_or_else(|e| e.into_inner()).len()
}

/// Stops every command [`SystemRunner::run`] is waiting for: asks, waits a
/// moment, then forces. For the end of the program. Commands started with
/// `spawn` (an emulator) are meant to outlive LF-Mide and are not touched.
pub fn terminate_running_commands() {
    SHUTTING_DOWN.store(true, std::sync::atomic::Ordering::SeqCst);
    let groups: Vec<libc::pid_t> = RUNNING.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if groups.is_empty() {
        return;
    }
    for group in &groups {
        // SAFETY: plain signal to a process group this module created.
        unsafe { libc::killpg(*group, libc::SIGTERM) };
    }
    std::thread::sleep(TERMINATE_GRACE);
    // Only the ones that are still listed: a finished command has been
    // reaped and its number could belong to something else by now.
    let still: Vec<libc::pid_t> = RUNNING.lock().unwrap_or_else(|e| e.into_inner()).clone();
    for group in groups.into_iter().filter(|g| still.contains(g)) {
        kill_group(group);
    }
}

/// The real machine.
pub struct SystemRunner {
    timeout: Duration,
}

impl SystemRunner {
    pub fn new() -> Self {
        Self::with_timeout(COMMAND_TIMEOUT)
    }

    pub fn with_timeout(timeout: Duration) -> Self {
        Self { timeout }
    }
}

impl Default for SystemRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemRunner {
    /// Runs `program`, in `dir` when given, and waits for it.
    fn run_command(
        &self,
        dir: Option<&Path>,
        program: &str,
        args: &[&str],
    ) -> Result<Output, RunError> {
        if shutting_down() {
            return Err(RunError::Other("LF-Mide is shutting down".to_string()));
        }
        let mut command = Command::new(program);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Own process group, so a timeout can stop grandchildren too.
            .process_group(0);
        if let Some(dir) = dir {
            // A folder that has gone is not "program not found".
            if !dir.is_dir() {
                return Err(RunError::Other(format!(
                    "{} is not a folder",
                    dir.display()
                )));
            }
            command.current_dir(dir);
        }
        let mut child = command.spawn().map_err(|err| match err.kind() {
            std::io::ErrorKind::NotFound => RunError::NotFound,
            _ => RunError::Other(err.to_string()),
        })?;
        let group = child.id() as libc::pid_t;
        let listed = RunningCommand::register(group);
        // Quitting began while this was starting: the sweep may have missed it.
        if shutting_down() {
            kill_group(group);
        }
        let stdout = Capture::start(child.stdout.take());
        let stderr = Capture::start(child.stderr.take());

        let deadline = Instant::now() + self.timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    // Reaped: its number may be reused from now on.
                    drop(listed);
                    break status;
                }
                Ok(None) if Instant::now() >= deadline => {
                    kill_group(group);
                    let _ = child.wait();
                    return Err(RunError::TimedOut);
                }
                Ok(None) => std::thread::sleep(POLL_INTERVAL),
                Err(err) => {
                    kill_group(group);
                    let _ = child.wait();
                    return Err(RunError::Other(err.to_string()));
                }
            }
        };

        // The child is done, but a background process it started may still
        // hold the pipes open. Wait briefly and return what was read. We do
        // not kill leftovers: the child is already reaped, so its group id
        // could by now belong to an unrelated process.
        let grace = Instant::now() + PIPE_GRACE;
        let (stdout, _) = stdout.finish(grace);
        let (stderr, _) = stderr.finish(grace);
        Ok(Output {
            success: status.success(),
            stdout,
            stderr,
        })
    }
}

impl System for SystemRunner {
    fn run(&self, program: &str, args: &[&str]) -> Result<Output, RunError> {
        self.run_command(None, program, args)
    }

    fn run_in(&self, dir: &Path, program: &str, args: &[&str]) -> Result<Output, RunError> {
        self.run_command(Some(dir), program, args)
    }

    fn spawn(&self, program: &str, args: &[&str]) -> Result<(), RunError> {
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            // Its own process group: Ctrl-C in our terminal must not stop it.
            .process_group(0)
            .spawn()
            .map_err(|err| match err.kind() {
                std::io::ErrorKind::NotFound => RunError::NotFound,
                _ => RunError::Other(err.to_string()),
            })?;
        // Reap it when it ends so it never lingers as a zombie.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }

    fn env(&self, key: &str) -> Option<String> {
        std::env::var(key).ok()
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn is_dir(&self, path: &Path) -> bool {
        path.is_dir()
    }

    fn list_dir(&self, path: &Path) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(path) else {
            return Vec::new();
        };
        let mut names: Vec<String> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn read_to_string(&self, path: &Path) -> Option<String> {
        // `metadata` first: opening a FIFO would block forever, and a device
        // such as /dev/zero would never end. Repo files are small, so cap them.
        if !std::fs::metadata(path).ok()?.is_file() {
            return None;
        }
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .ok()?
            .take(MAX_READ_BYTES as u64)
            .read_to_end(&mut bytes)
            .ok()?;
        Some(String::from_utf8_lossy(&bytes).into_owned())
    }

    fn canonicalize(&self, path: &Path) -> Option<PathBuf> {
        std::fs::canonicalize(path).ok()
    }

    fn home(&self) -> Option<PathBuf> {
        std::env::var_os("HOME").map(PathBuf::from)
    }

    #[cfg(target_os = "macos")]
    fn footprint(&self, pid: u32) -> Option<u64> {
        let pid = libc::c_int::try_from(pid).ok()?;
        // SAFETY: `info` is a plain-data struct the kernel fills in; the
        // buffer type is the C `rusage_info_t` pointer the call expects.
        let mut info: libc::rusage_info_v2 = unsafe { std::mem::zeroed() };
        let status = unsafe {
            libc::proc_pid_rusage(
                pid,
                libc::RUSAGE_INFO_V2,
                (&raw mut info).cast::<libc::rusage_info_t>(),
            )
        };
        (status == 0).then_some(info.ri_phys_footprint)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_captures_stdout_and_success() {
        let out = SystemRunner::new().run("echo", &["hello"]).unwrap();

        assert!(out.success);
        assert_eq!(out.text(), "hello");
    }

    #[test]
    fn run_reports_failure_exit_status_with_stderr() {
        let out = SystemRunner::new()
            .run("sh", &["-c", "echo oops >&2; exit 3"])
            .unwrap();

        assert!(!out.success);
        assert_eq!(out.stderr.trim(), "oops");
    }

    #[test]
    fn run_reports_missing_program() {
        let err = SystemRunner::new()
            .run("definitely-not-a-real-tool-xyz", &[])
            .unwrap_err();

        assert_eq!(err, RunError::NotFound);
    }

    #[test]
    fn run_kills_and_reports_timeout() {
        let runner = SystemRunner::with_timeout(Duration::from_millis(150));
        let started = std::time::Instant::now();

        let err = runner.run("sleep", &["5"]).unwrap_err();

        assert_eq!(err, RunError::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn run_does_not_deadlock_on_large_output() {
        let out = SystemRunner::new()
            .run("sh", &["-c", "yes x | head -c 300000"])
            .unwrap();

        assert_eq!(out.stdout.len(), 300_000);
    }

    #[test]
    fn run_returns_promptly_when_a_background_child_keeps_the_pipe_open() {
        let started = Instant::now();

        let out = SystemRunner::new()
            .run("sh", &["-c", "echo hi; sleep 3 &"])
            .unwrap();

        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
        assert!(out.success);
        assert_eq!(out.stdout, "hi\n");
    }

    #[test]
    fn timeout_also_stops_grandchildren() {
        let runner = SystemRunner::with_timeout(Duration::from_millis(200));
        let started = Instant::now();

        let err = runner.run("sh", &["-c", "sleep 30 & wait"]).unwrap_err();

        assert_eq!(err, RunError::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn run_caps_captured_output_without_blocking_the_child() {
        let out = SystemRunner::new()
            .run("sh", &["-c", "yes x | head -c 3000000"])
            .unwrap();

        assert!(out.success);
        assert_eq!(out.stdout.len(), MAX_OUTPUT_BYTES);
    }

    #[test]
    fn run_error_messages_are_readable() {
        assert_eq!(RunError::NotFound.to_string(), "program not found");
        assert_eq!(RunError::TimedOut.to_string(), "timed out");
        assert_eq!(RunError::Other("boom".into()).to_string(), "boom");
    }

    #[test]
    fn read_to_string_only_reads_regular_files_and_caps_the_size() {
        let tmp = tempfile::TempDir::new().unwrap();
        let big = tmp.path().join("big.properties");
        std::fs::write(&big, "a".repeat(MAX_READ_BYTES * 2)).unwrap();
        let small = tmp.path().join("small.properties");
        std::fs::write(&small, "k=v\n").unwrap();
        let sys = SystemRunner::new();

        assert_eq!(sys.read_to_string(&small).as_deref(), Some("k=v\n"));
        assert_eq!(
            sys.read_to_string(&big).map(|t| t.len()),
            Some(MAX_READ_BYTES)
        );
        assert_eq!(
            sys.read_to_string(Path::new("/dev/zero")),
            None,
            "device files are not regular files"
        );
        assert_eq!(
            sys.read_to_string(tmp.path()),
            None,
            "directories are not readable as text"
        );
    }

    #[test]
    fn canonicalize_resolves_symlinks_and_rejects_missing_paths() {
        let tmp = tempfile::TempDir::new().unwrap();
        let real = tmp.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let sys = SystemRunner::new();

        assert_eq!(sys.canonicalize(&link), std::fs::canonicalize(&real).ok());
        assert_eq!(sys.canonicalize(&tmp.path().join("nope")), None);
    }

    #[test]
    fn spawn_returns_at_once_and_the_program_keeps_running() {
        let tmp = tempfile::TempDir::new().unwrap();
        let marker = tmp.path().join("marker");
        let started = Instant::now();

        SystemRunner::new()
            .spawn(
                "sh",
                &["-c", &format!("sleep 1; touch {}", marker.display())],
            )
            .unwrap();

        assert!(
            started.elapsed() < Duration::from_millis(500),
            "did not wait"
        );
        assert!(!marker.exists(), "still running");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !marker.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(marker.exists(), "the detached program finished its work");
    }

    #[test]
    fn spawn_reports_a_missing_program_and_discards_output() {
        let sys = SystemRunner::new();

        assert_eq!(
            sys.spawn("definitely-not-a-real-tool-xyz", &[]),
            Err(RunError::NotFound)
        );
        assert_eq!(
            sys.spawn("sh", &["-c", "echo noisy; echo noisy >&2"]),
            Ok(())
        );
    }

    #[test]
    fn list_dir_is_sorted_and_empty_when_missing() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("b"), "").unwrap();
        std::fs::write(tmp.path().join("a"), "").unwrap();
        let sys = SystemRunner::new();

        assert_eq!(sys.list_dir(tmp.path()), vec!["a", "b"]);
        assert!(sys.list_dir(&tmp.path().join("nope")).is_empty());
    }
}
