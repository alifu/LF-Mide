//! `lf-mide clean`: show what could be freed, and with `--delete` free it.

use std::io::{BufRead, IsTerminal, Write};

use color_eyre::Result;
use color_eyre::eyre::{WrapErr, eyre};
use lf_mide_core::project;
use lf_mide_system::{System, SystemRunner};
use lf_mide_tidy::{
    Action, Category, CleanEnv, Outcome, Plan, Safety, execute, plan, selection, total_text,
    unknown_ids,
};

use crate::cli::CleanArgs;

/// Items listed under a category before "and N more".
const SHOWN_ITEMS: usize = 6;
const SHOWN_REPORT_ITEMS: usize = 10;
/// Cleaning can wait on `simctl` for a while.
const TOOL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

pub fn run(args: CleanArgs) -> Result<i32> {
    let named = [args.include.clone(), args.only.clone()].concat();
    let unknown = unknown_ids(&named);
    if !unknown.is_empty() {
        return Err(eyre!(
            "unknown category: {}. Run `lf-mide clean` to see the names.",
            unknown.join(", ")
        ));
    }
    let start = match &args.path {
        Some(path) => path.clone(),
        None => std::env::current_dir().wrap_err("cannot read current directory")?,
    };
    let start = start
        .canonicalize()
        .wrap_err_with(|| format!("cannot open {}", start.display()))?;
    let sys = SystemRunner::with_timeout(TOOL_TIMEOUT);
    let home = crate::paths::user_home(&sys)?;
    let env = CleanEnv {
        gradle_home: crate::paths::gradle_user_home(&sys, &home),
        home,
        project: project::detect(&start).ok().map(|p| p.root),
        own_pid: std::process::id(),
        now_secs: now_secs(),
    };
    eprintln!("Measuring… (large folders take a moment)");
    let found = plan(&sys, &env);

    let stdin = std::io::stdin();
    // Questions are asked on the screen and answered on the keyboard: both
    // ends must be a person's terminal, or the answer is to a question
    // nobody saw.
    let interactive = stdin.is_terminal() && std::io::stdout().is_terminal();
    let mut out = std::io::stdout().lock();
    let options = Options {
        delete: args.delete,
        yes: args.yes,
        include: args.include,
        only: args.only,
        interactive,
    };
    session(&found, &env, &sys, &options, &mut stdin.lock(), &mut out)
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

pub struct Options {
    pub delete: bool,
    pub yes: bool,
    pub include: Vec<String>,
    pub only: Vec<String>,
    /// Input is a terminal, so a question can be asked.
    pub interactive: bool,
}

/// Prints the plan and, if asked, cleans. Returns the exit code.
pub fn session(
    found: &Plan,
    env: &CleanEnv,
    sys: &dyn System,
    options: &Options,
    input: &mut dyn BufRead,
    out: &mut dyn Write,
) -> Result<i32> {
    let chosen = selection(found, &options.include, &options.only);
    write!(out, "{}", render_plan(found, &chosen, options.delete))?;
    if !options.delete {
        return Ok(0);
    }
    if chosen.is_empty() {
        writeln!(out, "\nNothing selected to clean.")?;
        return Ok(0);
    }
    if !options.yes && !options.interactive {
        return Err(eyre!(
            "refusing to delete without a terminal to ask on: add --yes to confirm in advance"
        ));
    }
    let mut freed = 0u64;
    let mut failed = false;
    for category in found.categories.iter().filter(|c| chosen.contains(&c.id)) {
        writeln!(
            out,
            "\n{} — {} ({})",
            category.title,
            count_text(category),
            total_text(category.bytes())
        )?;
        if !options.yes && !confirm(input, out, "Delete these?")? {
            writeln!(out, "  skipped")?;
            continue;
        }
        let outcomes = execute(found, &[category.id], sys, env);
        freed = freed.saturating_add(report_outcomes(&outcomes, out, &mut failed)?);
    }
    writeln!(out, "\nFreed about {}.", total_text(freed))?;
    Ok(i32::from(failed))
}

fn report_outcomes(outcomes: &[Outcome], out: &mut dyn Write, failed: &mut bool) -> Result<u64> {
    let mut freed = 0u64;
    for outcome in outcomes {
        match &outcome.result {
            Ok(bytes) => {
                freed = freed.saturating_add(*bytes);
                writeln!(
                    out,
                    "  ✓ {} ({})",
                    printable(&outcome.label),
                    total_text(*bytes)
                )?;
            }
            Err(why) => {
                *failed = true;
                writeln!(out, "  ✗ {}: {}", printable(&outcome.label), printable(why))?;
            }
        }
    }
    Ok(freed)
}

/// Asks `question` and reads one line; only "y"/"yes" agree.
pub fn confirm(input: &mut dyn BufRead, out: &mut dyn Write, question: &str) -> Result<bool> {
    write!(out, "{question} [y/N] ")?;
    out.flush()?;
    let mut line = String::new();
    input.read_line(&mut line)?;
    Ok(matches!(line.trim().to_lowercase().as_str(), "y" | "yes"))
}

/// Text that came from outside, safe to print: no control characters.
fn printable(text: &str) -> String {
    lf_mide_core::git::clean_text(text)
}

fn count_text(category: &Category) -> String {
    match category.items.len() {
        1 => "1 item".to_string(),
        n => format!("{n} items"),
    }
}

/// The plan as text. `chosen` are the categories a `--delete` would clean.
pub fn render_plan(found: &Plan, chosen: &[&str], deleting: bool) -> String {
    let mut text = String::new();
    let heading = if deleting {
        "LF-Mide clean"
    } else {
        "LF-Mide clean — what could be freed (nothing has been changed)"
    };
    text.push_str(&format!("{heading}\n"));
    let sections = [
        (
            "Rebuildable: cleaned by --delete",
            Safety::Rebuildable,
            false,
        ),
        (
            "Careful: only with --include <name>",
            Safety::Careful,
            false,
        ),
        (
            "For you to review: LF-Mide only shows the commands",
            Safety::Careful,
            true,
        ),
    ];
    for (title, safety, report) in sections {
        let members: Vec<&Category> = found
            .categories
            .iter()
            .filter(|c| (c.action == Action::ReportOnly) == report && c.safety == safety)
            .collect();
        if members.is_empty() {
            continue;
        }
        text.push_str(&format!("\n{title}\n"));
        for category in members {
            text.push_str(&render_category(category, chosen.contains(&category.id)));
        }
    }
    let total: u64 = found
        .categories
        .iter()
        .filter(|c| chosen.contains(&c.id))
        .map(Category::bytes)
        .fold(0, u64::saturating_add);
    text.push_str(&format!(
        "\nDefault `--delete` would free about {}.\n",
        total_text(total)
    ));
    if !deleting {
        text.push_str(
            "Nothing was deleted. Add --delete to clean the rebuildable ones (it asks first).\n",
        );
    }
    text
}

fn render_category(category: &Category, chosen: bool) -> String {
    let mark = if category.blocked.is_some() {
        "✗"
    } else if chosen {
        "●"
    } else {
        "○"
    };
    let mut text = format!(
        "  {mark} {:<24} {:>7}  {}\n",
        category.id,
        total_text(category.bytes()),
        printable(&category.title)
    );
    text.push_str(&format!("      {}\n", category.why));
    if let Some(reason) = &category.blocked {
        text.push_str(&format!("      blocked: {reason}\n"));
    }
    let shown = if category.action == Action::ReportOnly {
        SHOWN_REPORT_ITEMS
    } else {
        SHOWN_ITEMS
    };
    let mut items: Vec<_> = category.items.iter().collect();
    items.sort_by_key(|i| std::cmp::Reverse(i.bytes));
    for item in items.iter().take(shown) {
        text.push_str(&format!(
            "      {:>7}  {}\n",
            total_text(item.bytes),
            printable(&item.label)
        ));
    }
    if items.len() > shown {
        text.push_str(&format!("      … and {} more\n", items.len() - shown));
    }
    for line in &category.advice {
        text.push_str(&format!("      $ {}\n", printable(line)));
    }
    text
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use lf_mide_system::fake::FakeSystem;
    use lf_mide_tidy::Item;

    use super::*;

    struct World {
        _tmp: tempfile::TempDir,
        home: std::path::PathBuf,
    }

    fn world() -> World {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = fs::canonicalize(tmp.path()).unwrap();
        World { _tmp: tmp, home }
    }

    fn env(w: &World) -> CleanEnv {
        CleanEnv {
            home: w.home.clone(),
            gradle_home: w.home.join(".gradle"),
            project: None,
            own_pid: std::process::id(),
            now_secs: 0,
        }
    }

    fn delete_category(id: &'static str, safety: Safety, base: &Path, names: &[&str]) -> Category {
        let items = names
            .iter()
            .map(|name| {
                fs::create_dir_all(base.join(name)).unwrap();
                fs::write(base.join(name).join("f"), vec![1u8; 20_000]).unwrap();
                Item {
                    label: (*name).to_string(),
                    path: Some(base.join(name)),
                    base: Some(base.to_path_buf()),
                    bytes: 20_000,
                }
            })
            .collect();
        Category {
            id,
            title: format!("Title of {id}"),
            why: "because".to_string(),
            safety,
            action: Action::Delete,
            items,
            advice: Vec::new(),
            blocked: None,
        }
    }

    fn report_only() -> Category {
        Category {
            id: "simulator-runtimes",
            title: "Runtimes".into(),
            why: "huge".into(),
            safety: Safety::Careful,
            action: Action::ReportOnly,
            items: vec![Item {
                label: "iOS 18.2 · unused".into(),
                path: None,
                base: None,
                bytes: 8_000_000_000,
            }],
            advice: vec!["xcrun simctl runtime delete AAAA".into()],
            blocked: None,
        }
    }

    fn options(delete: bool, yes: bool, interactive: bool) -> Options {
        Options {
            delete,
            yes,
            include: vec![],
            only: vec![],
            interactive,
        }
    }

    fn run(found: &Plan, w: &World, opts: &Options, typed: &str) -> (Result<i32>, String) {
        let mut input = std::io::Cursor::new(typed.to_string());
        let mut out = Vec::new();
        let result = session(
            found,
            &env(w),
            &FakeSystem::new(),
            opts,
            &mut input,
            &mut out,
        );
        (result, String::from_utf8(out).unwrap())
    }

    fn sample(w: &World) -> Plan {
        Plan {
            categories: vec![
                delete_category(
                    "derived-data",
                    Safety::Rebuildable,
                    &w.home.join("dd"),
                    &["App", "Other"],
                ),
                delete_category(
                    "device-support",
                    Safety::Careful,
                    &w.home.join("ds"),
                    &["17.0"],
                ),
                report_only(),
            ],
        }
    }

    #[test]
    fn looking_shows_the_three_groups_and_deletes_nothing() {
        let w = world();
        let found = sample(&w);

        let (code, text) = run(&found, &w, &options(false, false, true), "");

        assert_eq!(code.unwrap(), 0);
        assert!(text.contains("nothing has been changed"));
        assert!(text.contains("Rebuildable: cleaned by --delete"));
        assert!(text.contains("Careful: only with --include <name>"));
        assert!(text.contains("For you to review"));
        assert!(text.contains("● derived-data") && text.contains("○ device-support"));
        assert!(text.contains("$ xcrun simctl runtime delete AAAA"));
        assert!(text.contains("7.5G"), "{text}");
        assert!(text.contains("Nothing was deleted. Add --delete"));
        assert!(w.home.join("dd/App/f").exists() && w.home.join("ds/17.0/f").exists());
    }

    #[test]
    fn yes_deletes_the_rebuildable_ones_and_leaves_the_careful_ones() {
        let w = world();
        let found = sample(&w);

        let (code, text) = run(&found, &w, &options(true, true, false), "");

        assert_eq!(code.unwrap(), 0);
        assert!(!w.home.join("dd/App").exists() && !w.home.join("dd/Other").exists());
        assert!(w.home.join("dd").exists(), "the folder itself stays");
        assert!(
            w.home.join("ds/17.0/f").exists(),
            "careful ones need --include"
        );
        assert!(text.contains("✓ App") && text.contains("Freed about"));
    }

    #[test]
    fn including_a_careful_category_cleans_it_too() {
        let w = world();
        let found = sample(&w);
        let mut opts = options(true, true, false);
        opts.include = vec!["device-support".to_string()];

        let (code, _) = run(&found, &w, &opts, "");

        assert_eq!(code.unwrap(), 0);
        assert!(!w.home.join("ds/17.0").exists());
    }

    #[test]
    fn only_limits_the_run_to_the_named_categories() {
        let w = world();
        let found = sample(&w);
        let mut opts = options(true, true, false);
        opts.include = vec!["device-support".to_string()];
        opts.only = vec!["device-support".to_string()];

        run(&found, &w, &opts, "").0.unwrap();

        assert!(w.home.join("dd/App/f").exists());
        assert!(!w.home.join("ds/17.0").exists());
    }

    #[test]
    fn without_yes_each_category_is_asked_about() {
        let w = world();
        let found = Plan {
            categories: vec![
                delete_category(
                    "derived-data",
                    Safety::Rebuildable,
                    &w.home.join("dd"),
                    &["App"],
                ),
                delete_category(
                    "gradle-logs",
                    Safety::Rebuildable,
                    &w.home.join("gw"),
                    &["gradle-8.0-bin"],
                ),
            ],
        };

        let (code, text) = run(&found, &w, &options(true, false, true), "n\ny\n");

        assert_eq!(code.unwrap(), 0);
        assert!(w.home.join("dd/App/f").exists(), "declined");
        assert!(!w.home.join("gw/gradle-8.0-bin").exists(), "agreed");
        assert!(text.contains("Delete these? [y/N]") && text.contains("skipped"));
    }

    #[test]
    fn an_empty_answer_or_end_of_input_means_no() {
        let w = world();
        let found = sample(&w);

        run(&found, &w, &options(true, false, true), "\n")
            .0
            .unwrap();
        run(&found, &w, &options(true, false, true), "").0.unwrap();

        assert!(w.home.join("dd/App/f").exists());
    }

    #[test]
    fn without_a_terminal_and_without_yes_it_refuses_to_delete() {
        let w = world();
        let found = sample(&w);

        let (result, _) = run(&found, &w, &options(true, false, false), "y\n");

        assert!(result.unwrap_err().to_string().contains("--yes"));
        assert!(w.home.join("dd/App/f").exists());
    }

    #[test]
    fn a_failed_item_is_reported_and_makes_the_exit_code_nonzero() {
        let w = world();
        let mut found = sample(&w);
        fs::remove_dir_all(w.home.join("dd/App")).unwrap();
        found.categories.truncate(1);

        let (code, text) = run(&found, &w, &options(true, true, false), "");

        assert_eq!(code.unwrap(), 1);
        assert!(text.contains("✗ App: already gone"), "{text}");
        assert!(!w.home.join("dd/Other").exists(), "the others still went");
    }

    #[test]
    fn a_blocked_category_is_shown_with_its_reason_and_never_run() {
        let w = world();
        let mut found = sample(&w);
        found.categories[1].blocked = Some("an emulator is running: stop it first".into());
        let mut opts = options(true, true, false);
        opts.include = vec!["device-support".to_string()];

        let (_, text) = run(&found, &w, &opts, "");

        assert!(
            text.contains("✗ device-support") && text.contains("blocked: an emulator is running")
        );
        assert!(w.home.join("ds/17.0/f").exists());
    }

    #[test]
    fn nothing_selected_says_so() {
        let w = world();
        let found = Plan {
            categories: vec![report_only()],
        };

        let (code, text) = run(&found, &w, &options(true, true, false), "");

        assert_eq!(code.unwrap(), 0);
        assert!(text.contains("Nothing selected to clean."));
    }

    #[test]
    fn long_lists_are_cut_with_a_count() {
        let w = world();
        let names: Vec<String> = (0..9).map(|n| format!("item{n}")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let found = Plan {
            categories: vec![delete_category(
                "derived-data",
                Safety::Rebuildable,
                &w.home.join("dd"),
                &refs,
            )],
        };

        let text = render_plan(&found, &["derived-data"], false);

        assert!(text.contains("… and 3 more"));
    }

    #[test]
    fn only_yes_agrees_to_a_question() {
        let ask = |typed: &str| {
            let mut input = std::io::Cursor::new(typed.to_string());
            confirm(&mut input, &mut Vec::new(), "Go?").unwrap()
        };

        assert!(ask("y\n") && ask("YES\n") && ask(" Yes \n"));
        assert!(!ask("n\n") && !ask("\n") && !ask("yep\n") && !ask(""));
    }
}
