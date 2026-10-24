//! Editor, language servers and search tools.

use lf_mide_core::{Category, CheckResult};

use super::{CheckFn, Ctx, ToolSpec, tool};
use crate::version::Version;

const MIN_NEOVIM: Version = Version::new(0, 11, 0);

pub const CHECKS: &[CheckFn] = &[editor_tools];

fn editor_tools(ctx: &Ctx) -> Vec<CheckResult> {
    let spec = |name, binary, version_args, min, required, hint| ToolSpec {
        category: Category::Editor,
        name,
        binary,
        version_args,
        min,
        required,
        hint,
    };
    let specs = [
        spec(
            "Neovim",
            "nvim",
            &["--version"][..],
            Some(MIN_NEOVIM),
            true,
            "brew install neovim",
        ),
        spec(
            "kmp-lsp",
            "kmp-lsp",
            &["--version"],
            None,
            false,
            "cargo install --locked kmp-lsp",
        ),
        spec(
            "sourcekit-lsp",
            "sourcekit-lsp",
            &[],
            None,
            false,
            "xcode-select --install",
        ),
        spec(
            "xcode-build-server",
            "xcode-build-server",
            &[],
            None,
            false,
            "brew install xcode-build-server",
        ),
        spec(
            "clangd",
            "clangd",
            &["--version"],
            None,
            false,
            "xcode-select --install",
        ),
        spec(
            "ripgrep",
            "rg",
            &["--version"],
            None,
            true,
            "brew install ripgrep",
        ),
        spec(
            "git",
            "git",
            &["--version"],
            None,
            true,
            "xcode-select --install",
        ),
    ];
    specs.iter().map(|spec| tool(ctx, spec, None)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeSystem;
    use lf_mide_core::Status;

    fn installed(sys: FakeSystem, binary: &str, version_args: &str, output: &str) -> FakeSystem {
        let path = format!("/opt/bin/{binary}");
        let command = format!("{path} {version_args}");
        sys.file(&path, "").ok(command.trim_end(), output)
    }

    fn run(sys: &FakeSystem) -> Vec<CheckResult> {
        editor_tools(&Ctx { sys, project: None })
    }

    fn find<'a>(results: &'a [CheckResult], name: &str) -> &'a CheckResult {
        results
            .iter()
            .find(|r| r.name == name)
            .unwrap_or_else(|| panic!("no result named {name}"))
    }

    #[test]
    fn reports_every_tool_in_the_plan() {
        let results = run(&FakeSystem::new().env("PATH", "/opt/bin"));

        let names: Vec<_> = results.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "Neovim",
                "kmp-lsp",
                "sourcekit-lsp",
                "xcode-build-server",
                "clangd",
                "ripgrep",
                "git"
            ]
        );
        assert!(results.iter().all(|r| r.category == Category::Editor));
    }

    #[test]
    fn neovim_is_required_and_language_servers_are_optional() {
        let results = run(&FakeSystem::new().env("PATH", "/opt/bin"));

        assert_eq!(find(&results, "Neovim").status, Status::Missing);
        assert_eq!(find(&results, "git").status, Status::Missing);
        assert_eq!(find(&results, "kmp-lsp").status, Status::Warn);
        assert_eq!(find(&results, "xcode-build-server").status, Status::Warn);
    }

    #[test]
    fn old_neovim_is_flagged() {
        let sys = installed(
            FakeSystem::new().env("PATH", "/opt/bin"),
            "nvim",
            "--version",
            "NVIM v0.10.4",
        );

        let r = run(&sys);

        assert_eq!(find(&r, "Neovim").status, Status::Warn);
        assert!(find(&r, "Neovim").detail.contains("need ≥ 0.11.0"));
    }

    #[test]
    fn hints_name_the_install_commands() {
        let results = run(&FakeSystem::new().env("PATH", "/opt/bin"));

        assert_eq!(
            find(&results, "kmp-lsp").hint.as_deref(),
            Some("cargo install --locked kmp-lsp")
        );
        assert_eq!(
            find(&results, "xcode-build-server").hint.as_deref(),
            Some("brew install xcode-build-server")
        );
        assert_eq!(
            find(&results, "ripgrep").hint.as_deref(),
            Some("brew install ripgrep")
        );
    }
}
