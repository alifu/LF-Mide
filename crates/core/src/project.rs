//! Compose Multiplatform project detection.

use std::path::{Path, PathBuf};

use thiserror::Error;

const SETTINGS_FILES: [&str; 2] = ["settings.gradle.kts", "settings.gradle"];
const IOS_DIR: &str = "iosApp";

/// Which CMP template the project follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectLayout {
    /// `composeApp/` holds shared code and the Android app.
    ComposeApp,
    /// `androidApp/` + `shared/` (newer template).
    AndroidAppShared,
}

impl ProjectLayout {
    pub fn label(self) -> &'static str {
        match self {
            ProjectLayout::ComposeApp => "composeApp",
            ProjectLayout::AndroidAppShared => "androidApp + shared",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub root: PathBuf,
    pub name: String,
    pub layout: ProjectLayout,
}

#[derive(Debug, Error)]
pub enum ProjectError {
    #[error("no Gradle project (settings.gradle[.kts]) found in {0} or its parents")]
    NotFound(PathBuf),
    #[error(
        "{0} is not a Compose Multiplatform project: expected iosApp/ plus composeApp/ or androidApp/ + shared/"
    )]
    UnknownLayout(PathBuf),
    #[error("failed to read {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

/// Finds the CMP project containing `start` by walking up to the nearest
/// directory with a Gradle settings file.
pub fn detect(start: &Path) -> Result<Project, ProjectError> {
    let (root, settings) = start
        .ancestors()
        .find_map(|dir| {
            SETTINGS_FILES
                .iter()
                .map(|file| dir.join(file))
                .find(|path| path.is_file())
                .map(|settings| (dir.to_path_buf(), settings))
        })
        .ok_or_else(|| ProjectError::NotFound(start.to_path_buf()))?;

    let layout = detect_layout(&root).ok_or_else(|| ProjectError::UnknownLayout(root.clone()))?;
    let text = std::fs::read_to_string(&settings).map_err(|source| ProjectError::Io {
        path: settings.clone(),
        source,
    })?;
    let name = root_project_name(&text).unwrap_or_else(|| dir_name(&root));

    Ok(Project { root, name, layout })
}

fn detect_layout(root: &Path) -> Option<ProjectLayout> {
    let has_dir = |name: &str| root.join(name).is_dir();
    if !has_dir(IOS_DIR) {
        return None;
    }
    if has_dir("composeApp") {
        Some(ProjectLayout::ComposeApp)
    } else if has_dir("androidApp") && has_dir("shared") {
        Some(ProjectLayout::AndroidAppShared)
    } else {
        None
    }
}

/// Extracts the value of `rootProject.name = "..."` (single or double quotes).
fn root_project_name(settings: &str) -> Option<String> {
    settings.lines().find_map(|line| {
        let value = line.trim().strip_prefix("rootProject.name")?.trim_start();
        let value = value.strip_prefix('=')?.trim();
        let quote = value.chars().next().filter(|c| *c == '"' || *c == '\'')?;
        let inner = &value[1..];
        inner.find(quote).map(|end| inner[..end].to_string())
    })
}

fn dir_name(root: &Path) -> String {
    root.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn make_project(dirs: &[&str], settings: &str) -> TempDir {
        let tmp = TempDir::new().unwrap();
        for dir in dirs {
            fs::create_dir_all(tmp.path().join(dir)).unwrap();
        }
        fs::write(tmp.path().join("settings.gradle.kts"), settings).unwrap();
        tmp
    }

    #[test]
    fn layout_labels_name_the_source_folders() {
        assert_eq!(ProjectLayout::ComposeApp.label(), "composeApp");
        assert_eq!(
            ProjectLayout::AndroidAppShared.label(),
            "androidApp + shared"
        );
    }

    #[test]
    fn detects_android_app_shared_layout_and_reads_name() {
        let tmp = make_project(
            &["androidApp", "shared", "iosApp"],
            "rootProject.name = \"lf_pos_mobile\"\ninclude(\":androidApp\")\n",
        );

        let project = detect(tmp.path()).unwrap();

        assert_eq!(project.layout, ProjectLayout::AndroidAppShared);
        assert_eq!(project.name, "lf_pos_mobile");
        assert_eq!(project.root, tmp.path());
    }

    #[test]
    fn detects_compose_app_layout() {
        let tmp = make_project(&["composeApp", "iosApp"], "rootProject.name = 'Demo'\n");

        let project = detect(tmp.path()).unwrap();

        assert_eq!(project.layout, ProjectLayout::ComposeApp);
        assert_eq!(project.name, "Demo");
    }

    #[test]
    fn finds_root_from_nested_directory() {
        let tmp = make_project(&["composeApp/src/commonMain", "iosApp"], "");

        let project = detect(&tmp.path().join("composeApp/src/commonMain")).unwrap();

        assert_eq!(project.root, tmp.path());
    }

    #[test]
    fn falls_back_to_directory_name_when_root_project_name_missing() {
        let tmp = make_project(&["composeApp", "iosApp"], "include(\":composeApp\")\n");

        let project = detect(tmp.path()).unwrap();

        let dir_name = tmp.path().file_name().unwrap().to_string_lossy();
        assert_eq!(project.name, dir_name);
    }

    #[test]
    fn rejects_gradle_project_without_ios_app() {
        let tmp = make_project(&["composeApp"], "");

        let err = detect(tmp.path()).unwrap_err();

        assert!(matches!(err, ProjectError::UnknownLayout(_)));
    }

    #[test]
    fn reports_not_found_outside_gradle_project() {
        let tmp = TempDir::new().unwrap();

        let err = detect(tmp.path()).unwrap_err();

        assert!(matches!(err, ProjectError::NotFound(_)));
    }
}
