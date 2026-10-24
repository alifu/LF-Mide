//! One builder per thing that can be cleaned or reviewed.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use lf_mide_core::git::clean_text;
use lf_mide_core::props;
use lf_mide_core::{ProcessKind, ProcessStat};
use lf_mide_system::System;
use lf_mide_vcs::ProjectRepo;

use crate::clean::{Action, Category, CleanEnv, Item, Safety};
use crate::size::disk_use;

const WEEK_SECS: i64 = 7 * 24 * 60 * 60;
/// Gradle distributions kept besides the one the project uses.
const KEEP_NEWEST_DISTRIBUTIONS: usize = 2;
/// Simulators listed by size.
const BIGGEST_SIMULATORS: usize = 10;
const XCRUN: &str = "/usr/bin/xcrun";

/// Every category id there is, whether or not it has anything to clean now.
pub const IDS: [&str; 13] = [
    "derived-data",
    "project-build",
    "gradle-wrappers",
    "gradle-logs",
    "unavailable-simulators",
    "emulator-snapshots",
    "device-support",
    "simulator-caches",
    "ide-caches",
    "gradle-daemons",
    "simulator-runtimes",
    "simulators",
    "android-images",
];

/// Why `id` cannot be cleaned right now because of what is running, looked
/// at afresh. Only the categories that touch files an app is using can be
/// blocked this way.
pub fn blocked_now(sys: &dyn System, env: &CleanEnv, category: &Category) -> Option<String> {
    let (kind, what) = match category.id {
        "emulator-snapshots" => (
            ProcessKind::Emulator,
            "an emulator is running: stop it first",
        ),
        "ide-caches" => (
            ProcessKind::AndroidStudio,
            "Android Studio is running: quit it first",
        ),
        "gradle-wrappers" => {
            let procs = process_list(sys, env);
            return match &procs {
                None => Some("could not check which Gradle versions are in use".to_string()),
                Some(list) => category
                    .items
                    .iter()
                    .find(|item| in_use(list, &item.label))
                    .map(|item| format!("{} is used by a running Gradle daemon", item.label)),
            };
        }
        _ => return None,
    };
    let procs = process_list(sys, env);
    match running(procs.as_deref(), kind) {
        Some(false) => None,
        Some(true) => Some(what.to_string()),
        None => Some("could not check what is running".to_string()),
    }
}

/// What is running, or `None` when that cannot be told: the command failed,
/// or its answer does not even list this process (a sandbox, output that
/// did not parse), in which case an empty list would be a guess.
fn process_list(sys: &dyn System, env: &CleanEnv) -> Option<Vec<ProcessStat>> {
    let sample = lf_mide_perf::sample(sys, env.own_pid).ok()?;
    sample
        .processes
        .iter()
        .any(|p| p.kind == ProcessKind::LfMide)
        .then_some(sample.processes)
}

pub fn all(sys: &dyn System, env: &CleanEnv) -> Vec<Category> {
    let running = process_list(sys, env);
    [
        derived_data(env),
        project_builds(env),
        gradle_distributions(env, running.as_deref()),
        gradle_logs(env),
        unavailable_simulators(sys, env),
        emulator_snapshots(env, running.as_deref()),
        device_support(env),
        simulator_caches(env),
        ide_caches(env, running.as_deref()),
        gradle_daemons(running.as_deref()),
        simulator_runtimes(sys),
        simulators_by_size(sys, env),
        android_images(sys, env),
    ]
    .into_iter()
    .flatten()
    .map(sanitized)
    .collect()
}

/// Names that come from other programs or from the repository are shown
/// and suggested as commands: control characters and look-alike direction
/// marks are taken out so a name cannot start a new line of a command or
/// redraw the screen.
fn sanitized(mut category: Category) -> Category {
    for item in &mut category.items {
        item.label = clean_text(&item.label);
    }
    category.advice = category.advice.iter().map(|a| clean_text(a)).collect();
    category
}

