//! Background work for the performance status: sampling on a timer and
//! stopping a process the user chose.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use lf_mide_core::{Action, ProcessKind};
use lf_mide_system::SystemRunner;
use tokio::sync::mpsc::UnboundedSender;

/// Samples every `interval` while `visible` is set (the indicator or the
/// panel is on screen); a hidden indicator costs nothing.
pub fn spawn_perf_watch(
    tx: UnboundedSender<Action>,
    interval: Duration,
    visible: Arc<AtomicBool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            if !visible.load(Ordering::Relaxed) {
                continue;
            }
            let sampled = tokio::task::spawn_blocking(|| {
                lf_mide_perf::sample(&SystemRunner::new(), std::process::id())
            })
            .await;
            match sampled {
                Ok(Ok(sample)) => {
                    if tx.send(Action::PerfSampled(sample)).is_err() {
                        return;
                    }
                }
                Ok(Err(error)) => tracing::warn!(%error, "performance sample failed"),
                Err(error) => tracing::warn!(%error, "performance sampler panicked"),
            }
        }
    })
}

/// How often the clock of a running build ticks.
const BUILD_TICK: Duration = Duration::from_millis(100);

/// Sends [`Action::Tick`] every [`BUILD_TICK`] while `active` is set (a
/// build is running); an idle app is not woken.
pub fn spawn_build_clock(
    tx: UnboundedSender<Action>,
    active: Arc<AtomicBool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let epoch = tokio::time::Instant::now();
        let mut ticker = tokio::time::interval(BUILD_TICK);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            if !active.load(Ordering::Relaxed) {
                continue;
            }
            let millis = u64::try_from(epoch.elapsed().as_millis()).unwrap_or(u64::MAX);
            if tx.send(Action::Tick { millis }).is_err() {
                return;
            }
        }
    })
}

/// Whether `pid` may be sent a signal at all: a real process that is not
/// init, not us, and not a value that would mean "a whole process group".
fn may_signal(pid: u32, own_pid: u32) -> bool {
    pid > 1 && pid != own_pid && i32::try_from(pid).is_ok()
}

/// Asks the process to end (SIGTERM), after checking that the pid still
/// belongs to the kind of process the user saw in the list.
pub fn spawn_stop_process(pid: u32, kind: ProcessKind, tx: UnboundedSender<Action>) {
    std::thread::spawn(move || {
        let own = std::process::id();
        let message = stop_process(&SystemRunner::new(), pid, kind, own);
        let _ = tx.send(Action::Status(message));
    });
}

fn stop_process(sys: &dyn lf_mide_system::System, pid: u32, kind: ProcessKind, own: u32) -> String {
    let name = kind.label();
    if !may_signal(pid, own) || !lf_mide_perf::is_still(sys, pid, kind, own) {
        return format!("{name} (pid {pid}) is no longer running");
    }
    // SAFETY: a plain signal to a process we just confirmed is the one the
    // user chose; the pid is positive and fits a pid_t.
    let sent = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
    if sent == 0 {
        format!("Asked {name} (pid {pid}) to stop")
    } else {
        format!(
            "Could not stop {name} (pid {pid}): {}",
            std::io::Error::last_os_error()
        )
    }
}

#[cfg(test)]
mod tests {
    use lf_mide_system::fake::FakeSystem;

    use super::*;

    #[test]
    fn only_ordinary_other_processes_may_be_signalled() {
        assert!(may_signal(4121, 10));
        assert!(!may_signal(0, 10), "pid 0 is our own process group");
        assert!(!may_signal(1, 10), "init");
        assert!(!may_signal(10, 10), "ourselves");
        assert!(!may_signal(u32::MAX, 10), "would become a negative pid");
    }

    #[test]
    fn a_process_that_changed_into_something_else_is_left_alone() {
        let sys = FakeSystem::new().ok("/bin/ps -ww -p 4121 -o args=", "/usr/bin/vim notes.txt\n");

        let message = stop_process(&sys, 4121, ProcessKind::GradleDaemon, 10);

        assert_eq!(message, "Gradle daemon (pid 4121) is no longer running");
    }

    #[test]
    fn a_process_that_is_gone_is_reported_as_gone() {
        let sys = FakeSystem::new();

        let message = stop_process(&sys, 4121, ProcessKind::KotlinDaemon, 10);

        assert!(message.contains("no longer running"));
    }
}
