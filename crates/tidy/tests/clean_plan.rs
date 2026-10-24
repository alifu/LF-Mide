//! `lf-mide clean` against a temporary home folder and canned tool output.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

use lf_mide_core::ProcessKind;
use lf_mide_system::fake::FakeSystem;
use lf_mide_tidy::{Action, CleanEnv, Plan, Safety, execute, plan, selection, unknown_ids};

const PS_CMD: &str = "/bin/ps -ww -axo pid=,ppid=,rss=,pcpu=,args=";

struct World {
    _tmp: tempfile::TempDir,
    home: PathBuf,
}

fn world() -> World {
    let tmp = tempfile::TempDir::new().unwrap();
    let home = fs::canonicalize(tmp.path()).unwrap();
    World { _tmp: tmp, home }
}

fn env(w: &World, project: Option<&Path>) -> CleanEnv {
    CleanEnv {
        home: w.home.clone(),
        gradle_home: w.home.join(".gradle"),
        project: project.map(Path::to_path_buf),
        own_pid: std::process::id(),
        now_secs: now_secs(),
    }
}

fn file(path: &Path, bytes: usize) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, vec![7u8; bytes]).unwrap();
}

/// `ps` output that lists this very process (a listing without it is not
/// trusted) and nothing else of interest.
fn ps_with_us(extra: &str) -> String {
    format!(
        "    1     0  19472   0.1 /sbin/launchd\n{} 1 9000 0.1 /bin/lf-mide\n{extra}",
        std::process::id()
    )
}

fn calm_machine() -> FakeSystem {
    FakeSystem::new().ok(PS_CMD, &ps_with_us(""))
}

fn category<'a>(plan: &'a Plan, id: &str) -> &'a lf_mide_tidy::Category {
    plan.categories
        .iter()
        .find(|c| c.id == id)
        .unwrap_or_else(|| {
            panic!(
                "no category {id}: {:?}",
                plan.categories.iter().map(|c| c.id).collect::<Vec<_>>()
            )
        })
}

fn set_age(path: &Path, seconds_ago: u64) {
    let when = SystemTime::now() - Duration::from_secs(seconds_ago);
    // Read-only handle: a folder cannot be opened for writing.
    fs::File::open(path).unwrap().set_modified(when).unwrap();
}

fn now_secs() -> i64 {
    let since = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap();
    i64::try_from(since.as_secs()).unwrap()
}

// --- DerivedData and friends ---