fn category(
    id: &'static str,
    title: &str,
    why: &str,
    safety: Safety,
    action: Action,
    items: Vec<Item>,
) -> Category {
    Category {
        id,
        title: title.to_string(),
        why: why.to_string(),
        safety,
        action,
        items,
        advice: Vec::new(),
        blocked: None,
    }
}

/// Entries of a folder, by name. Unreadable folders have none.
fn entries(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(read) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<(String, PathBuf)> = read
        .flatten()
        .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
        .collect();
    found.sort();
    found
}

/// An item for something inside `base`, labelled relative to `shown_from`.
fn item(path: &Path, base: &Path, shown_from: &Path) -> Item {
    Item {
        label: path
            .strip_prefix(shown_from)
            .unwrap_or(path)
            .display()
            .to_string(),
        path: Some(path.to_path_buf()),
        base: Some(base.to_path_buf()),
        bytes: disk_use(path),
    }
}

/// A real folder (not a link).
fn is_real_dir(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.is_dir())
}

fn modified_secs(path: &Path) -> Option<i64> {
    let modified = fs::symlink_metadata(path).ok()?.modified().ok()?;
    let secs = modified.duration_since(UNIX_EPOCH).ok()?.as_secs();
    i64::try_from(secs).ok()
}

fn nonempty(category: Category) -> Option<Category> {
    (!category.items.is_empty() || category.blocked.is_some()).then_some(category)
}

// --- rebuildable -----------------------------------------------------------

/// Xcode's build products, and the ones LF-Mide's own iOS builds leave.
fn derived_data(env: &CleanEnv) -> Option<Category> {
    let bases = [
        env.home.join("Library/Developer/Xcode/DerivedData"),
        env.home.join("Library/Caches/lf-mide/derived"),
    ];
    let items = bases
        .iter()
        .flat_map(|base| {
            entries(base)
                .into_iter()
                .map(move |(_, path)| item(&path, base, &env.home))
        })
        .collect();
    nonempty(category(
        "derived-data",
        "Xcode build products (DerivedData)",
        "Rebuilt by the next iOS build; the first build after cleaning is slower.",
        Safety::Rebuildable,
        Action::Delete,
        items,
    ))
}

/// `build`, `.gradle` and `.kotlin` folders in the project, but only ones git
/// already ignores: a folder git tracks is somebody's work.
fn project_builds(env: &CleanEnv) -> Option<Category> {
    let root = env.project.as_ref()?;
    let mut entry = category(
        "project-build",
        "This project's build folders",
        "Gradle's build output and caches inside the project; the next build recreates them.",
        Safety::Rebuildable,
        Action::Delete,
        Vec::new(),
    );
    let Ok(Some(repo)) = ProjectRepo::open(root) else {
        entry.blocked = Some("the project is not in a git repository, so generated folders cannot be told from yours".to_string());
        return Some(entry);
    };
    // The repository must really be this project's: its settings file is
    // tracked, and it is not a repository of the whole home folder (a
    // dotfiles repository that ignores everything would make every folder
    // look disposable).
    let settings_tracked = ["settings.gradle.kts", "settings.gradle"]
        .iter()
        .any(|name| repo.tracks_file(Path::new(name)));
    if repo.workdir() == env.home.as_path() || !settings_tracked {
        entry.blocked = Some("git does not track this project's settings.gradle(.kts) in a repository of its own, so what is generated cannot be told from what is yours".to_string());
        return Some(entry);
    }
    // Candidates are taken from the folder listing, so the name is exactly
    // what is on disk: on a case-insensitive disk `root.join("build")` would
    // also find a folder called `Build`.
    let mut candidates: Vec<PathBuf> = entries(root)
        .into_iter()
        .filter(|(name, _)| matches!(name.as_str(), ".gradle" | ".kotlin" | "build"))
        .map(|(_, path)| path)
        .collect();
    for (name, path) in entries(root) {
        if name.starts_with('.') || !is_real_dir(&path) {
            continue;
        }
        candidates.extend(
            entries(&path)
                .into_iter()
                .filter(|(inner, _)| inner == "build")
                .map(|(_, build)| build),
        );
    }
    entry.items = candidates
        .into_iter()
        .filter(|path| is_real_dir(path))
        .filter(|path| {
            // Ignored by a pattern, nothing in it tracked, and not inside
            // some other repository (whose rules are not this one's).
            path.strip_prefix(root).is_ok_and(|relative| {
                repo.is_ignored(relative, true)
                    && !repo.tracks_anything_under(relative)
                    && !repo.inside_nested_repo(relative)
            })
        })
        .map(|path| item(&path, root, root))
        .collect();
    nonempty(entry)
}

