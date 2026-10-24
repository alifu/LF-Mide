//! LF-Mide: a lightweight terminal IDE for Compose Multiplatform projects.

mod branch_watch;
mod cli;
mod doctor_cmd;
mod editor;
mod effects;
mod event_loop;
mod jobs;
mod keymap;
mod paths;
mod perf_effects;
mod settings;

use clap::Parser;
use color_eyre::Result;
use color_eyre::eyre::WrapErr;
use lf_mide_core::{AppState, Config, project};
use lf_mide_doctor::checks::CHECK_THREAD_NAME;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};

use cli::{Cli, Command};

const LOG_FILE_PREFIX: &str = "lf-mide";
const LOG_FILES_KEPT: usize = 7;
const PANIC_EXIT_CODE: i32 = 101;

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;
    let cli = Cli::parse();
    if let Some(Command::Doctor(args)) = cli.command {
        std::process::exit(doctor_cmd::run(args)?);
    }

    // Everything that can fail with a readable message happens before the
    // terminal switches to the alternate screen.
    let start = match cli.path {
        Some(path) => path,
        None => std::env::current_dir().wrap_err("cannot read current directory")?,
    };
    let start = start
        .canonicalize()
        .wrap_err_with(|| format!("cannot open {}", start.display()))?;
    let project = project::detect(&start)?;
    let global = paths::global_config();
    let config = Config::load(
        global.as_deref(),
        Some(&paths::project_config(&project.root)),
    )?;
    let log_guard = init_logging();
    tracing::info!(root = %project.root.display(), "starting");

    let terminal = ratatui::try_init().wrap_err("cannot set up the terminal")?;
    exit_on_panic();
    let result = event_loop::run(terminal, AppState::new(project, config)).await;
    ratatui::restore();
    drop(log_guard);
    // Returning from `main` would wait for device listings still running on
    // the blocking pool (each can take seconds); the terminal is already
    // restored, so leave now.
    if result.is_ok() {
        std::process::exit(0);
    }
    result
}

/// ratatui's panic hook restores the terminal, but a panic inside a tokio
/// task would leave the app running on a restored (cooked) terminal. Any
/// panic is a bug, so report it and exit instead (except in doctor checks,
/// which contain their own panics).
fn exit_on_panic() {
    let restore_and_report = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if is_contained_panic(std::thread::current().name()) {
            // Reported as a warning in the doctor; don't touch the terminal.
            tracing::error!(%info, "a doctor check panicked");
            return;
        }
        restore_and_report(info);
        std::process::exit(PANIC_EXIT_CODE);
    }));
}

/// Logs to a daily-rotated file. Any failure disables logging instead of
/// stopping the app.
fn init_logging() -> Option<WorkerGuard> {
    let dir = paths::log_dir()?;
    let appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix(LOG_FILE_PREFIX)
        .filename_suffix("log")
        .max_log_files(LOG_FILES_KEPT)
        .build(dir)
        .ok()?;
    let (writer, guard) = tracing_appender::non_blocking(appender);
    tracing_subscriber::fmt()
        .with_writer(writer)
        .with_ansi(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("LF_MIDE_LOG")
                .unwrap_or_else(|_| "info".into()),
        )
        .init();
    Some(guard)
}

/// True for panics the doctor already contains (a check thread whose panic
/// becomes a warning in the report): those must not end the whole app.
fn is_contained_panic(thread_name: Option<&str>) -> bool {
    thread_name == Some(CHECK_THREAD_NAME)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_doctor_check_threads_are_contained() {
        assert!(is_contained_panic(Some(CHECK_THREAD_NAME)));
        assert!(!is_contained_panic(Some("main")));
        assert!(!is_contained_panic(Some("tokio-runtime-worker")));
        assert!(!is_contained_panic(None));
    }
}
