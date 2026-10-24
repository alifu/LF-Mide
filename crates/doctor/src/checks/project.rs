//! Checks specific to the project being opened.

use lf_mide_core::{Category, CheckResult, Status};

use super::resources::{format_gb, ram_bytes};
use super::{CheckFn, Ctx};
use crate::props;

const MB: u64 = 1024 * 1024;
/// JVM heaps above this share of RAM leave too little for the emulator,
/// simulator and Xcode. Measured in Phase 0: 7 GB of heaps on a 16 GB Mac.
const MAX_HEAP_SHARE_PERCENT: u64 = 40;

pub const CHECKS: &[CheckFn] = &[layout, gradle_memory, build_server];

/// `-Xmx` from a JVM argument string, in megabytes (`-Xmx3g` → 3072).
pub fn parse_xmx_mb(jvm_args: &str) -> Option<u64> {
    let value = jvm_args
        .split_whitespace()
        .find_map(|arg| arg.strip_prefix("-Xmx"))?;
    let digits: String = value.chars().take_while(char::is_ascii_digit).collect();
    let amount: u64 = digits.parse().ok()?;
    match value[digits.len()..].to_ascii_lowercase().as_str() {
        "g" => Some(amount.saturating_mul(1024)),
        "m" => Some(amount),
        "k" => Some(amount / 1024),
        "" => Some(amount / MB),
        _ => None,
    }
}

fn layout(ctx: &Ctx) -> Vec<CheckResult> {
    const NAME: &str = "CMP project";
    let result = match ctx.project {
        Some(project) => CheckResult::new(
            Category::Project,
            NAME,
            Status::Ok,
            format!("{} + iosApp", project.layout.label()),
        )
        .with_path(project.root.display().to_string()),
        None => CheckResult::new(
            Category::Project,
            NAME,
            Status::Warn,
            "not inside a CMP project",
        )
        .with_hint("Run from the project directory or pass its path: lf-mide doctor <path>"),
    };
    vec![result]
}

fn gradle_memory(ctx: &Ctx) -> Vec<CheckResult> {
    const NAME: &str = "Gradle & Kotlin daemon memory";
    let Some(project) = ctx.project else {
        return Vec::new();
    };
    let text = ctx
        .sys
        .read_to_string(&project.root.join("gradle.properties"))
        .unwrap_or_default();
    // The user's own ~/.gradle/gradle.properties applies to every project;
    // the project's file wins where both set a key (as in Gradle).
    let user_text = ctx
        .sys
        .home()
        .and_then(|home| {
            ctx.sys
                .read_to_string(&home.join(".gradle/gradle.properties"))
        })
        .unwrap_or_default();
    let heap = |key| {
        props::get(&text, key)
            .or_else(|| props::get(&user_text, key))
            .and_then(|args| parse_xmx_mb(&args))
    };
    let sizes = [
        ("Gradle", heap("org.gradle.jvmargs")),
        ("Kotlin", heap("kotlin.daemon.jvmargs")),
    ];
    if sizes.iter().all(|(_, mb)| mb.is_none()) {
        return vec![CheckResult::new(
            Category::Project,
            NAME,
            Status::Ok,
            "no -Xmx set (JVM defaults)",
        )];
    }

    let parts: Vec<String> = sizes
        .iter()
        .filter_map(|(name, mb)| {
            Some(format!(
                "{name} {}",
                format_gb(mb.as_ref()?.saturating_mul(MB))
            ))
        })
        .collect();
    let total_mb = sizes
        .iter()
        .filter_map(|(_, mb)| *mb)
        .fold(0u64, u64::saturating_add);
    let ram = ram_bytes(ctx);

    let mut detail = parts.join(" + ");
    if parts.len() > 1 {
        detail.push_str(&format!(" = {}", format_gb(total_mb.saturating_mul(MB))));
    }
    if let Some(ram) = ram {
        detail.push_str(&format!(" of {} RAM", format_gb(ram)));
    }
    let too_large = ram.is_some_and(|ram| {
        total_mb.saturating_mul(MB).saturating_mul(100) > ram.saturating_mul(MAX_HEAP_SHARE_PERCENT)
    });
    if !too_large {
        return vec![CheckResult::new(
            Category::Project,
            NAME,
            Status::Ok,
            detail,
        )];
    }
    vec![
        CheckResult::new(Category::Project, NAME, Status::Warn, detail).with_hint(
            "In gradle.properties set org.gradle.jvmargs=-Xmx3g and kotlin.daemon.jvmargs=-Xmx2g (`lf-mide tune` suggests more)",
        ),
    ]
}