/// Old Gradle distributions: all but the project's own and the newest few.
/// True when a running Gradle daemon was started from the distribution
/// folder called `name`.
fn in_use(procs: &[ProcessStat], name: &str) -> bool {
    procs
        .iter()
        .any(|p| p.kind == ProcessKind::GradleDaemon && p.detail == name)
}

fn gradle_distributions(env: &CleanEnv, procs: Option<&[ProcessStat]>) -> Option<Category> {
    let base = env.gradle_home.join("wrapper/dists");
    let used = env
        .project
        .as_ref()
        .and_then(|root| project_distribution(root));
    let mut dists: Vec<(PathBuf, i64)> = entries(&base)
        .into_iter()
        .filter(|(name, path)| name.starts_with("gradle-") && is_real_dir(path))
        .map(|(_, path)| {
            let modified = modified_secs(&path).unwrap_or(0);
            (path, modified)
        })
        .collect();
    dists.sort_by_key(|(_, modified)| std::cmp::Reverse(*modified));
    let items = dists
        .into_iter()
        .skip(KEEP_NEWEST_DISTRIBUTIONS)
        .filter(|(path, _)| path.file_name().map(|n| n.to_string_lossy().into_owned()) != used)
        .filter(|(path, _)| {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
            // A version a running daemon was started from is in use, whoever
            // downloaded it and whichever project asked for it.
            !procs.is_some_and(|list| name.as_deref().is_some_and(|n| in_use(list, n)))
        })
        .map(|(path, _)| item(&path, &base, &base))
        .collect();
    let mut entry = category(
        "gradle-wrappers",
        "Old Gradle versions",
        "Downloaded again if a project asks for that version. The newest two, this project's and any a running daemon uses are kept.",
        Safety::Rebuildable,
        Action::Delete,
        items,
    );
    if procs.is_none() && !entry.items.is_empty() {
        entry.blocked = Some("could not check which Gradle versions are in use".to_string());
    }
    nonempty(entry)
}

/// `gradle-9.5.1-bin` for a project whose wrapper uses that distribution.
fn project_distribution(root: &Path) -> Option<String> {
    let text = fs::read_to_string(root.join("gradle/wrapper/gradle-wrapper.properties")).ok()?;
    let url = props::get(&text, "distributionUrl")?;
    let file = url.rsplit('/').next()?;
    Some(file.strip_suffix(".zip")?.to_string())
}

/// Gradle daemon logs older than a week.
fn gradle_logs(env: &CleanEnv) -> Option<Category> {
    let base = env.gradle_home.join("daemon");
    let items = entries(&base)
        .into_iter()
        .filter(|(_, path)| is_real_dir(path))
        .flat_map(|(_, dir)| entries(&dir))
        .filter(|(name, path)| name.ends_with(".log") && !is_link(path))
        .filter(|(_, path)| modified_secs(path).is_some_and(|secs| env.now_secs - secs > WEEK_SECS))
        .map(|(_, path)| item(&path, &base, &base))
        .collect();
    nonempty(category(
        "gradle-logs",
        "Old Gradle daemon logs",
        "Logs of daemons that stopped more than a week ago.",
        Safety::Rebuildable,
        Action::Delete,
        items,
    ))
}

fn is_link(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
}

