//! Android SDK, emulator tooling and JDK.

use std::path::{Component, Path, PathBuf};

use lf_mide_core::{Category, CheckResult, Status};

use super::{CheckFn, Ctx, ToolSpec, tool};
use crate::props;
use crate::version::Version;

const MIN_JDK: Version = Version::new(17, 0, 0);
const STUDIO_APP: &str = "Android Studio.app";
const SDK_HINT: &str = "brew install --cask android-commandlinetools, then export ANDROID_HOME=$HOME/Library/Android/sdk";

pub const CHECKS: &[CheckFn] = &[
    android_studio,
    sdk,
    adb,
    emulator,
    sdkmanager,
    build_tools,
    platforms,
    system_images,
    avds,
    jdk,
    gradle_wrapper,
];

pub use lf_mide_platform::android::Sdk;

/// The SDK we may run tools from. Shared with the rest of LF-Mide: see
/// [`lf_mide_platform::android::find_sdk`] for the order and the trust rules.
pub fn find_sdk(ctx: &Ctx) -> Option<Sdk> {
    lf_mide_platform::android::find_sdk(ctx.sys)
}

/// The `sdk.dir` in the project's `local.properties`, offered to the user as
/// a suggestion only. Skips anything a repo could have planted: relative
/// paths, `..` components, paths inside the project, or directories that
/// don't exist.
fn suggested_sdk(ctx: &Ctx) -> Option<PathBuf> {
    let project = ctx.project?;
    let text = ctx
        .sys
        .read_to_string(&project.root.join("local.properties"))?;
    let dir = PathBuf::from(props::get(&text, "sdk.dir")?);
    let plain = dir.is_absolute() && !dir.components().any(|c| matches!(c, Component::ParentDir));
    if !plain {
        return None;
    }
    // Compare resolved paths: on macOS `/var` is a symlink to `/private/var`,
    // so a planted path can look "outside" the project as plain text.
    let resolved = ctx.sys.canonicalize(&dir)?;
    let root = ctx.sys.canonicalize(&project.root)?;
    (ctx.sys.is_dir(&resolved) && !resolved.starts_with(&root)).then_some(dir)
}

/// `gradle-9.5.1-bin.zip` URL → `9.5.1`.
pub fn parse_wrapper_version(distribution_url: &str) -> Option<String> {
    let file = distribution_url.rsplit('/').next()?;
    let version = file.strip_prefix("gradle-")?.split('-').next()?;
    Version::find_in(version).map(|_| version.to_string())
}

fn android_tool(
    ctx: &Ctx,
    name: &str,
    binary: &str,
    args: &[&str],
    required: bool,
    hint: &str,
    at: Option<PathBuf>,
) -> Vec<CheckResult> {
    let spec = ToolSpec {
        category: Category::Android,
        name,
        binary,
        version_args: args,
        min: None,
        required,
        hint,
    };
    vec![tool(ctx, &spec, at)]
}

fn android_studio(ctx: &Ctx) -> Vec<CheckResult> {
    const NAME: &str = "Android Studio";
    let locations = [
        Some(PathBuf::from("/Applications")),
        ctx.sys.home().map(|h| h.join("Applications")),
    ];
    let app = locations
        .into_iter()
        .flatten()
        .map(|dir| dir.join(STUDIO_APP))
        .find(|app| ctx.sys.is_dir(app));
    let Some(app) = app else {
        return vec![CheckResult::new(
            Category::Android,
            NAME,
            Status::Ok,
            "not installed (optional)",
        )];
    };
    let plist = format!("{}/Contents/Info", app.display());
    let version = ctx
        .sys
        .run("defaults", &["read", &plist, "CFBundleShortVersionString"])
        .ok()
        .filter(|output| output.success)
        .map(|output| output.stdout.trim().to_string())
        .filter(|version| !version.is_empty());
    let detail = match version {
        Some(version) => format!("{version} (optional: builds don't need it)"),
        None => "installed (optional)".to_string(),
    };
    vec![
        CheckResult::new(Category::Android, NAME, Status::Ok, detail)
            .with_path(app.display().to_string()),
    ]
}

