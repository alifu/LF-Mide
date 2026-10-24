//! `lf-mide tune`: Gradle memory settings worth having.

use std::io::Write;
use std::path::{Path, PathBuf};

use color_eyre::Result;
use color_eyre::eyre::{WrapErr, eyre};
use lf_mide_core::project;
use lf_mide_system::{System, SystemRunner};
use lf_mide_tidy::tune::{self, Row, Source, TuneError};

use crate::cli::TuneArgs;

const USER_FILE_NAME: &str = "gradle.properties";

pub fn run(args: TuneArgs) -> Result<i32> {
    let sys = SystemRunner::new();
    let home = crate::paths::user_home(&sys)?;
    let start = match &args.path {
        Some(path) => path.clone(),
        None => std::env::current_dir().wrap_err("cannot read current directory")?,
    };
    let start = start
        .canonicalize()
        .wrap_err_with(|| format!("cannot open {}", start.display()))?;
    let project_file = project::detect(&start)
        .ok()
        .map(|p| p.root.join("gradle.properties"));
    let user_file = crate::paths::gradle_user_home(&sys, &home).join(USER_FILE_NAME);
    let mut out = std::io::stdout().lock();
    session(
        &user_file,
        project_file.as_deref(),
        args.apply,
        args.with_configuration_cache,
        &mut out,
    )
}

/// Shows the review and, if asked, adds the missing settings to `user_file`.
pub fn session(
    user_file: &Path,
    project_file: Option<&Path>,
    apply: bool,
    with_configuration_cache: bool,
    out: &mut dyn Write,
) -> Result<i32> {
    let user = tune::read_for_review(user_file).map_err(|e| eyre!("{e}"))?;
    // The project's file is only read, and a problem with it is not ours.
    let project = project_file.and_then(|path| tune::read_for_review(path).ok().flatten());
    let rows = tune::review(user.as_deref(), project.as_deref());
    write!(out, "{}", render(&rows, user_file))?;

    let missing = tune::additions(&rows, with_configuration_cache);
    if missing.is_empty() {
        writeln!(out, "\nNothing to add.")?;
        return Ok(0);
    }
    if !apply {
        writeln!(
            out,
            "\nRun `lf-mide tune --apply` to add the {} missing setting{} to {}.",
            missing.len(),
            if missing.len() == 1 { "" } else { "s" },
            user_file.display()
        )?;
        return Ok(0);
    }
    match tune::apply(user_file, &missing) {
        Ok(applied) => {
            writeln!(
                out,
                "\nAdded {} to {}.",
                applied.added.join(", "),
                user_file.display()
            )?;
            if let Some(backup) = applied.backup {
                writeln!(out, "The original is kept as {}.", backup.display())?;
            }
            Ok(0)
        }
        Err(error @ TuneError::Symlink(_)) => {
            writeln!(out, "\n{error}. These are the lines to add:")?;
            for (key, value) in &missing {
                writeln!(out, "  {key}={value}")?;
            }
            Ok(1)
        }
        Err(error) => Err(eyre!("{error}")),
    }
}

fn render(rows: &[Row], user_file: &Path) -> String {
    let mut text = format!(
        "Gradle memory settings (new ones go into {})\n\n",
        shown_path(user_file)
    );
    for row in rows {
        let state = match &row.current {
            None => "missing".to_string(),
            // The values come from files, the project's from a repository.
            Some((Source::User, value)) => {
                format!("yours: {}", lf_mide_core::git::clean_text(value))
            }
            Some((Source::Project, value)) => {
                format!("project: {}", lf_mide_core::git::clean_text(value))
            }
        };
        let optional = if row.setting.optional {
            " (optional)"
        } else {
            ""
        };
        text.push_str(&format!(
            "  {:<34} {}\n      suggested: {}{optional}\n      {}\n",
            row.setting.key, state, row.setting.value, row.setting.why
        ));
    }
    text
}