#[test]
fn derived_data_lists_each_build_and_deleting_keeps_the_folder() {
    let w = world();
    file(
        &w.home
            .join("Library/Developer/Xcode/DerivedData/App-abc/Build/x.o"),
        100_000,
    );
    file(
        &w.home
            .join("Library/Developer/Xcode/DerivedData/Other-def/y.o"),
        50_000,
    );
    file(
        &w.home.join("Library/Caches/lf-mide/derived/proj-1/z.o"),
        20_000,
    );
    let p = plan(&calm_machine(), &env(&w, None));

    let c = category(&p, "derived-data");
    assert_eq!(c.items.len(), 3);
    assert!(c.bytes() >= 170_000);
    assert_eq!(
        (c.safety, &c.action),
        (Safety::Rebuildable, &Action::Delete)
    );

    let outcomes = execute(&p, &["derived-data"], &calm_machine(), &env(&w, None));

    assert!(outcomes.iter().all(|o| o.result.is_ok()), "{outcomes:?}");
    assert!(w.home.join("Library/Developer/Xcode/DerivedData").exists());
    assert_eq!(
        fs::read_dir(w.home.join("Library/Developer/Xcode/DerivedData"))
            .unwrap()
            .count(),
        0
    );
    assert_eq!(
        fs::read_dir(w.home.join("Library/Caches/lf-mide/derived"))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn a_gradle_user_home_elsewhere_is_cleaned_and_allowed() {
    let w = world();
    let elsewhere = tempfile::TempDir::new().unwrap();
    let gradle_home = fs::canonicalize(elsewhere.path()).unwrap();
    let dir = gradle_home.join("daemon/9.5.1");
    file(&dir.join("daemon-1.out.log"), 5000);
    set_age(&dir.join("daemon-1.out.log"), 30 * 86_400);
    let mut e = env(&w, None);
    e.gradle_home = gradle_home.clone();
    let p = plan(&calm_machine(), &e);

    let outcomes = execute(&p, &["gradle-logs"], &calm_machine(), &e);

    assert!(outcomes.iter().all(|o| o.result.is_ok()), "{outcomes:?}");
    assert!(!dir.join("daemon-1.out.log").exists());
}

#[test]
fn what_is_running_is_looked_at_again_just_before_cleaning() {
    let w = world();
    file(
        &w.home
            .join(".android/avd/P.avd/snapshots/default_boot/ram.bin"),
        90_000,
    );
    let e = env(&w, None);
    let planned = plan(&calm_machine(), &e);
    assert!(category(&planned, "emulator-snapshots").is_actionable());
    // The user starts an emulator while the question waits for an answer.
    let now = busy_machine("/sdk/emulator/qemu/darwin-aarch64/qemu-system-aarch64 -avd P");

    let outcomes = execute(&planned, &["emulator-snapshots"], &now, &e);

    assert_eq!(outcomes.len(), 1);
    assert!(
        outcomes[0]
            .result
            .as_ref()
            .unwrap_err()
            .contains("not done")
    );
    assert!(
        w.home
            .join(".android/avd/P.avd/snapshots/default_boot/ram.bin")
            .exists()
    );
}

#[test]
fn a_hostile_simulator_name_cannot_start_a_new_command_line() {
    let w = world();
    let devices = r#"{"devices":{"com.apple.CoreSimulator.SimRuntime.iOS-26-0":[
      {"name":"iPhone\n$ rm -rf ~\u001b[2J","udid":"DDDDDDDD-4444-4444-4444-444444444444","state":"Shutdown"}]}}"#;
    let sys = calm_machine().ok("/usr/bin/xcrun simctl list devices -j", devices);

    let p = plan(&sys, &env(&w, None));

    let c = category(&p, "simulators");
    for line in c.advice.iter().chain(c.items.iter().map(|i| &i.label)) {
        assert!(!line.chars().any(char::is_control), "{line:?}");
    }
    assert!(c.advice[0].starts_with("xcrun simctl delete DDDD"));
}

#[test]
fn nothing_to_clean_means_no_category() {
    let w = world();

    let p = plan(&calm_machine(), &env(&w, None));

    assert!(
        p.categories
            .iter()
            .all(|c| c.id != "derived-data" && c.id != "gradle-wrappers")
    );
}