fn sdk(ctx: &Ctx) -> Vec<CheckResult> {
    const NAME: &str = "Android SDK";
    let result = match find_sdk(ctx) {
        Some(sdk) => CheckResult::new(Category::Android, NAME, Status::Ok, sdk.source)
            .with_path(sdk.root.display().to_string()),
        None => match suggested_sdk(ctx) {
            Some(dir) => CheckResult::new(
                Category::Android,
                NAME,
                Status::Missing,
                format!("not found (local.properties points to {})", dir.display()),
            )
            .with_hint(format!(
                "If you trust that project: export ANDROID_HOME={}. Otherwise: {SDK_HINT}",
                dir.display()
            )),
            None => CheckResult::new(Category::Android, NAME, Status::Missing, "not found")
                .with_hint(SDK_HINT),
        },
    };
    vec![result]
}

fn sdk_tool_path(ctx: &Ctx, relative: &str) -> Option<PathBuf> {
    find_sdk(ctx).map(|sdk| sdk.root.join(relative))
}

fn adb(ctx: &Ctx) -> Vec<CheckResult> {
    let at = sdk_tool_path(ctx, "platform-tools/adb");
    android_tool(
        ctx,
        "adb",
        "adb",
        &["version"],
        true,
        "sdkmanager \"platform-tools\"",
        at,
    )
}

fn emulator(ctx: &Ctx) -> Vec<CheckResult> {
    let at = sdk_tool_path(ctx, "emulator/emulator");
    android_tool(
        ctx,
        "Android Emulator",
        "emulator",
        &["-version"],
        true,
        "sdkmanager \"emulator\"",
        at,
    )
}

/// `latest` if present, else the highest-numbered cmdline-tools directory.
fn pick_cmdline_tools(names: &[String]) -> Option<&String> {
    names
        .iter()
        .find(|name| *name == "latest")
        .or_else(|| newest_version(names))
}

fn sdkmanager(ctx: &Ctx) -> Vec<CheckResult> {
    let at = find_sdk(ctx).and_then(|sdk| {
        let dir = sdk.root.join("cmdline-tools");
        let names = ctx.sys.list_dir(&dir);
        let chosen = pick_cmdline_tools(&names)?;
        Some(dir.join(chosen).join("bin/sdkmanager"))
    });
    android_tool(
        ctx,
        "sdkmanager",
        "sdkmanager",
        &["--version"],
        false,
        "brew install --cask android-commandlinetools",
        at,
    )
}

fn newest_version(names: &[String]) -> Option<&String> {
    names
        .iter()
        .filter_map(|name| Some((Version::find_in(name)?, name)))
        .max_by_key(|(version, _)| *version)
        .map(|(_, name)| name)
}

fn build_tools(ctx: &Ctx) -> Vec<CheckResult> {
    const NAME: &str = "Build tools";
    let Some(sdk) = find_sdk(ctx) else {
        return Vec::new();
    };
    let dir = sdk.root.join("build-tools");
    let names = ctx.sys.list_dir(&dir);
    let result = match newest_version(&names) {
        Some(newest) => CheckResult::new(Category::Android, NAME, Status::Ok, newest.as_str())
            .with_path(dir.display().to_string()),
        None => CheckResult::new(Category::Android, NAME, Status::Missing, "none installed")
            .with_hint("sdkmanager \"build-tools;36.0.0\""),
    };
    vec![result]
}

fn platforms(ctx: &Ctx) -> Vec<CheckResult> {
    const NAME: &str = "Platforms";
    let Some(sdk) = find_sdk(ctx) else {
        return Vec::new();
    };
    let dir = sdk.root.join("platforms");
    let apis: Vec<String> = ctx
        .sys
        .list_dir(&dir)
        .into_iter()
        .filter(|name| name.starts_with("android-"))
        .collect();
    let result = if apis.is_empty() {
        CheckResult::new(Category::Android, NAME, Status::Missing, "none installed")
            .with_hint("sdkmanager \"platforms;android-36\"")
    } else {
        CheckResult::new(Category::Android, NAME, Status::Ok, apis.join(", "))
            .with_path(dir.display().to_string())
    };
    vec![result]
}