fn build_server(ctx: &Ctx) -> Vec<CheckResult> {
    const NAME: &str = "Swift build server config";
    let Some(project) = ctx.project else {
        return Vec::new();
    };
    let file = project.root.join("buildServer.json");
    let result = if ctx.sys.exists(&file) {
        CheckResult::new(
            Category::Project,
            NAME,
            Status::Ok,
            "buildServer.json present",
        )
        .with_path(file.display().to_string())
    } else {
        CheckResult::new(
            Category::Project,
            NAME,
            Status::Warn,
            "missing (add it and .compile to .gitignore)",
        )
        .with_hint("xcode-build-server config -project iosApp/iosApp.xcodeproj -scheme iosApp")
    };
    vec![result]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeSystem;
    use lf_mide_core::{Project, ProjectLayout};
    use std::path::PathBuf;

    const SIXTEEN_GB: &str = "17179869184";

    fn project() -> Project {
        Project {
            root: PathBuf::from("/p"),
            name: "demo".into(),
            layout: ProjectLayout::AndroidAppShared,
        }
    }

    fn run(check: CheckFn, sys: &FakeSystem, project: Option<&Project>) -> Vec<CheckResult> {
        check(&Ctx { sys, project })
    }

    fn one(mut results: Vec<CheckResult>) -> CheckResult {
        assert_eq!(results.len(), 1, "{results:?}");
        results.remove(0)
    }

    #[test]
    fn the_users_own_gradle_properties_count_when_the_project_sets_nothing() {
        let sys = FakeSystem::new()
            .home("/Users/me")
            .file("/p/gradle.properties", "kotlin.code.style=official\n")
            .file(
                "/Users/me/.gradle/gradle.properties",
                "org.gradle.jvmargs=-Xmx4096M\nkotlin.daemon.jvmargs=-Xmx3072M\n",
            );

        let r = one(run(gradle_memory, &sys, Some(&project())));

        assert!(r.detail.contains("Gradle 4 GB"), "{}", r.detail);
        assert!(r.detail.contains("Kotlin 3 GB"), "{}", r.detail);
    }

    #[test]
    fn the_projects_value_wins_over_the_users() {
        let sys = FakeSystem::new()
            .home("/Users/me")
            .file("/p/gradle.properties", "org.gradle.jvmargs=-Xmx1024M\n")
            .file(
                "/Users/me/.gradle/gradle.properties",
                "org.gradle.jvmargs=-Xmx8192M\n",
            );

        let r = one(run(gradle_memory, &sys, Some(&project())));

        assert!(r.detail.contains("Gradle 1 GB"), "{}", r.detail);
    }

    #[test]
    fn parses_xmx_in_common_units() {
        assert_eq!(parse_xmx_mb("-Xmx4096M -Dfile.encoding=UTF-8"), Some(4096));
        assert_eq!(parse_xmx_mb("-Dx=1 -Xmx3g"), Some(3072));
        assert_eq!(parse_xmx_mb("-Xmx2048m"), Some(2048));
        assert_eq!(parse_xmx_mb("-Xmx1048576k"), Some(1024));
        assert_eq!(parse_xmx_mb("-Dfile.encoding=UTF-8"), None);
        assert_eq!(parse_xmx_mb("-Xmx"), None);
    }

    #[test]
    fn absurd_xmx_values_saturate_instead_of_overflowing() {
        assert_eq!(parse_xmx_mb("-Xmx18446744073709551615g"), Some(u64::MAX));
        assert_eq!(
            parse_xmx_mb("-Xmx99999999999999999999999g"),
            None,
            "does not fit in u64"
        );
    }

    #[test]
    fn repo_supplied_huge_heaps_warn_without_panicking() {
        let sys = FakeSystem::new()
            .ok("sysctl -n hw.memsize", SIXTEEN_GB)
            .file("/p/gradle.properties", "org.gradle.jvmargs=-Xmx18446744073709551615g\nkotlin.daemon.jvmargs=-Xmx18446744073709551615g\n");
        let p = project();

        let r = one(run(gradle_memory, &sys, Some(&p)));

        assert_eq!(r.status, Status::Warn);
    }

    #[test]
    fn layout_reports_the_detected_template() {
        let p = project();

        let r = one(run(layout, &FakeSystem::new(), Some(&p)));

        assert_eq!(
            (r.status, r.detail.as_str(), r.path.as_deref()),
            (Status::Ok, "androidApp + shared + iosApp", Some("/p"))
        );
    }

    #[test]
    fn layout_warns_outside_a_cmp_project() {
        let r = one(run(layout, &FakeSystem::new(), None));

        assert_eq!(r.status, Status::Warn);
        assert!(r.hint.unwrap().contains("lf-mide doctor <path>"));
    }

    #[test]
    fn large_heaps_on_a_small_machine_warn() {
        // The values from LF-POS-Mobile: 4 GB + 3 GB on 16 GB RAM.
        let sys = FakeSystem::new()
            .ok("sysctl -n hw.memsize", SIXTEEN_GB)
            .file("/p/gradle.properties", "org.gradle.jvmargs=-Xmx4096M -Dfile.encoding=UTF-8\nkotlin.daemon.jvmargs=-Xmx3072M\n");
        let p = project();

        let r = one(run(gradle_memory, &sys, Some(&p)));

        assert_eq!(r.status, Status::Warn);
        assert_eq!(r.detail, "Gradle 4 GB + Kotlin 3 GB = 7 GB of 16 GB RAM");
        assert!(r.hint.unwrap().contains("org.gradle.jvmargs=-Xmx3g"));
    }

    #[test]
    fn modest_heaps_are_fine() {
        let sys = FakeSystem::new()
            .ok("sysctl -n hw.memsize", SIXTEEN_GB)
            .file(
                "/p/gradle.properties",
                "org.gradle.jvmargs=-Xmx3g\nkotlin.daemon.jvmargs=-Xmx2g\n",
            );
        let p = project();

        let r = one(run(gradle_memory, &sys, Some(&p)));

        assert_eq!(
            (r.status, r.detail.as_str()),
            (Status::Ok, "Gradle 3 GB + Kotlin 2 GB = 5 GB of 16 GB RAM")
        );
    }

    #[test]
    fn unset_heaps_use_jvm_defaults_and_no_project_skips() {
        let sys = FakeSystem::new()
            .ok("sysctl -n hw.memsize", SIXTEEN_GB)
            .file("/p/gradle.properties", "kotlin.code.style=official\n");
        let p = project();

        let r = one(run(gradle_memory, &sys, Some(&p)));

        assert_eq!(
            (r.status, r.detail.as_str()),
            (Status::Ok, "no -Xmx set (JVM defaults)")
        );
        assert!(run(gradle_memory, &sys, None).is_empty());
    }

    #[test]
    fn heap_check_without_known_ram_cannot_judge_and_stays_ok() {
        let sys = FakeSystem::new().file("/p/gradle.properties", "org.gradle.jvmargs=-Xmx8g\n");
        let p = project();

        assert_eq!(one(run(gradle_memory, &sys, Some(&p))).status, Status::Ok);
    }

    #[test]
    fn missing_build_server_warns_with_the_config_command() {
        let p = project();

        let missing = one(run(build_server, &FakeSystem::new(), Some(&p)));
        let present = one(run(
            build_server,
            &FakeSystem::new().file("/p/buildServer.json", "{}"),
            Some(&p),
        ));

        assert_eq!(missing.status, Status::Warn);
        assert_eq!(
            missing.hint.as_deref(),
            Some("xcode-build-server config -project iosApp/iosApp.xcodeproj -scheme iosApp")
        );
        assert!(missing.detail.contains(".gitignore"));
        assert_eq!(present.status, Status::Ok);
        assert!(run(build_server, &FakeSystem::new(), None).is_empty());
    }
}