// --- the project's own build folders ---

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .args([
            "-c",
            "user.name=T",
            "-c",
            "user.email=t@e",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(dir)
        .stdout(Stdio::null())
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

fn project(w: &World) -> PathBuf {
    let root = w.home.join("code/app");
    file(&root.join(".gitignore"), 0);
    fs::write(root.join(".gitignore"), "build/\n.gradle/\n").unwrap();
    file(&root.join("shared/src/App.kt"), 10);
    file(&root.join("shared/build/out.bin"), 80_000);
    file(&root.join("androidApp/build/out.bin"), 60_000);
    file(&root.join("build/out.bin"), 40_000);
    file(&root.join(".gradle/cache.bin"), 30_000);
    // Not ignored: a folder named build that is part of the source.
    file(&root.join("tools/build/script.sh"), 100);
    file(&root.join("tools/x.txt"), 1);
    file(&root.join("settings.gradle.kts"), 20);
    git(&root, &["init", "-q", "-b", "main"]);
    git(
        &root,
        &[
            "add",
            "-A",
            "-f",
            "tools",
            "shared/src",
            ".gitignore",
            "settings.gradle.kts",
        ],
    );
    git(&root, &["commit", "-q", "-m", "init"]);
    fs::canonicalize(root).unwrap()
}

#[test]
fn only_git_ignored_build_folders_are_offered() {
    let w = world();
    let root = project(&w);

    let p = plan(&calm_machine(), &env(&w, Some(&root)));

    let c = category(&p, "project-build");
    let labels: Vec<&str> = c.items.iter().map(|i| i.label.as_str()).collect();
    assert_eq!(
        labels,
        [".gradle", "build", "androidApp/build", "shared/build"]
    );
    assert!(!labels.contains(&"tools/build"), "tracked, so it is source");
}

#[test]
fn a_folder_that_differs_only_in_case_is_never_taken_for_the_ignored_one() {
    let w = world();
    let root = project(&w);
    // Committed from another system: tracked scripts under `Build/`, while
    // the usual `build/` pattern still matches the name on a disk that
    // ignores case.
    file(&root.join("Build/run.sh"), 100);
    git(&root, &["add", "-f", "Build/run.sh"]);
    git(&root, &["commit", "-q", "-m", "scripts"]);

    let p = plan(&calm_machine(), &env(&w, Some(&root)));

    let labels: Vec<String> = p
        .categories
        .iter()
        .filter(|c| c.id == "project-build")
        .flat_map(|c| c.items.iter().map(|i| i.label.clone()))
        .collect();
    assert!(
        labels.iter().all(|l| l.to_lowercase() != "build"),
        "{labels:?}"
    );
    assert!(root.join("Build/run.sh").exists());
}

#[test]
fn build_folders_inside_a_nested_repository_are_left_alone() {
    let w = world();
    let root = project(&w);
    let inner = root.join("vendor/lib");
    file(&inner.join("build/important.txt"), 100);
    file(&inner.join("src/x.kt"), 10);
    git(&inner, &["init", "-q", "-b", "main"]);
    git(&inner, &["add", "-A", "-f"]);
    git(&inner, &["commit", "-q", "-m", "inner"]);
    // The outer repository ignores `vendor/` entirely (a typical clone).
    fs::write(root.join(".gitignore"), "build/\n.gradle/\nvendor/\n").unwrap();

    let p = plan(&calm_machine(), &env(&w, Some(&root)));

    let labels: Vec<&str> = category(&p, "project-build")
        .items
        .iter()
        .map(|i| i.label.as_str())
        .collect();
    assert!(
        labels.iter().all(|l| !l.starts_with("vendor")),
        "{labels:?}"
    );
    let e = env(&w, Some(&root));
    execute(&p, &["project-build"], &calm_machine(), &e);
    assert!(inner.join("build/important.txt").exists());
}

#[test]
fn a_dotfiles_repository_of_the_whole_home_folder_does_not_make_everything_disposable() {
    let w = world();
    git(&w.home, &["init", "-q", "-b", "main"]);
    fs::write(w.home.join(".gitignore"), "*\n").unwrap();
    git(&w.home, &["add", "-f", ".gitignore"]);
    git(&w.home, &["commit", "-q", "-m", "dotfiles"]);
    let root = w.home.join("code/app");
    file(&root.join("settings.gradle.kts"), 20);
    file(&root.join("shared/build/hand-written.txt"), 1000);

    let p = plan(&calm_machine(), &env(&w, Some(&root)));

    let c = category(&p, "project-build");
    assert!(c.blocked.is_some() && c.items.is_empty(), "{c:?}");
    assert!(root.join("shared/build/hand-written.txt").exists());
}

#[test]
fn a_repository_that_does_not_track_the_projects_settings_is_not_trusted() {
    let w = world();
    let root = w.home.join("code/app");
    file(&root.join("settings.gradle.kts"), 20);
    file(&root.join("shared/build/out.bin"), 1000);
    fs::write(root.join(".gitignore"), "build/\n").unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["add", ".gitignore"]);
    git(&root, &["commit", "-q", "-m", "only the ignore file"]);

    let p = plan(&calm_machine(), &env(&w, Some(&root)));

    assert!(category(&p, "project-build").blocked.is_some());
}

#[test]
fn deleting_project_builds_leaves_the_sources() {
    let w = world();
    let root = project(&w);
    let e = env(&w, Some(&root));
    let p = plan(&calm_machine(), &e);

    let outcomes = execute(&p, &["project-build"], &calm_machine(), &e);

    assert!(outcomes.iter().all(|o| o.result.is_ok()), "{outcomes:?}");
    assert!(!root.join("shared/build").exists() && !root.join(".gradle").exists());
    assert!(root.join("shared/src/App.kt").exists() && root.join("tools/build/script.sh").exists());
    assert!(root.exists());
}

#[test]
fn a_project_outside_git_is_blocked_not_guessed_at() {
    let w = world();
    let root = w.home.join("plain");
    file(&root.join("build/out.bin"), 1000);

    let p = plan(&calm_machine(), &env(&w, Some(&root)));

    let c = category(&p, "project-build");
    assert!(
        c.blocked
            .as_deref()
            .unwrap()
            .contains("not in a git repository")
    );
    assert!(!c.is_actionable());
    assert!(selection(&p, &[], &[]).is_empty());
    assert!(root.join("build/out.bin").exists());
}

// --- Gradle ---

#[test]
fn old_gradle_versions_go_but_the_newest_two_and_the_projects_stay() {
    let w = world();
    let root = w.home.join("code/app");
    fs::create_dir_all(root.join("gradle/wrapper")).unwrap();
    fs::write(
        root.join("gradle/wrapper/gradle-wrapper.properties"),
        "distributionUrl=https\\://services.gradle.org/distributions/gradle-7.0-bin.zip\n",
    )
    .unwrap();
    let dists = w.home.join(".gradle/wrapper/dists");
    for (name, age_days) in [
        ("gradle-9.5-bin", 1),
        ("gradle-9.4-bin", 10),
        ("gradle-8.0-bin", 100),
        ("gradle-7.0-bin", 400),
        ("gradle-6.0-bin", 500),
    ] {
        file(&dists.join(name).join("hash/lib.jar"), 10_000);
        set_age(&dists.join(name), age_days * 86_400);
    }
    let root = fs::canonicalize(root).unwrap();

    let p = plan(&calm_machine(), &env(&w, Some(&root)));

    let names: Vec<&str> = category(&p, "gradle-wrappers")
        .items
        .iter()
        .map(|i| i.label.as_str())
        .collect();
    assert_eq!(names, ["gradle-8.0-bin", "gradle-6.0-bin"]);
}

#[test]
fn a_gradle_version_a_running_daemon_uses_is_never_offered_or_deleted() {
    let w = world();
    let dists = w.home.join(".gradle/wrapper/dists");
    for (name, age_days) in [
        ("gradle-9.5-bin", 1),
        ("gradle-9.4-bin", 10),
        ("gradle-7.6-bin", 700),
        ("gradle-6.0-bin", 800),
    ] {
        file(&dists.join(name).join("hash/lib.jar"), 10_000);
        set_age(&dists.join(name), age_days * 86_400);
    }
    let daemon = "/usr/bin/java -cp /x/.gradle/wrapper/dists/gradle-7.6-bin/h/gradle-7.6/lib/a.jar org.gradle.launcher.daemon.bootstrap.GradleDaemon 7.6";
    let busy = busy_machine(daemon);

    let p = plan(&busy, &env(&w, None));

    let names: Vec<&str> = category(&p, "gradle-wrappers")
        .items
        .iter()
        .map(|i| i.label.as_str())
        .collect();
    assert_eq!(
        names,
        ["gradle-6.0-bin"],
        "7.6 is in use by a running daemon"
    );

    // A daemon that starts after planning protects its version too.
    let mut forged = p.clone();
    forged
        .categories
        .iter_mut()
        .find(|c| c.id == "gradle-wrappers")
        .unwrap()
        .items
        .push(lf_mide_tidy::Item {
            label: "gradle-7.6-bin".into(),
            path: Some(dists.join("gradle-7.6-bin")),
            base: Some(dists.clone()),
            bytes: 1,
        });
    let outcomes = execute(&forged, &["gradle-wrappers"], &busy, &env(&w, None));
    assert!(
        outcomes[0]
            .result
            .as_ref()
            .unwrap_err()
            .contains("running Gradle daemon")
    );
    assert!(dists.join("gradle-7.6-bin/hash/lib.jar").exists());
}

#[test]
fn when_what_is_running_cannot_be_told_old_gradle_versions_are_not_cleaned() {
    let w = world();
    let dists = w.home.join(".gradle/wrapper/dists");
    for (name, age_days) in [
        ("gradle-9.5-bin", 1),
        ("gradle-9.4-bin", 10),
        ("gradle-6.0-bin", 800),
    ] {
        file(&dists.join(name).join("hash/lib.jar"), 10_000);
        set_age(&dists.join(name), age_days * 86_400);
    }
    // ps works but does not list this process: its answer is not trusted.
    let unreliable = FakeSystem::new().ok(PS_CMD, "    1     0  19472   0.1 /sbin/launchd\n");

    let p = plan(&unreliable, &env(&w, None));

    let c = category(&p, "gradle-wrappers");
    assert!(c.blocked.is_some() && !c.is_actionable());
    assert!(selection(&p, &[], &[]).is_empty());
}

#[test]
fn only_week_old_daemon_logs_are_offered() {
    let w = world();
    let dir = w.home.join(".gradle/daemon/9.5.1");
    file(&dir.join("daemon-1.out.log"), 5000);
    file(&dir.join("daemon-2.out.log"), 5000);
    file(&dir.join("registry.bin"), 5000);
    set_age(&dir.join("daemon-1.out.log"), 30 * 86_400);

    let p = plan(&calm_machine(), &env(&w, None));

    let items: Vec<&str> = category(&p, "gradle-logs")
        .items
        .iter()
        .map(|i| i.label.as_str())
        .collect();
    assert_eq!(items, ["9.5.1/daemon-1.out.log"]);
}

// --- simulators ---

const UNAVAILABLE: &str = r#"{"devices":{"com.apple.CoreSimulator.SimRuntime.iOS-17-0":[
  {"name":"iPhone 15","udid":"AAAAAAAA-0000-0000-0000-000000000001","isAvailable":false},
  {"name":"iPad","udid":"AAAAAAAA-0000-0000-0000-000000000002","isAvailable":false}]}}"#;

