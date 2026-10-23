//! The real [`Streamer`]: runs the command and forwards its output.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lf_mide_system::RunError;
use lf_mide_system::stream::{Canceller, Event, Exit, StreamSpec, start};

use crate::{CancelFlag, Streamer};

/// How often a stream is checked for a cancel request.
const POLL: Duration = Duration::from_millis(100);

/// Runs commands as real processes. Clones share one list of what is
/// running, so [`kill_all`](Self::kill_all) can stop everything started
/// through any of them.
#[derive(Debug, Default, Clone)]
pub struct ProcessStreamer {
    running: Arc<Mutex<Vec<(u64, Canceller)>>>,
    next_id: Arc<AtomicU64>,
}

impl ProcessStreamer {
    /// Kills every command still running, with no grace period.
    pub fn kill_all(&self) {
        let running = self.running.lock().unwrap_or_else(|e| e.into_inner());
        for (_, canceller) in running.iter() {
            canceller.kill_now();
        }
    }

    fn register(&self, canceller: Canceller) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        self.running
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((id, canceller));
        id
    }

    fn unregister(&self, id: u64) {
        self.running
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|(known, _)| *known != id);
    }
}

impl Streamer for ProcessStreamer {
    fn stream(
        &self,
        spec: &StreamSpec,
        cancel: &CancelFlag,
        on_lines: &mut dyn FnMut(Vec<String>),
    ) -> Result<Exit, RunError> {
        let mut stream = start(spec)?;
        let canceller = stream.canceller();
        let id = self.register(canceller.clone());
        let exit = loop {
            if cancel.is_cancelled() {
                canceller.cancel();
            }
            match stream.next(POLL) {
                Event::Lines(lines) => on_lines(lines),
                Event::Exited(exit) => break exit,
                Event::Idle => {}
            }
        };
        self.unregister(id);
        Ok(exit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(script: &str) -> StreamSpec {
        StreamSpec {
            program: "sh".into(),
            args: vec!["-c".into(), script.into()],
            ..StreamSpec::default()
        }
    }

    #[test]
    fn forwards_output_and_returns_the_exit() {
        let mut seen = Vec::new();

        let exit = ProcessStreamer::default()
            .stream(
                &sh("echo a; echo b; exit 2"),
                &CancelFlag::default(),
                &mut |l| seen.extend(l),
            )
            .unwrap();

        assert_eq!(seen, ["a", "b"]);
        assert_eq!(exit, Exit { code: Some(2) });
    }

    #[test]
    fn a_cancel_request_stops_a_long_command() {
        let cancel = CancelFlag::default();
        let remote = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            remote.cancel();
        });

        let started = std::time::Instant::now();
        let exit = ProcessStreamer::default()
            .stream(&sh("sleep 600"), &cancel, &mut |_| {})
            .unwrap();

        assert!(!exit.success());
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn kill_all_stops_a_command_that_ignores_the_polite_request() {
        let streamer = ProcessStreamer::default();
        let runner = streamer.clone();
        let handle = std::thread::spawn(move || {
            runner.stream(
                &sh("trap '' TERM; echo ready; while true; do sleep 1; done"),
                &CancelFlag::default(),
                &mut |_| {},
            )
        });
        let started = std::time::Instant::now();
        while streamer.running.lock().unwrap().is_empty() {
            assert!(started.elapsed() < Duration::from_secs(5), "never started");
            std::thread::sleep(Duration::from_millis(20));
        }
        std::thread::sleep(Duration::from_millis(300));

        streamer.kill_all();

        let exit = handle.join().unwrap().unwrap();
        assert!(!exit.success());
        assert!(started.elapsed() < Duration::from_secs(10));
        assert!(streamer.running.lock().unwrap().is_empty());
    }

    #[test]
    fn a_finished_command_is_forgotten() {
        let streamer = ProcessStreamer::default();

        streamer
            .stream(&sh("true"), &CancelFlag::default(), &mut |_| {})
            .unwrap();

        assert!(streamer.running.lock().unwrap().is_empty());
        streamer.kill_all();
    }
}
