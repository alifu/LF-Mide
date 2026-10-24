//! What the project can build: Gradle variants (flavors × build types) for
//! Android, Xcode schemes for iOS. Read from the build files, so asking is
//! quick and never starts Gradle.

use std::path::Path;

use lf_mide_core::{Platform, ProjectLayout};
use lf_mide_system::System;

use crate::{android, ios};

const DEFAULT_BUILD_TYPES: [&str; 2] = ["debug", "release"];

/// One Android variant: `freeDebug` is flavor `free` with build type `debug`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Variant {
    pub name: String,
    pub flavor: Option<String>,
    pub build_type: String,
}

/// The names to offer for `platform`.
pub fn list_variants(
    platform: Platform,
    root: &Path,
    layout: ProjectLayout,
    sys: &dyn System,
) -> Result<Vec<String>, String> {
    match platform {
        Platform::Android => {
            let text = android::build_file_text(root, layout, sys)?;
            Ok(android_variants(&text)?
                .into_iter()
                .map(|variant| variant.name)
                .collect())
        }
        Platform::Ios => ios::scheme_names(root, sys),
    }
}

/// Flavors × build types, debug variants first.
pub(crate) fn android_variants(build_file: &str) -> Result<Vec<Variant>, String> {
    let dimensions = android::block_after(build_file, "flavorDimensions")
        .map(|_| 0)
        .or_else(|| {
            build_file
                .lines()
                .find(|line| line.contains("flavorDimensions"))
                .map(|line| line.matches('"').count() / 2)
        })
        .unwrap_or(0);
    if dimensions > 1 {
        return Err("several flavor dimensions are not supported yet".to_string());
    }
    let flavors = names_in(build_file, "productFlavors");
    let mut build_types: Vec<String> = DEFAULT_BUILD_TYPES.map(String::from).to_vec();
    for name in names_in(build_file, "buildTypes") {
        if !build_types.contains(&name) {
            build_types.push(name);
        }
    }
    let mut variants = Vec::new();
    let flavors: Vec<Option<String>> = if flavors.is_empty() {
        vec![None]
    } else {
        flavors.into_iter().map(Some).collect()
    };
    for build_type in &build_types {
        for flavor in &flavors {
            variants.push(Variant {
                name: variant_name(flavor.as_deref(), build_type),
                flavor: flavor.clone(),
                build_type: build_type.clone(),
            });
        }
    }
    variants.sort_by_key(|variant| variant.build_type != "debug");
    Ok(variants)
}

/// `free` + `debug` → `freeDebug`; no flavor → `debug`.
fn variant_name(flavor: Option<&str>, build_type: &str) -> String {
    match flavor {
        Some(flavor) => format!("{flavor}{}", capitalized(build_type)),
        None => build_type.to_string(),
    }
}

pub(crate) fn capitalized(name: &str) -> String {
    let mut chars = name.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// Names declared directly inside the `block { ... }`: `create("x")`,
/// `register("x")`, `getByName("x")` or a bare `x { ... }`.
fn names_in(build_file: &str, block: &str) -> Vec<String> {
    let Some(body) = android::block_after(build_file, block) else {
        return Vec::new();
    };
    let mut depth = 0usize;
    let mut names = Vec::new();
    for line in body.lines() {
        let line = line.split("//").next().unwrap_or("");
        if depth == 0
            && let Some(name) = declared_name(line)
            && !names.contains(&name)
        {
            names.push(name);
        }
        depth = (depth + line.matches('{').count()).saturating_sub(line.matches('}').count());
    }
    names
}

fn declared_name(line: &str) -> Option<String> {
    let line = line.trim();
    for call in ["create(", "register(", "getByName(", "maybeCreate("] {
        if let Some(rest) = line.strip_prefix(call) {
            let rest = rest.strip_prefix(['"', '\''])?;
            let end = rest.find(['"', '\''])?;
            return valid_name(&rest[..end]);
        }
    }
    let ident: String = line
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    let rest = line[ident.len()..].trim_start();
    if rest.starts_with('{') {
        return valid_name(&ident);
    }
    None
}

fn valid_name(name: &str) -> Option<String> {
    (name.starts_with(|c: char| c.is_ascii_alphabetic())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
    .then(|| name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(build_file: &str) -> Vec<String> {
        android_variants(build_file)
            .unwrap()
            .into_iter()
            .map(|variant| variant.name)
            .collect()
    }

    #[test]
    fn a_plain_project_has_debug_and_release() {
        assert_eq!(names("android { }"), ["debug", "release"]);
    }

    #[test]
    fn flavors_and_custom_build_types_combine_with_debug_first() {
        let file = r#"
android {
    buildTypes {
        release { isMinifyEnabled = true }
        create("staging") { initWith(getByName("debug")) }
    }
    productFlavors {
        create("free") { applicationIdSuffix = ".free" }
        register("paid")
    }
}
"#;

        assert_eq!(
            names(file),
            [
                "freeDebug",
                "paidDebug",
                "freeRelease",
                "paidRelease",
                "freeStaging",
                "paidStaging"
            ]
        );
    }

    #[test]
    fn groovy_style_bare_blocks_are_found_and_nested_blocks_are_not() {
        let file =
            "productFlavors {\n  dev {\n    manifestPlaceholders { x }\n  }\n  prod {\n  }\n}";

        assert_eq!(&names(file)[..2], ["devDebug", "prodDebug"]);
    }

    #[test]
    fn several_flavor_dimensions_are_refused_not_guessed() {
        let file =
            "flavorDimensions += listOf(\"tier\", \"env\")\nproductFlavors { create(\"a\") }";

        assert!(android_variants(file).unwrap_err().contains("dimension"));
        assert!(android_variants("flavorDimensions += \"tier\"").is_ok());
    }

    #[test]
    fn names_are_capitalized_and_unsafe_names_are_dropped() {
        assert_eq!(capitalized("debug"), "Debug");
        assert_eq!(capitalized(""), "");
        assert_eq!(declared_name("create(\"a;b\")"), None);
        assert_eq!(declared_name("create(\"ok_1\")").as_deref(), Some("ok_1"));
    }
}