#[test]
fn unavailable_simulators_are_removed_by_simctl_itself() {
    let w = world();
    file(
        &w.home.join(
            "Library/Developer/CoreSimulator/Devices/AAAAAAAA-0000-0000-0000-000000000001/data/x",
        ),
        40_000,
    );
    let sys = calm_machine()
        .ok(
            "/usr/bin/xcrun simctl list devices unavailable -j",
            UNAVAILABLE,
        )
        .ok("/usr/bin/xcrun simctl delete unavailable", "");
    let e = env(&w, None);
    let p = plan(&sys, &e);

    let c = category(&p, "unavailable-simulators");
    assert_eq!(c.items.len(), 2);
    assert!(c.bytes() >= 40_000);

    let outcomes = execute(&p, &["unavailable-simulators"], &sys, &e);

    assert_eq!(outcomes.len(), 1);
    assert!(outcomes[0].result.is_ok());
}

#[test]
fn a_failing_simctl_is_reported_with_its_own_words() {
    let w = world();
    let sys = calm_machine()
        .ok(
            "/usr/bin/xcrun simctl list devices unavailable -j",
            UNAVAILABLE,
        )
        .fails(
            "/usr/bin/xcrun simctl delete unavailable",
            "CoreSimulator is busy",
        );
    let e = env(&w, None);
    let p = plan(&sys, &e);

    let outcomes = execute(&p, &["unavailable-simulators"], &sys, &e);

    assert_eq!(outcomes[0].result, Err("CoreSimulator is busy".to_string()));
}

