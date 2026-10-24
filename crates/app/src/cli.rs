//! Command-line arguments.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(version, about, args_conflicts_with_subcommands = true)]
pub struct Cli {
    /// Path inside the Compose Multiplatform project (default: current directory).
    pub path: Option<PathBuf>,

    /// Use a Neovim you already started (`nvim --listen PATH`) instead of
    /// opening one with `e`.
    #[arg(long, value_name = "PATH")]
    pub nvim_socket: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Check that the tools LF-Mide needs are installed and suggest fixes.
    Doctor(DoctorArgs),
}

#[derive(Debug, Args)]
pub struct DoctorArgs {
    /// Print the report as JSON.
    #[arg(long)]
    pub json: bool,

    /// Path inside the project to check (default: current directory).
    pub path: Option<PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(std::iter::once("lf-mide").chain(args.iter().copied())).unwrap()
    }

    #[test]
    fn definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn no_arguments_opens_the_ide_in_the_current_directory() {
        let cli = parse(&[]);

        assert!(cli.command.is_none());
        assert_eq!(cli.path, None);
    }

    #[test]
    fn an_existing_neovim_can_be_named() {
        let cli = parse(&["--nvim-socket", "/tmp/nvim.sock", "/work/pos"]);

        assert_eq!(cli.nvim_socket, Some(PathBuf::from("/tmp/nvim.sock")));
        assert_eq!(cli.path, Some(PathBuf::from("/work/pos")));
        assert_eq!(parse(&[]).nvim_socket, None);
    }

    #[test]
    fn a_bare_path_opens_the_ide_there() {
        let cli = parse(&["/work/pos"]);

        assert_eq!(cli.path, Some(PathBuf::from("/work/pos")));
        assert!(cli.command.is_none());
    }

    #[test]
    fn doctor_defaults_to_text_in_the_current_directory() {
        let Some(Command::Doctor(args)) = parse(&["doctor"]).command else {
            panic!("not doctor")
        };

        assert!(!args.json);
        assert_eq!(args.path, None);
    }

    #[test]
    fn doctor_accepts_json_and_a_path() {
        let Some(Command::Doctor(args)) = parse(&["doctor", "--json", "/work/pos"]).command else {
            panic!("not doctor")
        };

        assert!(args.json);
        assert_eq!(args.path, Some(PathBuf::from("/work/pos")));
    }

    #[test]
    fn unknown_flags_are_rejected() {
        assert!(Cli::try_parse_from(["lf-mide", "doctor", "--nope"]).is_err());
    }
}
