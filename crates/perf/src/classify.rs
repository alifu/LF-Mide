//! Deciding which processes matter and what they are.

use std::collections::{BTreeMap, BTreeSet};

use lf_mide_core::{ProcessKind, ProcessStat};

use crate::parse::PsRow;

const GRADLE_DAEMON_CLASS: &str = "org.gradle.launcher.daemon.bootstrap.GradleDaemon";
const KOTLIN_DAEMON_CLASS: &str = "org.jetbrains.kotlin.daemon.KotlinCompileDaemon";
const DEVICES_DIR: &str = "/Devices/";
const UDID_LEN: usize = 36;

/// What `args` (a process's command line) is, and which device or AVD it
/// belongs to. The program is looked at, not just any word in the line, so a
/// shell or `grep` that merely mentions "GradleDaemon" is not one.
///
/// Anything in the command line can be chosen by a repository's scripts, so
/// the label is a good guess, not proof; stopping a process re-checks it
/// (`is_still`) and never offers editors or IDEs.
pub fn classify(args: &str, pid: u32, own_pid: u32) -> Option<(ProcessKind, String)> {
    if pid == own_pid {
        return Some((ProcessKind::LfMide, String::new()));
    }
    let first = args.split_whitespace().next()?;
    // The program path runs up to the first option; it may hold spaces
    // (`/Users/John Smith/Library/Android/sdk/...`).
    let head = args.split(" -").next().unwrap_or(args);
    let names = [basename(first), basename(head)];
    let none = String::new;

    if let Some(rest) = java_arguments(args) {
        return classify_java(args, &rest);
    }
    if names.iter().any(|n| is_kotlin_lsp_launcher(n)) {
        return Some((ProcessKind::KotlinLsp, none()));
    }
    if basename(first) == "kmp-lsp" {
        return Some((ProcessKind::KmpLsp, none()));
    }
    if names.iter().any(|n| n.starts_with("qemu-system")) {
        // Other virtual machines (Colima, UTM, Docker) use qemu too; an
        // Android emulator always names its virtual device.
        return avd_name(args)
            .filter(|name| !name.is_empty())
            .map(|name| (ProcessKind::Emulator, name));
    }
    if basename(first) == "launchd_sim" {
        return Some((
            ProcessKind::Simulator,
            simulator_udid(args).unwrap_or_default(),
        ));
    }
    if names.contains(&"nvim") {
        return Some((ProcessKind::Neovim, none()));
    }
    if head.ends_with("/Xcode.app/Contents/MacOS/Xcode") {
        return Some((ProcessKind::Xcode, none()));
    }
    // The launcher is called `studio` whatever the app is named (a renamed
    // or preview copy: "AS Canary.app").
    if args.starts_with('/') && args.contains(".app/Contents/MacOS/studio") {
        return Some((ProcessKind::AndroidStudio, none()));
    }
    None
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn is_kotlin_lsp_launcher(name: &str) -> bool {
    name == "kotlin-lsp" || name == "kotlin-language-server"
}

/// The words after the `java` program, if `args` is a `java` command that
/// was started directly (not wrapped in `sh -c '...'` or `ssh host '...'`).
fn java_arguments(args: &str) -> Option<Vec<&str>> {
    let first = args.split_whitespace().next()?;
    if basename(first) == "java" {
        return Some(args.split_whitespace().skip(1).collect());
    }
    // A java inside a path with spaces (Android Studio's bundled one).
    let at = args.find("/bin/java ")?;
    let before = &args[..at];
    let direct = args.starts_with('/') && !before.contains(" -") && !before.contains('\'');
    direct.then(|| args[at + "/bin/java ".len()..].split_whitespace().collect())
}

fn classify_java(args: &str, words: &[&str]) -> Option<(ProcessKind, String)> {
    if words.contains(&GRADLE_DAEMON_CLASS) {
        return Some((ProcessKind::GradleDaemon, gradle_distribution(args)));
    }
    let kind = if words.contains(&KOTLIN_DAEMON_CLASS) {
        ProcessKind::KotlinDaemon
    } else if args.starts_with('/')
        && args.contains(".app/Contents/jbr/")
        && (args.contains("/Android Studio") || args.contains("idea.paths.selector=AndroidStudio"))
    {
        ProcessKind::AndroidStudio
    } else if words.iter().any(|word| names_kotlin_lsp(word)) {
        ProcessKind::KotlinLsp
    } else {
        return None;
    };
    Some((kind, String::new()))
}

/// A class path entry or program that is the Kotlin language server: a
/// directory called exactly `kotlin-lsp` (Homebrew's cellar), its jar, or the
/// `kotlin-language-server` launcher. A project folder that merely starts
/// with the name does not count.
fn names_kotlin_lsp(word: &str) -> bool {
    let last = basename(word);
    word.split('/').any(|part| part == "kotlin-lsp")
        || (last.starts_with("kotlin-lsp") && last.ends_with(".jar"))
        || last == "kotlin-language-server"
}

/// The Gradle version a daemon runs from: the folder under
/// `wrapper/dists/` named in its class path (`gradle-8.14-bin`), or empty.
fn gradle_distribution(args: &str) -> String {
    let marker = "wrapper/dists/";
    let Some(at) = args.find(marker) else {
        return String::new();
    };
    args[at + marker.len()..]
        .split('/')
        .next()
        .filter(|name| name.starts_with("gradle-"))
        .map(|name| name.chars().take(60).collect())
        .unwrap_or_default()
}

/// `-avd NAME` or `@NAME` from an emulator command line.
fn avd_name(args: &str) -> Option<String> {
    let mut words = args.split_whitespace();
    while let Some(word) = words.next() {
        if word == "-avd" {
            return words.next().map(str::to_string);
        }
        if let Some(name) = word.strip_prefix('@') {
            return Some(name.to_string());
        }
    }
    None
}

/// The device id after `/Devices/` in a `launchd_sim` command line.
fn simulator_udid(args: &str) -> Option<String> {
    let after = args.split(DEVICES_DIR).nth(1)?;
    let udid: String = after.chars().take(UDID_LEN).collect();
    let shaped = udid.len() == UDID_LEN && udid.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
    shaped.then_some(udid)
}

/// The watched processes, biggest memory use first. A simulator counts as
/// one entry: its `launchd_sim` plus everything running inside it.
pub fn collect(rows: &[PsRow], own_pid: u32) -> Vec<ProcessStat> {
    collect_with_members(rows, own_pid)
        .into_iter()
        .map(|(stat, _)| stat)
        .collect()
}

/// Like [`collect`], and for each entry the processes it adds up, as
/// `(pid, resident bytes)`.
pub fn collect_with_members(rows: &[PsRow], own_pid: u32) -> Vec<(ProcessStat, Vec<(u32, u64)>)> {
    let mut children: BTreeMap<u32, Vec<&PsRow>> = BTreeMap::new();
    for row in rows {
        children.entry(row.ppid).or_default().push(row);
    }
    let mut inside_simulators = BTreeSet::new();
    let mut found = Vec::new();
    for row in rows {
        let Some((ProcessKind::Simulator, detail)) = classify(&row.args, row.pid, own_pid) else {
            continue;
        };
        let tree = subtree(row, &children);
        inside_simulators.extend(tree.iter().map(|r| r.pid).filter(|pid| *pid != row.pid));
        let stat = ProcessStat {
            kind: ProcessKind::Simulator,
            pid: row.pid,
            rss: tree.iter().fold(0u64, |sum, r| sum.saturating_add(r.rss)),
            cpu_tenths: tree
                .iter()
                .fold(0u32, |sum, r| sum.saturating_add(r.cpu_tenths)),
            detail,
        };
        found.push((stat, tree.iter().map(|r| (r.pid, r.rss)).collect()));
    }
    for row in rows {
        if inside_simulators.contains(&row.pid) {
            continue;
        }
        let Some((kind, detail)) = classify(&row.args, row.pid, own_pid) else {
            continue;
        };
        if kind != ProcessKind::Simulator {
            let stat = ProcessStat {
                kind,
                pid: row.pid,
                rss: row.rss,
                cpu_tenths: row.cpu_tenths,
                detail,
            };
            found.push((stat, vec![(row.pid, row.rss)]));
        }
    }
    found.sort_by(|(a, _), (b, _)| b.rss.cmp(&a.rss).then(a.pid.cmp(&b.pid)));
    found
}

/// `root` and all its descendants (a pid is visited once, so a loop in the
/// data cannot hang this).
fn subtree<'a>(root: &'a PsRow, children: &BTreeMap<u32, Vec<&'a PsRow>>) -> Vec<&'a PsRow> {
    let mut seen = BTreeSet::from([root.pid]);
    let mut all = vec![root];
    let mut next = 0;
    while next < all.len() {
        let pid = all[next].pid;
        next += 1;
        for child in children.get(&pid).into_iter().flatten() {
            if seen.insert(child.pid) {
                all.push(child);
            }
        }
    }
    all
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse_ps;

    const OWN: u32 = 3790;

    fn kinds_of(args: &str) -> Option<ProcessKind> {
        classify(args, 1, OWN).map(|(kind, _)| kind)
    }

    #[test]
    fn the_java_daemons_are_told_apart_by_their_main_class() {
        let java = "/Library/Java/jdk/bin/java -cp x.jar";

        assert_eq!(
            kinds_of(&format!(
                "{java} org.gradle.launcher.daemon.bootstrap.GradleDaemon 8.14"
            )),
            Some(ProcessKind::GradleDaemon)
        );
        assert_eq!(
            kinds_of(&format!(
                "{java} org.jetbrains.kotlin.daemon.KotlinCompileDaemon --x"
            )),
            Some(ProcessKind::KotlinDaemon)
        );
        assert_eq!(
            kinds_of(&format!(
                "{java} -jar /opt/homebrew/Cellar/kotlin-lsp/0.2/kotlin-lsp.jar"
            )),
            Some(ProcessKind::KotlinLsp)
        );
        assert_eq!(kinds_of(&format!("{java} -jar app.jar")), None);
    }

    #[test]
    fn a_gradle_daemon_names_the_distribution_it_runs_from() {
        let wrapper = "/usr/bin/java -cp /Users/me/.gradle/wrapper/dists/gradle-7.6-bin/abc123/gradle-7.6/lib/gradle-launcher-7.6.jar org.gradle.launcher.daemon.bootstrap.GradleDaemon 7.6";
        let installed = "/usr/bin/java -cp /opt/homebrew/Cellar/gradle/8.5/libexec/lib/x.jar org.gradle.launcher.daemon.bootstrap.GradleDaemon 8.5";

        assert_eq!(
            classify(wrapper, 1, OWN),
            Some((ProcessKind::GradleDaemon, "gradle-7.6-bin".to_string()))
        );
        assert_eq!(
            classify(installed, 1, OWN),
            Some((ProcessKind::GradleDaemon, String::new()))
        );
    }

    #[test]
    fn kmp_lsp_is_found_by_its_program() {
        assert_eq!(
            kinds_of("/Users/me/.cargo/bin/kmp-lsp"),
            Some(ProcessKind::KmpLsp)
        );
        assert_eq!(
            kinds_of("kmp-lsp --index-only /p"),
            Some(ProcessKind::KmpLsp)
        );
        assert_eq!(kinds_of("cargo install kmp-lsp"), None);
        assert_eq!(kinds_of("/bin/zsh -c kmp-lsp"), None);
    }

    #[test]
    fn a_command_that_only_mentions_a_daemon_is_not_one() {
        for line in [
            "grep org.gradle.launcher.daemon.bootstrap.GradleDaemon",
            "/bin/zsh -c brew uninstall kotlin-lsp",
            "vim GradleDaemon.kt",
            "ps -ef | grep KotlinCompileDaemon",
        ] {
            assert_eq!(kinds_of(line), None, "{line}");
        }
    }

    #[test]
    fn the_emulator_is_found_by_its_qemu_process_and_names_its_avd() {
        let (kind, avd) = classify(
            "/sdk/emulator/qemu/darwin-aarch64/qemu-system-aarch64 -avd Pixel_8 -x",
            5,
            OWN,
        )
        .unwrap();
        let (_, at_form) = classify(
            "/sdk/emulator/qemu/qemu-system-x86_64-headless @Tablet_API_34",
            6,
            OWN,
        )
        .unwrap();

        assert_eq!(kind, ProcessKind::Emulator);
        assert_eq!(avd, "Pixel_8");
        assert_eq!(at_form, "Tablet_API_34");
        assert_eq!(classify("/sdk/qemu-system-aarch64", 7, OWN), None);
        assert_eq!(
            kinds_of("/sdk/emulator/emulator -avd Pixel_8"),
            None,
            "the launcher is not counted twice"
        );
    }

    #[test]
    fn other_virtual_machines_are_not_android_emulators() {
        for line in [
            "/opt/homebrew/bin/qemu-system-aarch64 -machine virt -m 4096 -drive file=ubuntu.qcow2",
            "/Applications/UTM.app/Contents/XPCServices/QEMUHelper.xpc/qemu-system-x86_64 -vnc :1",
            "/usr/local/bin/qemu-system-aarch64",
        ] {
            assert_eq!(kinds_of(line), None, "{line}");
        }
    }

    #[test]
    fn programs_in_paths_with_spaces_are_still_recognised() {
        let (kind, avd) = classify(
            "/Users/John Smith/Library/Android/sdk/emulator/qemu/darwin-aarch64/qemu-system-aarch64 -avd Pixel_8 -x",
            1,
            OWN,
        )
        .unwrap();

        assert_eq!((kind, avd.as_str()), (ProcessKind::Emulator, "Pixel_8"));
        assert_eq!(
            kinds_of("/Users/John Smith/bin/nvim --embed"),
            Some(ProcessKind::Neovim)
        );
        assert_eq!(
            kinds_of(
                "/Users/John Smith/.gradle/jdks/jdk-21/Contents/Home/bin/java -cp x org.gradle.launcher.daemon.bootstrap.GradleDaemon 8.14"
            ),
            Some(ProcessKind::GradleDaemon)
        );
    }

    #[test]
    fn a_java_command_wrapped_in_something_else_is_not_a_daemon() {
        for line in [
            "sh -c /usr/bin/java -cp x org.gradle.launcher.daemon.bootstrap.GradleDaemon 8.14",
            "ssh devbox '/usr/bin/java -cp x org.gradle.launcher.daemon.bootstrap.GradleDaemon'",
            "/bin/zsh -c /opt/jdk/bin/java -jar kotlin-lsp.jar",
        ] {
            assert_eq!(kinds_of(line), None, "{line}");
        }
    }

    #[test]
    fn a_java_app_in_a_folder_that_only_resembles_the_language_server_is_left_alone() {
        let java = "/usr/bin/java";

        assert_eq!(
            kinds_of(&format!(
                "{java} -jar myapp.jar --dir /Users/me/code/kotlin-lsp-demo"
            )),
            None
        );
        assert_eq!(
            kinds_of(&format!(
                "{java} -cp /opt/homebrew/Cellar/kotlin-lsp/0.2/lib/x.jar Main"
            )),
            Some(ProcessKind::KotlinLsp)
        );
        assert_eq!(
            kinds_of(&format!("{java} -jar /opt/kotlin-lsp-0.253.jar")),
            Some(ProcessKind::KotlinLsp)
        );
    }

    #[test]
    fn a_simulator_is_launchd_sim_with_its_device_id() {
        let (kind, udid) = classify(
            "launchd_sim /Users/me/Library/Developer/CoreSimulator/Devices/23F0A11F-0DD8-4766-853D-987DE9852BAC/data/x.plist",
            1,
            OWN,
        )
        .unwrap();
        let (_, odd) = classify("launchd_sim /Devices/not-a-udid/data", 2, OWN).unwrap();

        assert_eq!(kind, ProcessKind::Simulator);
        assert_eq!(udid, "23F0A11F-0DD8-4766-853D-987DE9852BAC");
        assert_eq!(odd, "", "an id that does not look like a UDID is dropped");
    }

    #[test]
    fn android_studio_is_recognised_under_any_app_name() {
        assert_eq!(
            kinds_of("/Applications/AS Canary.app/Contents/MacOS/studio"),
            Some(ProcessKind::AndroidStudio)
        );
        assert_eq!(
            kinds_of(
                "/Users/me/Apps/Preview.app/Contents/jbr/Contents/Home/bin/java -Didea.paths.selector=AndroidStudioPreview2025.1 -x"
            ),
            Some(ProcessKind::AndroidStudio)
        );
        assert_eq!(
            kinds_of(
                "/Applications/GoLand.app/Contents/jbr/Contents/Home/bin/java -Didea.paths.selector=GoLand2025.1"
            ),
            None,
            "other JetBrains IDEs are not Android Studio"
        );
        assert_eq!(
            kinds_of("/Applications/GoLand.app/Contents/MacOS/goland"),
            None
        );
    }

    #[test]
    fn editors_ides_and_ourselves() {
        assert_eq!(kinds_of("nvim --embed"), Some(ProcessKind::Neovim));
        assert_eq!(
            kinds_of("/opt/homebrew/bin/nvim a.kt"),
            Some(ProcessKind::Neovim)
        );
        assert_eq!(
            kinds_of("/Applications/Xcode.app/Contents/MacOS/Xcode"),
            Some(ProcessKind::Xcode)
        );
        assert_eq!(
            kinds_of("/Applications/Android Studio.app/Contents/MacOS/studio"),
            Some(ProcessKind::AndroidStudio)
        );
        assert_eq!(
            kinds_of("/Applications/Android Studio.app/Contents/jbr/Contents/Home/bin/java -Dx"),
            Some(ProcessKind::AndroidStudio)
        );
        assert_eq!(
            classify("anything at all", OWN, OWN).unwrap().0,
            ProcessKind::LfMide
        );
        assert_eq!(kinds_of("/usr/bin/vim"), None);
        assert_eq!(kinds_of(""), None);
    }

    #[test]
    fn collecting_the_fixture_lists_the_right_things_biggest_first() {
        let stats = collect(&parse_ps(include_str!("../fixtures/ps.txt")), OWN);

        let kinds: Vec<ProcessKind> = stats.iter().map(|s| s.kind).collect();
        assert_eq!(
            kinds,
            [
                ProcessKind::Xcode,
                ProcessKind::Emulator,
                ProcessKind::AndroidStudio,
                ProcessKind::GradleDaemon,
                ProcessKind::KotlinLsp,
                ProcessKind::AndroidStudio,
                ProcessKind::KotlinDaemon,
                ProcessKind::Simulator,
                ProcessKind::Neovim,
                ProcessKind::LfMide,
                ProcessKind::Neovim,
            ]
        );
        assert!(stats.windows(2).all(|w| w[0].rss >= w[1].rss));
    }

    #[test]
    fn a_simulator_is_one_entry_that_adds_up_everything_inside_it() {
        let stats = collect(&parse_ps(include_str!("../fixtures/ps.txt")), OWN);

        let sim = stats
            .iter()
            .find(|s| s.kind == ProcessKind::Simulator)
            .unwrap();

        // launchd_sim + notifyd + backboardd + the grandchild geocorrectiond
        assert_eq!(sim.rss, (13_616 + 77_280 + 80_256 + 55_552) * 1024);
        assert_eq!(sim.cpu_tenths, 20);
        assert_eq!(sim.detail, "23F0A11F-0DD8-4766-853D-987DE9852BAC");
        assert_eq!(
            stats
                .iter()
                .filter(|s| s.pid == 1407 || s.pid == 6129)
                .count(),
            0
        );
    }

    #[test]
    fn the_emulator_entry_is_the_qemu_process_not_its_launcher() {
        let stats = collect(&parse_ps(include_str!("../fixtures/ps.txt")), OWN);

        let emulators: Vec<_> = stats
            .iter()
            .filter(|s| s.kind == ProcessKind::Emulator)
            .collect();

        assert_eq!(emulators.len(), 1);
        assert_eq!(
            (emulators[0].pid, emulators[0].detail.as_str()),
            (5002, "Pixel_8_API_35")
        );
    }

    #[test]
    fn a_loop_in_the_parent_data_cannot_hang_the_walk() {
        let rows = parse_ps(
            "  10 11 100 0.0 launchd_sim /Devices/23F0A11F-0DD8-4766-853D-987DE9852BAC/x\n  11 10 100 0.0 /usr/bin/x\n",
        );

        let stats = collect(&rows, 1);

        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].rss, 200 * 1024);
    }
}