const RUNTIMES: &str = r#"{
 "AAAAAAAA-1111-1111-1111-111111111111":{"identifier":"AAAAAAAA-1111-1111-1111-111111111111","version":"18.2","build":"22C150","sizeBytes":8706942096,"deletable":true,"runtimeIdentifier":"com.apple.CoreSimulator.SimRuntime.iOS-18-2"},
 "BBBBBBBB-2222-2222-2222-222222222222":{"identifier":"BBBBBBBB-2222-2222-2222-222222222222","version":"26.0","build":"23A343","sizeBytes":7000000000,"deletable":true,"runtimeIdentifier":"com.apple.CoreSimulator.SimRuntime.iOS-26-0"},
 "CCCCCCCC-3333-3333-3333-333333333333":{"identifier":"CCCCCCCC-3333-3333-3333-333333333333","version":"17.0","build":"21A","sizeBytes":6000000000,"deletable":false,"runtimeIdentifier":"com.apple.CoreSimulator.SimRuntime.iOS-17-0"}}"#;
const DEVICES: &str = r#"{"devices":{
 "com.apple.CoreSimulator.SimRuntime.iOS-18-2":[],
 "com.apple.CoreSimulator.SimRuntime.iOS-26-0":[{"name":"iPhone 17","udid":"DDDDDDDD-4444-4444-4444-444444444444","state":"Shutdown"}],
 "com.apple.CoreSimulator.SimRuntime.iOS-17-0":[]}}"#;

#[test]
fn runtimes_are_listed_biggest_first_and_only_unused_deletable_ones_get_a_command() {
    let w = world();
    let sys = calm_machine()
        .ok("/usr/bin/xcrun simctl runtime list -j", RUNTIMES)
        .ok("/usr/bin/xcrun simctl list devices -j", DEVICES);

    let p = plan(&sys, &env(&w, None));

    let c = category(&p, "simulator-runtimes");
    assert_eq!(c.action, Action::ReportOnly);
    assert!(
        c.items[0].label.starts_with("iOS 18.2")
            && c.items[0].label.contains("no simulators use it")
    );
    assert!(c.items[1].label.contains("1 simulators"));
    assert_eq!(c.advice.len(), 1, "{:?}", c.advice);
    assert!(
        c.advice[0].starts_with("xcrun simctl runtime delete AAAAAAAA-1111-1111-1111-111111111111")
    );
    assert!(
        selection(&p, &["simulator-runtimes".into()], &[]).is_empty(),
        "never done for you"
    );
}

