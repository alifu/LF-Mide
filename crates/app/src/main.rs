//! LF-Mide: a lightweight terminal IDE for Compose Multiplatform projects.

mod branch_watch;
mod clean_cmd;
mod cli;
mod doctor_cmd;
mod editor;
mod effects;
mod embedded_editor;
mod event_loop;
mod git_effects;
mod jobs;
mod keymap;
mod mouse;
mod paths;
mod perf_effects;
mod settings;
mod tune_cmd;

use clap::Parser;
use color_eyre::Result;
use color_eyre::eyre::WrapErr;
use lf_mide_core::{Action, AppState, Config, project, reduce};
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
    match cli.command {
        Some(Command::Doctor(args)) => std::process::exit(doctor_cmd::run(args)?),
        Some(Command::Clean(args)) => std::process::exit(clean_cmd::run(args)?),
        Some(Command::Tune(args)) => std::process::exit(tune_cmd::run(args)?),
        None => {}
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
    if lf_mide_editor::keys::leader(&config.leader).is_none() {
        color_eyre::eyre::bail!(
            "leader = {:?} is not a key LF-Mide understands; try \"C-Space\" or \"C-\\\\\"",
            config.leader
        );
    }
    lf_mide_ui::theme::use_theme(config.theme);
    let log_guard = init_logging();
    tracing::info!(root = %project.root.display(), "starting");

    let terminal = ratatui::try_init().wrap_err("cannot set up the terminal")?;
    // Pastes arrive as one event, so they can go to Neovim as one change.
    set_input_modes(true);
    exit_on_panic();
    let mut state = AppState::new(project, config);
    if let Some(socket) = attached_socket(cli.nvim_socket) {
        state = reduce(state, Action::EditorOpened(socket));
        state.status = Some("Attached to your Neovim".to_string());
    }
    let result = event_loop::run(terminal, state).await;
    set_input_modes(false);
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

/// Turns bracketed paste and mouse reporting on or off. A terminal without
/// them just ignores the request, so a failure is only logged.
fn set_input_modes(on: bool) {
    use crossterm::event::{
        DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    };
    let outcome = if on {
        crossterm::execute!(std::io::stdout(), EnableBracketedPaste, EnableMouseCapture)
    } else {
        crossterm::execute!(
            std::io::stdout(),
            DisableBracketedPaste,
            DisableMouseCapture
        )
    };
    if let Err(error) = outcome {
        tracing::debug!(%error, "cannot change the input modes");
    }
}

/// The Neovim socket named on the command line, made absolute (the app may
/// be started from another directory than the one the user typed it in).
fn attached_socket(given: Option<std::path::PathBuf>) -> Option<std::path::PathBuf> {
    let given = given?;
    if given.is_absolute() {
        return Some(given);
    }
    std::env::current_dir().ok().map(|here| here.join(given))
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
        set_input_modes(false);
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
    fn a_relative_socket_path_is_made_absolute_and_an_absolute_one_kept() {
        let here = std::env::current_dir().unwrap();

        assert_eq!(attached_socket(None), None);
        assert_eq!(
            attached_socket(Some("/tmp/nvim.sock".into())),
            Some("/tmp/nvim.sock".into())
        );
        assert_eq!(
            attached_socket(Some("n.sock".into())),
            Some(here.join("n.sock"))
        );
    }

    #[test]
    fn only_doctor_check_threads_are_contained() {
        assert!(is_contained_panic(Some(CHECK_THREAD_NAME)));
        assert!(!is_contained_panic(Some("main")));
        assert!(!is_contained_panic(Some("tokio-runtime-worker")));
        assert!(!is_contained_panic(None));
    }
}
