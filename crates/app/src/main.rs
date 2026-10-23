//! LF-Mide: a lightweight terminal IDE for Compose Multiplatform projects.

mod branch_watch;
mod event_loop;
mod keymap;
mod paths;

use std::path::PathBuf;

use clap::Parser;
use color_eyre::Result;
use color_eyre::eyre::WrapErr;
use lf_mide_core::{AppState, Config, project};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};

const LOG_FILE_PREFIX: &str = "lf-mide";
const LOG_FILES_KEPT: usize = 7;
const PANIC_EXIT_CODE: i32 = 101;

#[derive(Debug, Parser)]
#[command(version, about)]
struct Cli {
    /// Path inside the Compose Multiplatform project (default: current directory).
    path: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;
    let cli = Cli::parse();

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
    let _log_guard = init_logging();
    tracing::info!(root = %project.root.display(), "starting");

    let terminal = ratatui::try_init().wrap_err("cannot set up the terminal")?;
    exit_on_panic();
    let result = event_loop::run(terminal, AppState::new(project, config)).await;
    ratatui::restore();
    result
}

/// ratatui's panic hook restores the terminal, but a panic inside a tokio
/// task would leave the app running on a restored (cooked) terminal. Any
/// panic is a bug, so report it and exit instead.
fn exit_on_panic() {
    let restore_and_report = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
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