#[test]
fn a_hostile_runtime_identifier_never_becomes_a_suggested_command() {
    let w = world();
    let hostile = RUNTIMES.replace("AAAAAAAA-1111-1111-1111-111111111111", "x; rm -rf ~");
    let sys = calm_machine()
        .ok("/usr/bin/xcrun simctl runtime list -j", &hostile)
        .ok("/usr/bin/xcrun simctl list devices -j", DEVICES);

    let p = plan(&sys, &env(&w, None));

    assert!(
        category(&p, "simulator-runtimes")
            .advice
            .iter()
            .all(|a| !a.contains("rm -rf"))
    );
}

#[test]
fn a_running_simulator_is_listed_but_never_offered_for_deletion() {
    let w = world();
    let running = DEVICES.replace("\"Shutdown\"", "\"Booted\"");
    file(
        &w.home.join(
            "Library/Developer/CoreSimulator/Devices/DDDDDDDD-4444-4444-4444-444444444444/data/big",
        ),
        300_000,
    );
    let sys = calm_machine().ok("/usr/bin/xcrun simctl list devices -j", &running);

    let p = plan(&sys, &env(&w, None));

    let c = category(&p, "simulators");
    assert!(c.items[0].label.ends_with("Booted"));
    assert!(
        c.advice.iter().all(|a| !a.contains("DDDDDDDD")),
        "{:?}",
        c.advice
    );
}

#[test]
fn the_simulators_with_most_data_are_listed_with_their_own_delete_command() {
    let w = world();
    file(
        &w.home.join(
            "Library/Developer/CoreSimulator/Devices/DDDDDDDD-4444-4444-4444-444444444444/data/big",
        ),
        300_000,
    );
    let sys = calm_machine().ok("/usr/bin/xcrun simctl list devices -j", DEVICES);

    let p = plan(&sys, &env(&w, None));

    let c = category(&p, "simulators");
    assert!(c.items[0].label.starts_with("iPhone 17 · iOS-26-0"));
    assert!(c.items[0].bytes >= 300_000);
    assert!(c.advice[0].starts_with("xcrun simctl delete DDDDDDDD-4444-4444-4444-444444444444"));
}

// --- Android ---

#[test]
fn virtual_devices_and_system_images_are_listed_with_commands_but_not_deleted() {
    let w = world();
    file(
        &w.home.join(".android/avd/Pixel_8.avd/userdata.img"),
        200_000,
    );
    file(&w.home.join(".android/avd/odd name; rm.avd/x"), 10);
    let sdk = w.home.join("Library/Android/sdk");
    file(
        &sdk.join("system-images/android-36/google_apis/arm64-v8a/system.img"),
        120_000,
    );
    let sys = calm_machine()
        .env("ANDROID_HOME", &sdk.display().to_string())
        .dir(&sdk.display().to_string(), &[]);

    let p = plan(&sys, &env(&w, None));

    let c = category(&p, "android-images");
    assert_eq!(c.action, Action::ReportOnly);
    assert_eq!(c.items.len(), 2, "{:?}", c.items);
    assert!(
        c.advice
            .contains(&"avdmanager delete avd -n Pixel_8".to_string())
    );
    assert!(c.advice.contains(
        &"sdkmanager --uninstall \"system-images;android-36;google_apis;arm64-v8a\"".to_string()
    ));
    assert!(
        c.advice.iter().all(|a| !a.contains("odd name")),
        "the oddly named device is skipped"
    );
}

// --- the careful ones ---

fn busy_machine(args: &str) -> FakeSystem {
    FakeSystem::new().ok(
        PS_CMD,
        &ps_with_us(&format!("  900  1  5000  0.1 {args}\n")),
    )
}

