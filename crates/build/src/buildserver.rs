//! `buildServer.json` for Swift completion: `sourcekit-lsp` needs a build
//! server to understand an `.xcodeproj`, and `xcode-build-server` provides
//! it. This only works out the command; nothing runs unless the caller
//! asks (the file is machine specific and belongs in the project folder).

use std::path::{Path, PathBuf};

use lf_mide_system::System;

use crate::ios;

const TOOL: &str = "xcode-build-server";
const CONFIG_FILE: &str = "buildServer.json";

/// What `xcode-build-server config` would run, and where it writes.
#[derive(Debug, PartialEq, Eq)]
pub struct BuildServerPlan {
    pub program: PathBuf,
    pub args: Vec<String>,
    /// The Xcode project or workspace it reads, relative to the project.
    pub container: PathBuf,
    pub scheme: String,
    pub config_file: PathBuf,
    /// A `buildServer.json` is already there and would be replaced.
    pub replaces: bool,
}

impl BuildServerPlan {
    /// The command as a person would type it.
    pub fn command_line(&self) -> String {
        let name = self
            .program
            .file_name()
            .map_or_else(|| TOOL.to_string(), |n| n.to_string_lossy().into_owned());
        std::iter::once(name)
            .chain(self.args.iter().cloned())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Works out the `xcode-build-server config` command for the project in
/// `root`: the same Xcode project and scheme a build would use.
pub fn plan(root: &Path, sys: &dyn System) -> Result<BuildServerPlan, String> {
    let program = sys
        .which(TOOL)
        .ok_or_else(|| format!("{TOOL} is not installed (brew install {TOOL})"))?;
    let container = ios::find_container(root, sys)?;
    let relative = container.relative.display().to_string();
    let listing = sys
        .run_in(
            root,
            "xcrun",
            &["xcodebuild", container.flag, &relative, "-list", "-json"],
        )
        .map_err(|error| format!("xcodebuild: {error}"))?;
    if !listing.success {
        return Err("xcodebuild could not list the Xcode schemes".to_string());
    }
    let scheme = ios::schemes_from(&listing.stdout, &container.stem)
        .ok_or_else(|| "no usable Xcode scheme found".to_string())?;
    let config_file = root.join(CONFIG_FILE);
    Ok(BuildServerPlan {
        replaces: sys.exists(&config_file),
        args: vec![
            "config".to_string(),
            container.flag.to_string(),
            relative,
            "-scheme".to_string(),
            scheme.clone(),
        ],
        container: container.relative,
        scheme,
        config_file,
        program,
    })
}

/// Runs the planned command in `root`; `Ok` when the config file exists
/// afterwards.
pub fn apply(plan: &BuildServerPlan, root: &Path, sys: &dyn System) -> Result<(), String> {
    let args: Vec<&str> = plan.args.iter().map(String::as_str).collect();
    let output = sys
        .run_in(root, &plan.program.display().to_string(), &args)
        .map_err(|error| format!("{TOOL}: {error}"))?;
    if !output.success {
        let reason = output.stderr.lines().next().unwrap_or("it failed").trim();
        return Err(format!("{TOOL} failed: {reason}"));
    }
    if sys.exists(&plan.config_file) {
        Ok(())
    } else {
        Err(format!("{TOOL} ran but wrote no {CONFIG_FILE}"))
    }
}

#[cfg(test)]
mod tests {
    use lf_mide_system::fake::FakeSystem;

    use super::*;

    const LIST: &str = "xcrun xcodebuild -project iosApp/iosApp.xcodeproj -list -json";
    const SCHEMES: &str = r#"{"project":{"schemes":["Other","iosApp"]}}"#;

    fn project() -> FakeSystem {
        FakeSystem::new()
            .env("PATH", "/opt/homebrew/bin")
            .file("/opt/homebrew/bin/xcode-build-server", "")
            .dir("/p/iosApp", &["iosApp.xcodeproj"])
            .ok(LIST, SCHEMES)
    }

    #[test]
    fn the_command_uses_the_project_and_the_scheme_a_build_would() {
        let plan = plan(Path::new("/p"), &project()).unwrap();

        assert_eq!(plan.scheme, "iosApp");
        assert_eq!(
            plan.command_line(),
            "xcode-build-server config -project iosApp/iosApp.xcodeproj -scheme iosApp"
        );
        assert_eq!(plan.config_file, Path::new("/p/buildServer.json"));
        assert!(!plan.replaces);
    }

    #[test]
    fn an_existing_config_is_reported_as_replaced() {
        let sys = project().file("/p/buildServer.json", "{}");

        assert!(plan(Path::new("/p"), &sys).unwrap().replaces);
    }

    #[test]
    fn a_missing_tool_says_how_to_install_it() {
        let sys = FakeSystem::new().dir("/p/iosApp", &["iosApp.xcodeproj"]);

        let error = plan(Path::new("/p"), &sys).unwrap_err();

        assert!(error.contains("brew install xcode-build-server"), "{error}");
    }

    #[test]
    fn no_xcode_project_or_scheme_is_an_error() {
        let none = FakeSystem::new()
            .env("PATH", "/bin")
            .file("/bin/xcode-build-server", "");
        let dash = project().ok(LIST, r#"{"project":{"schemes":["-bad"]}}"#);

        assert!(plan(Path::new("/p"), &none).is_err());
        assert!(plan(Path::new("/p"), &dash).is_err());
    }

    #[test]
    fn applying_needs_the_tool_to_succeed_and_write_the_file() {
        let p = plan(Path::new("/p"), &project()).unwrap();
        let line = "/opt/homebrew/bin/xcode-build-server config -project iosApp/iosApp.xcodeproj -scheme iosApp";
        let wrote = project().ok(line, "").file("/p/buildServer.json", "{}");
        let silent = project().ok(line, "");
        let failed = project().fails(line, "no such scheme\nmore");

        assert_eq!(apply(&p, Path::new("/p"), &wrote), Ok(()));
        assert!(
            apply(&p, Path::new("/p"), &silent)
                .unwrap_err()
                .contains("wrote no")
        );
        assert_eq!(
            apply(&p, Path::new("/p"), &failed).unwrap_err(),
            "xcode-build-server failed: no such scheme"
        );
    }
}