/// Simulators whose runtime was uninstalled; `simctl` removes them.
fn unavailable_simulators(sys: &dyn System, env: &CleanEnv) -> Option<Category> {
    let out = sys
        .run(XCRUN, &["simctl", "list", "devices", "unavailable", "-j"])
        .ok()
        .filter(|o| o.success)?;
    let json: serde_json::Value = serde_json::from_str(&out.stdout).ok()?;
    let devices_dir = env.home.join("Library/Developer/CoreSimulator/Devices");
    let items: Vec<Item> = json
        .get("devices")?
        .as_object()?
        .values()
        .flat_map(|list| list.as_array().into_iter().flatten())
        .filter_map(|device| {
            let udid = device.get("udid")?.as_str()?;
            let name = device.get("name")?.as_str()?;
            let dir = devices_dir.join(udid);
            Some(Item {
                label: name.to_string(),
                path: None,
                base: None,
                bytes: if valid_id(udid) { disk_use(&dir) } else { 0 },
            })
        })
        .collect();
    nonempty(category(
        "unavailable-simulators",
        "Simulators that can no longer run",
        "Their runtime is gone, so they cannot start and only take space (xcrun simctl delete unavailable). Anything inside them is lost.",
        Safety::Rebuildable,
        Action::Command {
            program: XCRUN.to_string(),
            args: ["simctl", "delete", "unavailable"]
                .map(String::from)
                .to_vec(),
        },
        items,
    ))
}

// --- careful ---------------------------------------------------------------

fn running(procs: Option<&[ProcessStat]>, kind: ProcessKind) -> Option<bool> {
    procs.map(|list| list.iter().any(|p| p.kind == kind))
}

/// Quick-boot snapshots of the Android virtual devices.
fn emulator_snapshots(env: &CleanEnv, procs: Option<&[ProcessStat]>) -> Option<Category> {
    let base = env.home.join(".android/avd");
    let items = entries(&base)
        .into_iter()
        .filter(|(name, path)| name.ends_with(".avd") && is_real_dir(path))
        .flat_map(|(_, avd)| entries(&avd.join("snapshots")))
        .map(|(_, path)| item(&path, &base, &base))
        .collect();
    let mut entry = category(
        "emulator-snapshots",
        "Android emulator snapshots",
        "Saved quick-boot states. The next start of that emulator is a full (slow) boot.",
        Safety::Careful,
        Action::Delete,
        items,
    );
    entry.blocked = match running(procs, ProcessKind::Emulator) {
        Some(false) => None,
        Some(true) => Some("an emulator is running: stop it first".to_string()),
        None => Some("could not check whether an emulator is running".to_string()),
    };
    nonempty(entry)
}

/// Symbols Xcode copied from physical iPhones and iPads.
fn device_support(env: &CleanEnv) -> Option<Category> {
    let base = env.home.join("Library/Developer/Xcode/iOS DeviceSupport");
    let items = entries(&base)
        .into_iter()
        .map(|(_, path)| item(&path, &base, &base))
        .collect();
    nonempty(category(
        "device-support",
        "Symbols from physical iOS devices",
        "Copied again the next time that device is connected; not used by the simulator.",
        Safety::Careful,
        Action::Delete,
        items,
    ))
}

fn simulator_caches(env: &CleanEnv) -> Option<Category> {
    let base = env.home.join("Library/Developer/CoreSimulator/Caches");
    let items = entries(&base)
        .into_iter()
        .map(|(_, path)| item(&path, &base, &base))
        .collect();
    nonempty(category(
        "simulator-caches",
        "Simulator caches",
        "Rebuilt the next time a simulator boots, which makes that first boot slow.",
        Safety::Careful,
        Action::Delete,
        items,
    ))
}

/// Android Studio's caches, if it is closed. Other JetBrains IDEs keep their
/// caches beside it; they are not Android Studio's to clean (one of them may
/// be open right now), so only folders named `AndroidStudio…` are offered.
fn ide_caches(env: &CleanEnv, procs: Option<&[ProcessStat]>) -> Option<Category> {
    let bases = [
        env.home.join("Library/Caches/Google"),
        env.home.join("Library/Caches/JetBrains"),
    ];
    let items = bases
        .iter()
        .flat_map(|base| {
            entries(base)
                .into_iter()
                .filter(|(name, path)| name.starts_with("AndroidStudio") && is_real_dir(path))
                .map(move |(_, path)| item(&path, base, base))
        })
        .collect();
    let mut entry = category(
        "ide-caches",
        "Android Studio caches",
        "Rebuilt (re-indexed) the next time Android Studio opens.",
        Safety::Careful,
        Action::Delete,
        items,
    );
    entry.blocked = match running(procs, ProcessKind::AndroidStudio) {
        Some(false) => None,
        Some(true) => Some("Android Studio is running: quit it first".to_string()),
        None => Some("could not check whether Android Studio is running".to_string()),
    };
    nonempty(entry)
}