#[test]
fn emulator_snapshots_are_blocked_while_an_emulator_runs() {
    let w = world();
    file(
        &w.home
            .join(".android/avd/P.avd/snapshots/default_boot/ram.bin"),
        90_000,
    );
    let running = busy_machine("/sdk/emulator/qemu/darwin-aarch64/qemu-system-aarch64 -avd P");

    let blocked = plan(&running, &env(&w, None));
    let free = plan(&calm_machine(), &env(&w, None));

    assert!(
        category(&blocked, "emulator-snapshots")
            .blocked
            .as_deref()
            .unwrap()
            .contains("running")
    );
    assert!(category(&free, "emulator-snapshots").is_actionable());
    assert_eq!(
        category(&free, "emulator-snapshots").safety,
        Safety::Careful
    );
}

#[test]
fn when_it_cannot_tell_what_is_running_it_does_not_assume_nothing_is() {
    let w = world();
    file(
        &w.home
            .join(".android/avd/P.avd/snapshots/default_boot/ram.bin"),
        90_000,
    );
    file(
        &w.home
            .join("Library/Caches/JetBrains/AndroidStudio2024/idx"),
        90_000,
    );

    let p = plan(&FakeSystem::new(), &env(&w, None));

    assert!(category(&p, "emulator-snapshots").blocked.is_some());
    assert!(category(&p, "ide-caches").blocked.is_some());
}

#[test]
fn ide_caches_wait_for_android_studio_to_close() {
    let w = world();
    file(
        &w.home
            .join("Library/Caches/JetBrains/AndroidStudio2024/idx"),
        90_000,
    );
    file(
        &w.home.join("Library/Caches/JetBrains/GoLand2025.1/idx"),
        90_000,
    );
    file(
        &w.home.join("Library/Caches/Google/AndroidStudio2024/c"),
        40_000,
    );
    file(&w.home.join("Library/Caches/Google/Chrome/keep"), 40_000);
    let open = busy_machine("/Applications/Android Studio.app/Contents/MacOS/studio");

    let blocked = plan(&open, &env(&w, None));
    let closed = plan(&calm_machine(), &env(&w, None));

    assert!(
        category(&blocked, "ide-caches")
            .blocked
            .as_deref()
            .unwrap()
            .contains("Android Studio")
    );
    let labels: Vec<&str> = category(&closed, "ide-caches")
        .items
        .iter()
        .map(|i| i.label.as_str())
        .collect();
    assert_eq!(labels, ["AndroidStudio2024", "AndroidStudio2024"]);
    assert!(!labels.contains(&"Chrome"));
    assert!(
        !labels.contains(&"GoLand2025.1"),
        "another IDE's caches are not Android Studio's"
    );
}

#[test]
fn device_support_and_simulator_caches_are_careful_deletions() {
    let w = world();
    file(
        &w.home
            .join("Library/Developer/Xcode/iOS DeviceSupport/17.0 (21A)/Symbols/x"),
        30_000,
    );
    file(
        &w.home.join("Library/Developer/CoreSimulator/Caches/dyld/x"),
        30_000,
    );
    let e = env(&w, None);
    let p = plan(&calm_machine(), &e);

    assert_eq!(category(&p, "device-support").safety, Safety::Careful);
    assert_eq!(category(&p, "simulator-caches").safety, Safety::Careful);
    let default = selection(&p, &[], &[]);
    assert!(!default.contains(&"device-support") && !default.contains(&"simulator-caches"));

    let chosen = selection(&p, &["device-support".to_string()], &[]);
    let outcomes = execute(&p, &chosen, &calm_machine(), &e);

    assert!(
        outcomes
            .iter()
            .all(|o| o.category == "device-support" && o.result.is_ok())
    );
    assert!(
        w.home
            .join("Library/Developer/CoreSimulator/Caches/dyld/x")
            .exists()
    );
}

#[test]
fn stopping_daemons_signals_only_a_process_that_is_still_a_daemon() {
    let w = world();
    // A harmless child of ours stands in for the daemon.
    let mut child = Command::new("sleep").arg("60").spawn().unwrap();
    let pid = child.id();
    let daemon_args = "/usr/bin/java -cp x org.gradle.launcher.daemon.bootstrap.GradleDaemon 9.5.1";
    let sys = FakeSystem::new()
        .ok(
            PS_CMD,
            &ps_with_us(&format!("{pid} 1 400000 1.0 {daemon_args}\n")),
        )
        .ok(&format!("/bin/ps -ww -p {pid} -o args="), daemon_args);
    let e = env(&w, None);
    let p = plan(&sys, &e);
    let c = category(&p, "gradle-daemons");
    assert_eq!(
        c.action,
        Action::StopProcesses(vec![(pid, ProcessKind::GradleDaemon)])
    );
    assert_eq!(c.safety, Safety::Careful);

    let outcomes = execute(&p, &["gradle-daemons"], &sys, &e);

    assert!(outcomes[0].result.is_ok(), "{outcomes:?}");
    let status = child.wait().unwrap();
    assert!(!status.success(), "it was ended by the signal");
}

