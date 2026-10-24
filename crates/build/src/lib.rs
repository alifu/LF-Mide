//! Building, installing and launching the app on a running emulator or
//! simulator, and following its log.
//!
//! The steps run through two small traits so they can be tested without a
//! real Gradle or Xcode: [`System`] for short commands and files, and
//! [`Streamer`] for long ones whose output is reported while they run.
//!
//! Trust: the repository decides what a build does (`gradlew` and the Xcode
//! project are its code), so a build only starts when the user asks for it.
//! Everything else that reaches a command line (application id, scheme,
//! bundle id, device ids) is validated first.

mod android;
mod ios;
mod logs;
mod streamer;

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use lf_mide_core::{BuildOutcome, Device, Platform, ProjectLayout};
use lf_mide_system::System;
use lf_mide_system::stream::{Exit, StreamSpec};

pub use logs::log_spec;
pub use streamer::ProcessStreamer;

/// Set by one thread to ask a running pipeline to stop.
#[derive(Debug, Clone, Default)]
pub struct CancelFlag(Arc<AtomicBool>);

impl CancelFlag {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// Where a pipeline reports to.
pub trait Report {
    /// The pipeline moved on to this step ("Building", "Installing").
    fn step(&mut self, step: &str);
    /// Lines for the build log.
    fn lines(&mut self, lines: Vec<String>);
}

/// Runs a long command, passing on its output as it arrives.
pub trait Streamer {
    /// Returns when the command ends (or was stopped through `cancel`).
    fn stream(
        &self,
        spec: &StreamSpec,
        cancel: &CancelFlag,
        on_lines: &mut dyn FnMut(Vec<String>),
    ) -> Result<Exit, lf_mide_system::RunError>;
}

/// What a pipeline needs to know.
pub struct Context<'a> {
    pub root: &'a Path,
    pub layout: ProjectLayout,
    /// Where Xcode may put its build products.
    pub derived_data: &'a Path,
    pub device: &'a Device,
}

/// Why a pipeline stopped before the end.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Stop {
    Cancelled,
    Failed(String),
}

pub(crate) type Step = Result<(), Stop>;

pub(crate) fn failed(message: impl Into<String>) -> Stop {
    Stop::Failed(message.into())
}

/// Builds the app, installs it on `ctx.device` and starts it.
pub fn build_and_run(
    ctx: &Context<'_>,
    sys: &dyn System,
    streamer: &dyn Streamer,
    report: &mut dyn Report,
    cancel: &CancelFlag,
) -> BuildOutcome {
    let result = match ctx.device.platform {
        Platform::Android => android::run(ctx, sys, streamer, report, cancel),
        Platform::Ios => ios::run(ctx, sys, streamer, report, cancel),
    };
    match result {
        Ok(()) => BuildOutcome::Succeeded,
        Err(Stop::Cancelled) => BuildOutcome::Cancelled,
        Err(Stop::Failed(reason)) => BuildOutcome::Failed(Some(reason)),
    }
}

/// Streams `spec` into the report. `Err` when it was cancelled, could not
/// start or exited with an error.
pub(crate) fn stream_step(
    streamer: &dyn Streamer,
    report: &mut dyn Report,
    cancel: &CancelFlag,
    spec: &StreamSpec,
    what: &str,
) -> Step {
    let shown: Vec<&str> = std::iter::once(spec.program.as_str())
        .chain(spec.args.iter().map(String::as_str))
        .collect();
    report.lines(vec![format!("$ {}", shown.join(" "))]);
    let exit = streamer.stream(spec, cancel, &mut |lines| report.lines(lines));
    if cancel.is_cancelled() {
        return Err(Stop::Cancelled);
    }
    match exit {
        Ok(exit) if exit.success() => Ok(()),
        Ok(Exit { code: Some(code) }) => Err(failed(format!("{what} exited with code {code}"))),
        Ok(Exit { code: None }) => Err(failed(format!("{what} was stopped by a signal"))),
        Err(lf_mide_system::RunError::NotFound) => Err(failed(format!("{what} was not found"))),
        Err(error) => Err(failed(format!("could not start {what}: {error}"))),
    }
}

/// A short command whose output is logged. `Err` on any failure.
pub(crate) fn run_step(
    sys: &dyn System,
    cancel: &CancelFlag,
    report: &mut dyn Report,
    dir: Option<&Path>,
    program: &str,
    args: &[&str],
    what: &str,
) -> Result<String, Stop> {
    if cancel.is_cancelled() {
        return Err(Stop::Cancelled);
    }
    report.lines(vec![format!("$ {program} {}", args.join(" "))]);
    let output = match dir {
        Some(dir) => sys.run_in(dir, program, args),
        None => sys.run(program, args),
    }
    .map_err(|error| failed(format!("{what}: {error}")))?;
    let text = output.text();
    if !text.is_empty() {
        report.lines(text.lines().map(str::to_string).collect());
    }
    if output.success {
        Ok(output.stdout)
    } else {
        Err(failed(format!("{what} failed")))
    }
}

/// True for the characters allowed in identifiers we put on a command line:
/// letters, digits and `. _ -`, not starting with `-` or `.`.
pub(crate) fn plain_identifier(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 200
        && !text.starts_with(['-', '.'])
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

#[cfg(test)]
pub(crate) mod testing {
    //! Fakes shared by the tests of the pipelines.

    use std::cell::RefCell;
    use std::collections::VecDeque;

    use super::*;

    #[derive(Default)]
    pub struct Recorder {
        pub steps: Vec<String>,
        pub log: Vec<String>,
    }

    impl Report for Recorder {
        fn step(&mut self, step: &str) {
            self.steps.push(step.to_string());
        }

        fn lines(&mut self, lines: Vec<String>) {
            self.log.extend(lines);
        }
    }

    /// One scripted answer: the output lines and how the command ended.
    pub type Answer = (Vec<&'static str>, Result<Exit, lf_mide_system::RunError>);

    /// Answers streamed commands in order and remembers what was asked.
    pub struct ScriptedStreamer {
        answers: RefCell<VecDeque<Answer>>,
        pub specs: RefCell<Vec<StreamSpec>>,
    }

    impl ScriptedStreamer {
        pub fn new(answers: Vec<Answer>) -> Self {
            Self {
                answers: RefCell::new(answers.into()),
                specs: RefCell::new(Vec::new()),
            }
        }

        pub fn ok(lines: Vec<&'static str>) -> Answer {
            (lines, Ok(Exit { code: Some(0) }))
        }

        pub fn exits(code: i32, lines: Vec<&'static str>) -> Answer {
            (lines, Ok(Exit { code: Some(code) }))
        }
    }

    impl Streamer for ScriptedStreamer {
        fn stream(
            &self,
            spec: &StreamSpec,
            _cancel: &CancelFlag,
            on_lines: &mut dyn FnMut(Vec<String>),
        ) -> Result<Exit, lf_mide_system::RunError> {
            self.specs.borrow_mut().push(spec.clone());
            let (lines, exit) = self
                .answers
                .borrow_mut()
                .pop_front()
                .expect("no more scripted commands");
            on_lines(lines.into_iter().map(str::to_string).collect());
            exit
        }
    }
}