/// Running Gradle and Kotlin daemons: memory, not disk.
fn gradle_daemons(procs: Option<&[ProcessStat]>) -> Option<Category> {
    let list = procs?;
    let daemons: Vec<&ProcessStat> = list
        .iter()
        .filter(|p| {
            matches!(
                p.kind,
                ProcessKind::GradleDaemon | ProcessKind::KotlinDaemon
            )
        })
        .collect();
    let targets = daemons.iter().map(|p| (p.pid, p.kind)).collect();
    let items = daemons
        .iter()
        .map(|p| Item {
            label: format!("{} (pid {})", p.kind.label(), p.pid),
            path: None,
            base: None,
            bytes: p.rss,
        })
        .collect();
    nonempty(category(
        "gradle-daemons",
        "Running Gradle and Kotlin daemons (memory)",
        "Frees memory now; the next build starts a daemon again. A build in progress would be cut short.",
        Safety::Careful,
        Action::StopProcesses(targets),
        items,
    ))
}

// --- to review yourself ----------------------------------------------------

/// Simulator runtimes: the biggest files on a typical iOS developer's disk.
fn simulator_runtimes(sys: &dyn System) -> Option<Category> {
    let runtimes = json_of(sys, &["simctl", "runtime", "list", "-j"])?;
    let devices = json_of(sys, &["simctl", "list", "devices", "-j"]);
    let device_count = |runtime_id: &str| -> usize {
        devices
            .as_ref()
            .and_then(|d| d.get("devices")?.get(runtime_id)?.as_array().map(Vec::len))
            .unwrap_or(0)
    };
    let mut rows: Vec<(u64, String, bool)> = runtimes
        .as_object()?
        .values()
        .filter_map(|runtime| {
            let id = runtime.get("identifier")?.as_str()?;
            let version = runtime.get("version")?.as_str()?;
            let build = runtime.get("build").and_then(|b| b.as_str()).unwrap_or("?");
            let platform = runtime
                .get("runtimeIdentifier")
                .and_then(|r| r.as_str())
                .unwrap_or("")
                .rsplit('.')
                .next()
                .unwrap_or("")
                .split('-')
                .next()
                .unwrap_or("");
            let bytes = runtime
                .get("sizeBytes")
                .and_then(|b| b.as_u64())
                .unwrap_or(0);
            let count = device_count(runtime.get("runtimeIdentifier")?.as_str()?);
            let deletable = runtime.get("deletable").and_then(|d| d.as_bool()) == Some(true);
            let used = if count == 0 {
                "no simulators use it".to_string()
            } else {
                format!("{count} simulators")
            };
            let label = format!("{platform} {version} ({build}) · {used}");
            let removable = count == 0 && deletable && valid_id(id);
            Some((bytes, format!("{label}\u{0}{id}"), removable))
        })
        .collect();
    rows.sort_by_key(|row| std::cmp::Reverse(row.0));
    let mut entry = category(
        "simulator-runtimes",
        "iOS, watchOS and visionOS simulator runtimes",
        "The largest files on most iOS developers' disks. Re-downloading one takes a while, so you choose.",
        Safety::Careful,
        Action::ReportOnly,
        Vec::new(),
    );
    for (bytes, packed, removable) in rows {
        let (label, id) = packed.split_once('\u{0}').unwrap_or((&packed, ""));
        entry.items.push(Item {
            label: label.to_string(),
            path: None,
            base: None,
            bytes,
        });
        if removable {
            entry
                .advice
                .push(format!("xcrun simctl runtime delete {id}    # {label}"));
        }
    }
    nonempty(entry)
}