#[test]
fn a_pid_that_became_something_else_is_left_alone() {
    let w = world();
    let mut child = Command::new("sleep").arg("60").spawn().unwrap();
    let pid = child.id();
    let daemon_args = "/usr/bin/java -cp x org.gradle.launcher.daemon.bootstrap.GradleDaemon 9.5.1";
    let planned = FakeSystem::new().ok(
        PS_CMD,
        &ps_with_us(&format!("{pid} 1 400000 1.0 {daemon_args}\n")),
    );
    let e = env(&w, None);
    let p = plan(&planned, &e);
    let later = FakeSystem::new().ok(
        &format!("/bin/ps -ww -p {pid} -o args="),
        "/usr/bin/vim notes.txt",
    );

    let outcomes = execute(&p, &["gradle-daemons"], &later, &e);

    assert_eq!(outcomes[0].result, Err("no longer running".to_string()));
    assert!(
        child.try_wait().unwrap().is_none(),
        "the process must still be running"
    );
    child.kill().unwrap();
    child.wait().unwrap();
}

// --- choosing what to run ---

#[test]
fn the_default_choice_is_rebuildable_and_usable_only() {
    let w = world();
    file(
        &w.home.join("Library/Developer/Xcode/DerivedData/App/x"),
        1000,
    );
    file(
        &w.home
            .join("Library/Developer/Xcode/iOS DeviceSupport/17/x"),
        1000,
    );

    let p = plan(&calm_machine(), &env(&w, None));

    assert_eq!(selection(&p, &[], &[]), ["derived-data"]);
    assert_eq!(
        selection(&p, &["device-support".into()], &[]),
        ["derived-data", "device-support"]
    );
    assert_eq!(
        selection(&p, &["device-support".into()], &["device-support".into()]),
        ["device-support"]
    );
    assert!(selection(&p, &["derived-data".into()], &["nothing".into()]).is_empty());
}

#[test]
fn ids_that_are_not_categories_are_reported() {
    let given = ["derived-data".to_string(), "dervied-data".to_string()];
    let typos = unknown_ids(&given);

    assert_eq!(typos, ["dervied-data"]);
}

// --- the guard at work in a real run ---

#[test]
fn something_swapped_for_a_symlink_after_planning_is_not_followed() {
    let w = world();
    let victim = tempfile::TempDir::new().unwrap();
    file(&victim.path().join("precious.txt"), 100);
    let dd = w.home.join("Library/Developer/Xcode/DerivedData");
    file(&dd.join("App/x"), 1000);
    let e = env(&w, None);
    let p = plan(&calm_machine(), &e);
    // The disk changes between planning and deleting.
    fs::remove_dir_all(dd.join("App")).unwrap();
    std::os::unix::fs::symlink(victim.path(), dd.join("App")).unwrap();

    let outcomes = execute(&p, &["derived-data"], &calm_machine(), &e);

    assert!(
        outcomes[0]
            .result
            .as_ref()
            .unwrap_err()
            .contains("symbolic link")
    );
    assert!(victim.path().join("precious.txt").exists());
}

#[test]
fn a_forged_plan_pointing_outside_its_base_deletes_nothing() {
    let w = world();
    let outside = w.home.join("documents/thesis.txt");
    file(&outside, 100);
    let forged = Plan {
        categories: vec![lf_mide_tidy::Category {
            id: "derived-data",
            title: String::new(),
            why: String::new(),
            safety: Safety::Rebuildable,
            action: Action::Delete,
            items: vec![lf_mide_tidy::Item {
                label: "thesis".into(),
                path: Some(outside.clone()),
                base: Some(w.home.join("Library/Developer/Xcode/DerivedData")),
                bytes: 1,
            }],
            advice: Vec::new(),
            blocked: None,
        }],
    };
    fs::create_dir_all(w.home.join("Library/Developer/Xcode/DerivedData")).unwrap();

    let outcomes = execute(&forged, &["derived-data"], &calm_machine(), &env(&w, None));

    assert!(outcomes[0].result.is_err());
    assert!(outside.exists());
}
