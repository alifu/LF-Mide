//! Listing the files of a project.

use std::io;
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

/// Directories never worth listing, whatever `.gitignore` says.
pub const EXCLUDED_DIRS: [&str; 8] = [
    ".git",
    ".gradle",
    ".kotlin",
    ".idea",
    "build",
    "DerivedData",
    "node_modules",
    "Pods",
];

/// Stop after this many files so a huge directory can't exhaust memory.
pub const MAX_FILES: usize = 200_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scan {
    /// Relative to the root, sorted.
    pub files: Vec<PathBuf>,
    /// True if [`MAX_FILES`] was reached.
    pub truncated: bool,
}

/// Every regular file under `root`, honoring `.gitignore` (also outside a
/// git repository) and skipping [`EXCLUDED_DIRS`]. Symbolic links are not
/// followed and not listed, so a link can neither loop nor leave the project.
pub fn scan(root: &Path) -> io::Result<Scan> {
    if !std::fs::metadata(root)?.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            "project root is not a directory",
        ));
    }
    let walker = WalkBuilder::new(root)
        .hidden(false)
        .follow_links(false)
        // Apply .gitignore files even when there is no `.git` yet, but not
        // the user's global ignore file: the list should not depend on it.
        .require_git(false)
        .git_global(false)
        .filter_entry(|entry| {
            let is_dir = entry.file_type().is_some_and(|kind| kind.is_dir());
            let excluded = entry.depth() > 0
                && is_dir
                && entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| EXCLUDED_DIRS.contains(&name));
            !excluded
        })
        .build();

    let mut files = Vec::new();
    let mut truncated = false;
    // Unreadable entries are skipped: a partial list beats no list.
    for entry in walker.flatten() {
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        if files.len() >= MAX_FILES {
            truncated = true;
            break;
        }
        if let Ok(relative) = entry.path().strip_prefix(root) {
            files.push(relative.to_path_buf());
        }
    }
    files.sort();
    Ok(Scan { files, truncated })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn touch(root: &Path, relative: &str) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "x").unwrap();
    }

    fn names(scan: &Scan) -> Vec<String> {
        scan.files.iter().map(|p| p.display().to_string()).collect()
    }

    #[test]
    fn lists_files_sorted_with_relative_paths() {
        let dir = TempDir::new().unwrap();
        for f in ["b.txt", "a/z.kt", "a/y.kt", "README.md"] {
            touch(dir.path(), f);
        }

        let scan = scan(dir.path()).unwrap();

        assert_eq!(names(&scan), vec!["README.md", "a/y.kt", "a/z.kt", "b.txt"]);
        assert!(!scan.truncated);
    }

    #[test]
    fn honors_gitignore_even_without_a_git_repository() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join(".gitignore"), "ignored.txt\nout/\n*.log\n").unwrap();
        for f in [
            "keep.kt",
            "ignored.txt",
            "out/x.class",
            "debug.log",
            "src/app.log",
        ] {
            touch(dir.path(), f);
        }

        let scan = scan(dir.path()).unwrap();

        assert_eq!(names(&scan), vec![".gitignore", "keep.kt"]);
    }

    #[test]
    fn skips_build_output_and_tool_directories_but_keeps_dotfiles() {
        let dir = TempDir::new().unwrap();
        for f in [
            "src/App.kt",
            ".git/config",
            ".gradle/x",
            "build/out.bin",
            "shared/build/y",
            ".idea/workspace.xml",
            "DerivedData/z",
            ".github/ci.yml",
            ".env.example",
        ] {
            touch(dir.path(), f);
        }

        let scan = scan(dir.path()).unwrap();

        assert_eq!(
            names(&scan),
            vec![".env.example", ".github/ci.yml", "src/App.kt"]
        );
    }

    #[test]
    fn does_not_follow_or_list_symbolic_links() {
        use std::os::unix::fs::symlink;
        let outside = TempDir::new().unwrap();
        touch(outside.path(), "secret.txt");
        let dir = TempDir::new().unwrap();
        touch(dir.path(), "real.txt");
        symlink(outside.path(), dir.path().join("linked_dir")).unwrap();
        symlink(
            outside.path().join("secret.txt"),
            dir.path().join("linked_file"),
        )
        .unwrap();
        symlink(dir.path(), dir.path().join("loop")).unwrap();

        let scan = scan(dir.path()).unwrap();

        assert_eq!(names(&scan), vec!["real.txt"]);
    }

    #[test]
    fn empty_directories_and_empty_projects_give_no_files() {
        let dir = TempDir::new().unwrap();
        fs::create_dir(dir.path().join("empty")).unwrap();

        assert!(scan(dir.path()).unwrap().files.is_empty());
    }

    #[test]
    fn a_missing_root_is_an_error() {
        let dir = TempDir::new().unwrap();

        assert!(scan(&dir.path().join("nope")).is_err());
    }

    #[test]
    fn keeps_unusual_file_names_intact() {
        let dir = TempDir::new().unwrap();
        for f in ["with space.kt", "quo'te.kt", "new\nline.kt", "é日本.kt"] {
            touch(dir.path(), f);
        }

        let scan = scan(dir.path()).unwrap();

        assert_eq!(scan.files.len(), 4);
        assert!(scan.files.contains(&PathBuf::from("new\nline.kt")));
    }
}