fn shown_path(path: &Path) -> String {
    let home: Option<PathBuf> = SystemRunner::new().home();
    match home.and_then(|h| path.strip_prefix(h).ok().map(Path::to_path_buf)) {
        Some(relative) => format!("~/{}", relative.display()),
        None => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn run_session(
        user: &Path,
        project: Option<&Path>,
        apply: bool,
        optional: bool,
    ) -> (i32, String) {
        let mut out = Vec::new();
        let code = session(user, project, apply, optional, &mut out).unwrap();
        (code, String::from_utf8(out).unwrap())
    }

    #[test]
    fn without_apply_it_only_shows_and_changes_nothing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let user = tmp.path().join(".gradle/gradle.properties");

        let (code, text) = run_session(&user, None, false, false);

        assert_eq!(code, 0);
        assert!(text.contains("org.gradle.jvmargs") && text.contains("missing"));
        assert!(text.contains("lf-mide tune --apply"));
        assert!(!user.exists());
    }

    #[test]
    fn apply_adds_only_what_is_missing_and_a_second_run_has_nothing_left() {
        let tmp = tempfile::TempDir::new().unwrap();
        let user = tmp.path().join(".gradle/gradle.properties");
        let project = tmp.path().join("gradle.properties");
        fs::write(&project, "org.gradle.caching=false\n").unwrap();

        let (_, first) = run_session(&user, Some(&project), true, false);
        let (_, second) = run_session(&user, Some(&project), true, false);

        let written = fs::read_to_string(&user).unwrap();
        assert!(first.contains(
            "Added org.gradle.jvmargs, kotlin.daemon.jvmargs, org.gradle.daemon.idletimeout"
        ));
        assert!(
            !written.contains("org.gradle.caching"),
            "the project sets it"
        );
        assert!(!written.contains("configuration-cache"));
        assert!(second.contains("Nothing to add."));
    }

    #[test]
    fn the_configuration_cache_only_when_asked() {
        let tmp = tempfile::TempDir::new().unwrap();
        let user = tmp.path().join("gradle.properties");

        run_session(&user, None, true, true);

        assert!(
            fs::read_to_string(&user)
                .unwrap()
                .contains("org.gradle.configuration-cache=true")
        );
    }

    #[test]
    fn a_symlinked_user_file_gets_the_lines_printed_instead() {
        let tmp = tempfile::TempDir::new().unwrap();
        let real = tmp.path().join("dot/gradle.properties");
        fs::create_dir_all(real.parent().unwrap()).unwrap();
        fs::write(&real, "keep=1\n").unwrap();
        let user = tmp.path().join("gradle.properties");
        std::os::unix::fs::symlink(&real, &user).unwrap();
        let mut out = Vec::new();

        let result = session(&user, None, true, false, &mut out);

        assert_eq!(result.unwrap(), 1);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("is a symbolic link"), "{text}");
        assert!(
            text.contains("org.gradle.jvmargs="),
            "the lines are printed to add by hand"
        );
        assert_eq!(fs::read_to_string(&real).unwrap(), "keep=1\n");
    }

    #[test]
    fn a_hostile_value_in_the_project_cannot_redraw_the_screen() {
        let tmp = tempfile::TempDir::new().unwrap();
        let user = tmp.path().join("gradle.properties");
        let project = tmp.path().join("p.properties");
        fs::write(
            &project,
            "org.gradle.jvmargs=-Xmx1g \u{1b}[2J\u{202e}evil\n",
        )
        .unwrap();

        let (_, text) = run_session(&user, Some(&project), false, false);

        assert!(
            !text.chars().any(|c| c == '\u{1b}' || c == '\u{202e}'),
            "{text:?}"
        );
        assert!(text.contains("project: -Xmx1g"));
    }

    #[test]
    fn values_already_set_are_shown_with_where_they_come_from() {
        let tmp = tempfile::TempDir::new().unwrap();
        let user = tmp.path().join("gradle.properties");
        fs::write(&user, "org.gradle.jvmargs=-Xmx1g\n").unwrap();
        let project = tmp.path().join("p.properties");
        fs::write(&project, "kotlin.daemon.jvmargs=-Xmx512m\n").unwrap();

        let (_, text) = run_session(&user, Some(&project), false, false);

        assert!(text.contains("yours: -Xmx1g"));
        assert!(text.contains("project: -Xmx512m"));
    }
}