fn valid_id(id: &str) -> bool {
    id.len() == 36 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

fn json_of(sys: &dyn System, args: &[&str]) -> Option<serde_json::Value> {
    let out = sys.run(XCRUN, args).ok().filter(|o| o.success)?;
    serde_json::from_str(&out.stdout).ok()
}

/// The simulators with the most data.
fn simulators_by_size(sys: &dyn System, env: &CleanEnv) -> Option<Category> {
    let listing = json_of(sys, &["simctl", "list", "devices", "-j"])?;
    let dir = env.home.join("Library/Developer/CoreSimulator/Devices");
    let mut rows: Vec<(u64, String, Option<String>)> = Vec::new();
    for (runtime, list) in listing.get("devices")?.as_object()? {
        let short = runtime.rsplit('.').next().unwrap_or(runtime);
        for device in list.as_array().into_iter().flatten() {
            let (Some(udid), Some(name)) = (
                device.get("udid").and_then(|u| u.as_str()),
                device.get("name").and_then(|n| n.as_str()),
            ) else {
                continue;
            };
            if !valid_id(udid) {
                continue;
            }
            let state = device.get("state").and_then(|s| s.as_str()).unwrap_or("?");
            rows.push((
                disk_use(&dir.join(udid)),
                format!("{name} · {short} · {state}"),
                (state == "Shutdown").then(|| udid.to_string()),
            ));
        }
    }
    rows.sort_by_key(|row| std::cmp::Reverse(row.0));
    let mut entry = category(
        "simulators",
        "Simulators with the most data",
        "Each one keeps its own apps and files. Deleting one removes it for good.",
        Safety::Careful,
        Action::ReportOnly,
        Vec::new(),
    );
    for (bytes, label, udid) in rows.into_iter().take(BIGGEST_SIMULATORS) {
        entry.items.push(Item {
            label: label.clone(),
            path: None,
            base: None,
            bytes,
        });
        // Only a simulator that is shut down is offered: deleting a running
        // one pulls it out from under whatever is using it.
        if let Some(udid) = udid {
            entry
                .advice
                .push(format!("xcrun simctl delete {udid}    # {label}"));
        }
    }
    entry.advice.push(
        "xcrun simctl erase all    # keeps every simulator, empties its apps and data".to_string(),
    );
    nonempty(entry)
}

/// Android virtual devices and downloaded system images.
fn android_images(sys: &dyn System, env: &CleanEnv) -> Option<Category> {
    let mut entry = category(
        "android-images",
        "Android virtual devices and system images",
        "Large, and not rebuilt for you: a virtual device holds its apps and data.",
        Safety::Careful,
        Action::ReportOnly,
        Vec::new(),
    );
    let avd_dir = env.home.join(".android/avd");
    for (name, path) in entries(&avd_dir) {
        let Some(avd) = name.strip_suffix(".avd") else {
            continue;
        };
        if !is_real_dir(&path)
            || !avd
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        {
            continue;
        }
        entry.items.push(Item {
            label: format!("virtual device {avd}"),
            path: None,
            base: None,
            bytes: disk_use(&path),
        });
        entry.advice.push(format!("avdmanager delete avd -n {avd}"));
    }
    if let Some(sdk) = lf_mide_platform::android::find_sdk(sys) {
        let images = sdk.root.join("system-images");
        for (api, api_path) in entries(&images) {
            for (tag, tag_path) in entries(&api_path) {
                for (abi, abi_path) in entries(&tag_path) {
                    let spec = format!("system-images;{api};{tag};{abi}");
                    if !is_real_dir(&abi_path)
                        || !spec.chars().all(|c| {
                            c.is_ascii_alphanumeric() || matches!(c, ';' | '-' | '_' | '.')
                        })
                    {
                        continue;
                    }
                    entry.items.push(Item {
                        label: format!("system image {api} {tag} {abi}"),
                        path: None,
                        base: None,
                        bytes: disk_use(&abi_path),
                    });
                    entry
                        .advice
                        .push(format!("sdkmanager --uninstall \"{spec}\""));
                }
            }
        }
    }
    nonempty(entry)
}
