//! `lf-mide swift-setup`: the build server config Swift completion needs.

use std::io::Write;
use std::path::Path;

use color_eyre::Result;
use color_eyre::eyre::WrapErr;
use lf_mide_build::{apply_build_server, plan_build_server};
use lf_mide_core::project;
use lf_mide_system::{System, SystemRunner};

use crate::cli::SwiftSetupArgs;

pub fn run(args: SwiftSetupArgs) -> Result<i32> {
    let start = match &args.path {
        Some(path) => path.clone(),
        None => std::env::current_dir().wrap_err("cannot read current directory")?,
    };
    let start = start
        .canonicalize()
        .wrap_err_with(|| format!("cannot open {}", start.display()))?;
    let root = match project::detect(&start) {
        Ok(project) => project.root,
        Err(error) => {
            eprintln!("{error}");
            return Ok(1);
        }
    };
    let mut out = std::io::stdout().lock();
    session(&root, args.apply, &SystemRunner::new(), &mut out)
}

/// Shows the command and, if asked, runs it in `root`.
pub fn session(root: &Path, apply: bool, sys: &dyn System, out: &mut dyn Write) -> Result<i32> {
    let plan = match plan_build_server(root, sys) {
        Ok(plan) => plan,
        Err(message) => {
            writeln!(out, "Cannot set up Swift completion: {message}")?;
            return Ok(1);
        }
    };
    writeln!(out, "Scheme:  {}", plan.scheme)?;
    writeln!(out, "Project: {}", plan.container.display())?;
    writeln!(out, "Command: {}", plan.command_line())?;
    if plan.replaces {
        writeln!(
            out,
            "{} exists and would be replaced.",
            plan.config_file.display()
        )?;
    }
    if !apply {
        writeln!(
            out,
            "\nRun `lf-mide swift-setup --apply` to write {}.",
            plan.config_file.display()
        )?;
        return Ok(0);
    }
    match apply_build_server(&plan, root, sys) {
        Ok(()) => {
            writeln!(out, "\nWrote {}.", plan.config_file.display())?;
            writeln!(
                out,
                "Add buildServer.json and .compile to .gitignore: they are machine specific."
            )?;
            Ok(0)
        }
        Err(message) => {
            writeln!(out, "\n{message}")?;
            Ok(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use lf_mide_system::fake::FakeSystem;

    use super::*;

    fn run_session(sys: &FakeSystem, apply: bool) -> (i32, String) {
        let mut out = Vec::new();
        let code = session(Path::new("/p"), apply, sys, &mut out).unwrap();
        (code, String::from_utf8(out).unwrap())
    }

    fn ready() -> FakeSystem {
        FakeSystem::new()
            .env("PATH", "/bin")
            .file("/bin/xcode-build-server", "")
            .dir("/p/iosApp", &["iosApp.xcodeproj"])
            .ok(
                "xcrun xcodebuild -project iosApp/iosApp.xcodeproj -list -json",
                r#"{"project":{"schemes":["iosApp"]}}"#,
            )
    }

    #[test]
    fn without_apply_it_only_shows_the_command() {
        let (code, text) = run_session(&ready(), false);

        assert_eq!(code, 0);
        assert!(text.contains("-scheme iosApp"), "{text}");
        assert!(text.contains("--apply"), "{text}");
        assert!(ready().ran_in().is_empty());
    }

    #[test]
    fn with_apply_it_runs_the_tool_in_the_project() {
        let sys = ready()
            .ok(
                "/bin/xcode-build-server config -project iosApp/iosApp.xcodeproj -scheme iosApp",
                "",
            )
            .file("/p/buildServer.json", "{}");

        let (code, text) = run_session(&sys, true);

        assert_eq!(code, 0, "{text}");
        assert!(text.contains("Wrote /p/buildServer.json"), "{text}");
        assert!(
            sys.ran_in()
                .iter()
                .any(|(dir, cmd)| dir == Path::new("/p")
                    && cmd.contains("xcode-build-server config"))
        );
    }

    #[test]
    fn a_missing_tool_is_reported_with_a_failing_exit_code() {
        let (code, text) = run_session(&FakeSystem::new(), false);

        assert_eq!(code, 1);
        assert!(text.contains("brew install xcode-build-server"), "{text}");
    }
}