fn system_images(ctx: &Ctx) -> Vec<CheckResult> {
    const NAME: &str = "Emulator system images";
    let Some(sdk) = find_sdk(ctx) else {
        return Vec::new();
    };
    let root = sdk.root.join("system-images");
    let list = |dir: &Path| ctx.sys.list_dir(dir);
    let images: Vec<String> = list(&root)
        .into_iter()
        .flat_map(|api| {
            let api_dir = root.join(&api);
            list(&api_dir).into_iter().flat_map(move |variant| {
                let variant_dir = api_dir.join(&variant);
                list(&variant_dir)
                    .into_iter()
                    .map(|abi| format!("{api} {variant} {abi}"))
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    let result = match images.first() {
        Some(first) => CheckResult::new(
            Category::Android,
            NAME,
            Status::Ok,
            format!("{} installed ({first})", images.len()),
        ),
        None => CheckResult::new(Category::Android, NAME, Status::Warn, "none installed")
            .with_hint("sdkmanager \"system-images;android-36;google_apis;arm64-v8a\""),
    };
    vec![result]
}

fn avds(ctx: &Ctx) -> Vec<CheckResult> {
    const NAME: &str = "Android virtual devices";
    let Some(sdk) = find_sdk(ctx) else {
        return Vec::new();
    };
    let emulator = sdk.root.join("emulator/emulator");
    if !ctx.sys.exists(&emulator) {
        return Vec::new();
    }
    let output = match ctx
        .sys
        .run(&emulator.display().to_string(), &["-list-avds"])
    {
        Ok(output) if output.success => output,
        _ => {
            return vec![CheckResult::new(
                Category::Android,
                NAME,
                Status::Warn,
                "could not list AVDs",
            )];
        }
    };
    // The emulator prints log lines (with spaces) around the AVD names.
    let names: Vec<&str> = output
        .stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.contains(char::is_whitespace))
        .collect();
    let result = if names.is_empty() {
        CheckResult::new(Category::Android, NAME, Status::Warn, "none created").with_hint(
            "avdmanager create avd -n Pixel_8 -k \"system-images;android-36;google_apis;arm64-v8a\"",
        )
    } else {
        CheckResult::new(
            Category::Android,
            NAME,
            Status::Ok,
            format!("{}: {}", names.len(), names.join(", ")),
        )
    };
    vec![result]
}

fn jdk(ctx: &Ctx) -> Vec<CheckResult> {
    let at = ctx
        .sys
        .env("JAVA_HOME")
        .map(|home| PathBuf::from(home).join("bin/java"));
    let spec = ToolSpec {
        category: Category::Android,
        name: "JDK",
        binary: "java",
        version_args: &["-version"],
        min: Some(MIN_JDK),
        required: true,
        hint: "brew install --cask zulu@21",
    };
    vec![tool(ctx, &spec, at)]
}

fn gradle_wrapper(ctx: &Ctx) -> Vec<CheckResult> {
    const NAME: &str = "Gradle wrapper";
    let Some(project) = ctx.project else {
        return Vec::new();
    };
    let script = project.root.join("gradlew");
    if !ctx.sys.exists(&script) {
        return vec![
            CheckResult::new(
                Category::Android,
                NAME,
                Status::Missing,
                "gradlew not found",
            )
            .with_hint("gradle wrapper"),
        ];
    }
    let properties = project
        .root
        .join("gradle/wrapper/gradle-wrapper.properties");
    let version = ctx
        .sys
        .read_to_string(&properties)
        .and_then(|text| props::get(&text, "distributionUrl"))
        .and_then(|url| parse_wrapper_version(&url));
    let detail = version.map_or_else(|| "found".to_string(), |v| format!("Gradle {v}"));
    vec![
        CheckResult::new(Category::Android, NAME, Status::Ok, detail)
            .with_path(script.display().to_string()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeSystem;
    use lf_mide_core::{Project, ProjectLayout};

    fn project(root: &str) -> Project {
        Project {
            root: PathBuf::from(root),
            name: "demo".into(),
            layout: ProjectLayout::AndroidAppShared,
        }
    }

    fn run(check: CheckFn, sys: &FakeSystem) -> Vec<CheckResult> {
        check(&Ctx { sys, project: None })
    }

    fn run_in(check: CheckFn, sys: &FakeSystem, root: &str) -> Vec<CheckResult> {
        let project = project(root);
        check(&Ctx {
            sys,
            project: Some(&project),
        })
    }

    fn one(mut results: Vec<CheckResult>) -> CheckResult {
        assert_eq!(results.len(), 1, "{results:?}");
        results.remove(0)
    }

    fn with_sdk() -> FakeSystem {
        FakeSystem::new()
            .env("ANDROID_HOME", "/sdk")
            .dir("/sdk", &[])
    }

    // --- locating the SDK -------------------------------------------------

    #[test]
    fn finds_sdk_from_android_home() {
        let sys = with_sdk();

        let sdk = find_sdk(&Ctx {
            sys: &sys,
            project: None,
        })
        .unwrap();

        assert_eq!(
            (sdk.root, sdk.source),
            (PathBuf::from("/sdk"), "ANDROID_HOME")
        );
    }

    #[test]
    fn skips_env_vars_pointing_nowhere_and_uses_sdk_root_next() {
        let sys = FakeSystem::new()
            .env("ANDROID_HOME", "/gone")
            .env("ANDROID_SDK_ROOT", "/sdk2")
            .dir("/sdk2", &[]);

        let sdk = find_sdk(&Ctx {
            sys: &sys,
            project: None,
        })
        .unwrap();

        assert_eq!(
            (sdk.root, sdk.source),
            (PathBuf::from("/sdk2"), "ANDROID_SDK_ROOT")
        );
    }

    #[test]
    fn local_properties_never_selects_the_sdk_we_run_tools_from() {
        // Even a plausible outside-the-repo path: the file is repo-controlled.
        let sys = FakeSystem::new()
            .file("/p/local.properties", "sdk.dir=/sdk3\n")
            .dir("/sdk3", &[]);
        let project = project("/p");

        assert!(
            find_sdk(&Ctx {
                sys: &sys,
                project: Some(&project)
            })
            .is_none()
        );
    }

    #[test]
    fn suggestions_see_through_symlinks_into_the_project() {
        // On macOS /var -> /private/var, so a planted path can look "outside"
        // the project as text while resolving to a directory inside it.
        let sys = FakeSystem::new()
            .file("/p/local.properties", "sdk.dir=/link/evil\n")
            .symlink("/link", "/p")
            .dir("/link/evil", &[])
            .dir("/p/evil", &[]);
        let project = project("/p");

        let r = one(sdk(&Ctx {
            sys: &sys,
            project: Some(&project),
        }));

        assert!(!r.detail.contains("local.properties"), "{}", r.detail);
    }

    #[test]
    fn missing_sdk_suggests_the_local_properties_dir_without_using_it() {
        let sys = FakeSystem::new()
            .file("/p/local.properties", "sdk.dir=/sdk3\n")
            .dir("/sdk3", &[]);
        let project = project("/p");

        let r = one(sdk(&Ctx {
            sys: &sys,
            project: Some(&project),
        }));

        assert_eq!(r.status, Status::Missing);
        assert!(
            r.detail.contains("local.properties points to /sdk3"),
            "{}",
            r.detail
        );
        assert!(r.hint.unwrap().contains("export ANDROID_HOME=/sdk3"));
    }

    #[test]
    fn suggestions_skip_paths_a_repo_could_have_planted() {
        for dir in [
            "/p/evil-sdk",
            "relative/sdk",
            "/Users/x/other/../p/evil",
            "/elsewhere/missing",
        ] {
            let sys = FakeSystem::new()
                .file("/p/local.properties", &format!("sdk.dir={dir}\n"))
                .dir("/p/evil-sdk", &[])
                .dir("/Users/x/other/../p/evil", &[])
                .dir("relative/sdk", &[]);
            let project = project("/p");

            let r = one(sdk(&Ctx {
                sys: &sys,
                project: Some(&project),
            }));

            assert!(
                !r.detail.contains("local.properties"),
                "{dir}: {}",
                r.detail
            );
        }
    }

    #[test]
    fn ignores_local_properties_sdk_inside_the_project() {
        // A cloned repo could ship its own "SDK" containing a fake adb that
        // the doctor would then execute.
        let sys = FakeSystem::new()
            .file("/p/local.properties", "sdk.dir=/p/evil-sdk\n")
            .dir("/p/evil-sdk", &[])
            .file("/p/evil-sdk/platform-tools/adb", "");
        let project = project("/p");

        assert!(
            find_sdk(&Ctx {
                sys: &sys,
                project: Some(&project)
            })
            .is_none()
        );
    }

    #[test]
    fn ignores_relative_local_properties_sdk() {
        let sys = FakeSystem::new()
            .file("/p/local.properties", "sdk.dir=sdk\n")
            .dir("sdk", &[]);
        let project = project("/p");

        assert!(
            find_sdk(&Ctx {
                sys: &sys,
                project: Some(&project)
            })
            .is_none()
        );
    }

    #[test]
    fn project_sdk_inside_the_repo_does_not_shadow_a_real_one() {
        let sys = FakeSystem::new()
            .file("/p/local.properties", "sdk.dir=/p/evil-sdk\n")
            .dir("/p/evil-sdk", &[])
            .home("/Users/a")
            .dir("/Users/a/Library/Android/sdk", &[]);
        let project = project("/p");

        let sdk = find_sdk(&Ctx {
            sys: &sys,
            project: Some(&project),
        })
        .unwrap();

        assert_eq!(sdk.source, "default location");
    }

    #[test]
    fn falls_back_to_default_location_then_none() {
        let sys = FakeSystem::new()
            .home("/Users/a")
            .dir("/Users/a/Library/Android/sdk", &[]);
        let found = find_sdk(&Ctx {
            sys: &sys,
            project: None,
        })
        .unwrap();

        assert_eq!(found.source, "default location");
        assert!(
            find_sdk(&Ctx {
                sys: &FakeSystem::new(),
                project: None
            })
            .is_none()
        );
    }

    #[test]
    fn sdk_check_reports_location_or_install_hint() {
        let found = one(run(sdk, &with_sdk()));
        let missing = one(run(sdk, &FakeSystem::new()));

        assert_eq!(
            (found.status, found.detail.as_str(), found.path.as_deref()),
            (Status::Ok, "ANDROID_HOME", Some("/sdk"))
        );
        assert_eq!(missing.status, Status::Missing);
        assert!(missing.hint.unwrap().contains("android-commandlinetools"));
    }

    // --- tools inside the SDK ---------------------------------------------

    #[test]
    fn adb_comes_from_platform_tools() {
        let sys = with_sdk().file("/sdk/platform-tools/adb", "").ok(
            "/sdk/platform-tools/adb version",
            "Android Debug Bridge version 1.0.41\nVersion 36.0.0-13206524",
        );

        let r = one(run(adb, &sys));

        assert_eq!(
            (r.name.as_str(), r.status, r.detail.as_str()),
            ("adb", Status::Ok, "1.0.41")
        );
        assert_eq!(r.path.as_deref(), Some("/sdk/platform-tools/adb"));
    }

    #[test]
    fn missing_adb_gets_a_sdkmanager_hint() {
        let r = one(run(adb, &with_sdk().env("PATH", "/bin")));

        assert_eq!(r.status, Status::Missing);
        assert_eq!(r.hint.as_deref(), Some("sdkmanager \"platform-tools\""));
    }

    #[test]
    fn emulator_version_is_read_from_sdk() {
        let sys = with_sdk().file("/sdk/emulator/emulator", "").ok(
            "/sdk/emulator/emulator -version",
            "Android emulator version 36.1.9.0 (build_id 13823996)",
        );

        let r = one(run(emulator, &sys));

        assert_eq!((r.status, r.detail.as_str()), (Status::Ok, "36.1.9"));
    }

    #[test]
    fn sdkmanager_prefers_latest_cmdline_tools() {
        let sys = with_sdk()
            .dir("/sdk/cmdline-tools", &["16.0", "latest"])
            .file("/sdk/cmdline-tools/latest/bin/sdkmanager", "")
            .ok(
                "/sdk/cmdline-tools/latest/bin/sdkmanager --version",
                "12.0\n",
            );

        let r = one(run(sdkmanager, &sys));

        assert_eq!((r.status, r.detail.as_str()), (Status::Ok, "12.0.0"));
    }

    #[test]
    fn sdkmanager_falls_back_to_highest_versioned_dir() {
        let sys = with_sdk()
            .dir("/sdk/cmdline-tools", &["9.0", "16.0"])
            .file("/sdk/cmdline-tools/16.0/bin/sdkmanager", "")
            .ok("/sdk/cmdline-tools/16.0/bin/sdkmanager --version", "16.0");

        assert_eq!(one(run(sdkmanager, &sys)).detail, "16.0.0");
    }

    #[test]
    fn missing_cmdline_tools_is_a_warning_with_install_hint() {
        let r = one(run(sdkmanager, &with_sdk().env("PATH", "/bin")));

        assert_eq!(r.status, Status::Warn);
        assert!(r.hint.unwrap().contains("android-commandlinetools"));
    }

    // --- SDK packages -----------------------------------------------------

    #[test]
    fn build_tools_reports_the_newest_version() {
        let sys = with_sdk().dir("/sdk/build-tools", &["35.0.0", "36.0.0", "35.0.1"]);

        let r = one(run(build_tools, &sys));

        assert_eq!((r.status, r.detail.as_str()), (Status::Ok, "36.0.0"));
    }

    #[test]
    fn package_checks_are_skipped_without_an_sdk_and_missing_with_empty_sdk() {
        assert!(run(build_tools, &FakeSystem::new()).is_empty());
        assert!(run(platforms, &FakeSystem::new()).is_empty());
        assert!(run(system_images, &FakeSystem::new()).is_empty());

        assert_eq!(one(run(build_tools, &with_sdk())).status, Status::Missing);
        assert_eq!(one(run(platforms, &with_sdk())).status, Status::Missing);
        assert_eq!(one(run(system_images, &with_sdk())).status, Status::Warn);
    }

    #[test]
    fn platforms_lists_android_api_levels() {
        let sys = with_sdk().dir("/sdk/platforms", &["android-35", "android-36", ".DS_Store"]);

        let r = one(run(platforms, &sys));

        assert_eq!(
            (r.status, r.detail.as_str()),
            (Status::Ok, "android-35, android-36")
        );
    }

    #[test]
    fn system_images_are_counted_across_api_variant_and_abi() {
        let sys = with_sdk()
            .dir("/sdk/system-images", &["android-36"])
            .dir("/sdk/system-images/android-36", &["google_apis_playstore"])
            .dir(
                "/sdk/system-images/android-36/google_apis_playstore",
                &["arm64-v8a"],
            );

        let r = one(run(system_images, &sys));

        assert_eq!(r.status, Status::Ok);
        assert_eq!(
            r.detail,
            "1 installed (android-36 google_apis_playstore arm64-v8a)"
        );
    }

    #[test]
    fn avds_are_listed_and_log_noise_is_ignored() {
        let sys = with_sdk().file("/sdk/emulator/emulator", "").ok(
            "/sdk/emulator/emulator -list-avds",
            "INFO    | Storing crashdata\nMedium_Phone_API_36\nPixel_8\n",
        );

        let r = one(run(avds, &sys));

        assert_eq!(
            (r.status, r.detail.as_str()),
            (Status::Ok, "2: Medium_Phone_API_36, Pixel_8")
        );
    }

    #[test]
    fn no_avds_warns_with_create_hint_and_no_emulator_skips() {
        let none = with_sdk()
            .file("/sdk/emulator/emulator", "")
            .ok("/sdk/emulator/emulator -list-avds", "");

        let r = one(run(avds, &none));

        assert_eq!(r.status, Status::Warn);
        assert!(r.hint.unwrap().contains("avdmanager create avd"));
        assert!(run(avds, &with_sdk().env("PATH", "/bin")).is_empty());
    }

    // --- JDK and Gradle ---------------------------------------------------

    #[test]
    fn jdk_comes_from_java_home_and_reads_stderr_banner() {
        let sys = FakeSystem::new()
            .env("JAVA_HOME", "/jdk")
            .file("/jdk/bin/java", "")
            .ok_stderr(
                "/jdk/bin/java -version",
                "openjdk version \"21.0.12\" 2026-01-20\nOpenJDK Runtime Environment Zulu21",
            );

        let r = one(run(jdk, &sys));

        assert_eq!((r.status, r.detail.as_str()), (Status::Ok, "21.0.12"));
    }

    #[test]
    fn jdk_8_is_too_old() {
        let sys = FakeSystem::new()
            .env("PATH", "/bin")
            .file("/bin/java", "")
            .ok_stderr("/bin/java -version", "java version \"1.8.0_292\"");

        let r = one(run(jdk, &sys));

        assert_eq!(r.status, Status::Warn);
        assert!(r.detail.contains("need ≥ 17.0.0"));
    }

    #[test]
    fn missing_jdk_hint_suggests_zulu() {
        let r = one(run(jdk, &FakeSystem::new().env("PATH", "/bin")));

        assert_eq!(r.status, Status::Missing);
        assert!(r.hint.unwrap().contains("zulu"));
    }

    #[test]
    fn parses_gradle_version_from_distribution_url() {
        assert_eq!(
            parse_wrapper_version("https://services.gradle.org/distributions/gradle-9.5.1-bin.zip"),
            Some("9.5.1".into())
        );
        assert_eq!(
            parse_wrapper_version("https://x/gradle-8.14-all.zip"),
            Some("8.14".into())
        );
        assert_eq!(parse_wrapper_version("https://x/other.zip"), None);
    }

    #[test]
    fn gradle_wrapper_is_read_from_the_project() {
        let sys = FakeSystem::new().file("/p/gradlew", "").file(
            "/p/gradle/wrapper/gradle-wrapper.properties",
            "distributionUrl=https\\://services.gradle.org/distributions/gradle-9.5.1-bin.zip\n",
        );

        let r = one(run_in(gradle_wrapper, &sys, "/p"));

        assert_eq!(
            (r.status, r.detail.as_str(), r.path.as_deref()),
            (Status::Ok, "Gradle 9.5.1", Some("/p/gradlew"))
        );
    }

    #[test]
    fn gradle_wrapper_missing_is_reported_and_skipped_outside_projects() {
        let r = one(run_in(gradle_wrapper, &FakeSystem::new(), "/p"));

        assert_eq!(r.status, Status::Missing);
        assert_eq!(r.hint.as_deref(), Some("gradle wrapper"));
        assert!(run(gradle_wrapper, &FakeSystem::new()).is_empty());
    }

    // --- Android Studio (optional) ---------------------------------------

    #[test]
    fn android_studio_is_informational_either_way() {
        let installed = FakeSystem::new()
            .dir("/Applications/Android Studio.app", &[])
            .ok("defaults read /Applications/Android Studio.app/Contents/Info CFBundleShortVersionString", "2025.3.1\n");

        let present = one(run(android_studio, &installed));
        let absent = one(run(android_studio, &FakeSystem::new()));

        assert_eq!(present.status, Status::Ok);
        assert!(present.detail.starts_with("2025.3.1"), "{}", present.detail);
        assert_eq!(
            (absent.status, absent.detail.as_str()),
            (Status::Ok, "not installed (optional)")
        );
        assert_eq!(present.category, Category::Android);
    }
}
