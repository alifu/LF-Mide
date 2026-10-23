//! The file tree as a pure view of a list of file paths.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

/// Relative paths of every file in the project, sorted. Shared (`Arc`) so
/// that copying the app state never copies the list.
pub type FileList = Arc<Vec<PathBuf>>;

/// One result of fuzzy-finding a file name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileMatch {
    /// Relative to the project root.
    pub path: PathBuf,
    pub score: u32,
    /// Which characters of the path (counted in `char`s) matched the query,
    /// for highlighting.
    pub indices: Vec<u32>,
}

/// One line found by a project search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrepHit {
    /// Relative to the project root.
    pub path: PathBuf,
    /// 1-based.
    pub line: u32,
    /// Byte offset of the first match in the line, 0-based.
    pub column: u32,
    /// The matching line, trimmed and shortened for display.
    pub text: String,
}

/// One visible line of the file tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeRow {
    /// Path relative to the project root.
    pub path: PathBuf,
    pub name: String,
    /// 0 for entries directly in the project root.
    pub depth: usize,
    pub is_dir: bool,
    /// Only meaningful for directories.
    pub expanded: bool,
}

/// The rows to show for `files`, with the directories in `expanded` open.
/// In each directory, subdirectories come first, then files, each sorted
/// case-insensitively. Directories are derived from the file paths, so an
/// empty directory never appears.
pub fn tree_rows(files: &[PathBuf], expanded: &BTreeSet<PathBuf>) -> Vec<TreeRow> {
    let mut root = Dir::default();
    for file in files {
        let Some(clean) = clean_relative(file) else {
            continue;
        };
        let mut parts: Vec<OsString> = clean.iter().map(OsString::from).collect();
        let Some(name) = parts.pop() else { continue };
        let dir = parts
            .into_iter()
            .fold(&mut root, |dir, part| dir.dirs.entry(part).or_default());
        dir.files.insert(name);
    }
    let mut rows = Vec::new();
    root.emit(Path::new(""), 0, expanded, &mut rows);
    rows
}

/// A directory while the tree is being assembled.
#[derive(Default)]
struct Dir {
    dirs: BTreeMap<OsString, Dir>,
    files: BTreeSet<OsString>,
}

/// Case-insensitive ordering key for a file name.
fn sort_key(name: &OsStr) -> (String, OsString) {
    (name.to_string_lossy().to_lowercase(), name.to_os_string())
}

impl Dir {
    fn emit(
        &self,
        prefix: &Path,
        depth: usize,
        expanded: &BTreeSet<PathBuf>,
        rows: &mut Vec<TreeRow>,
    ) {
        let mut dirs: Vec<_> = self.dirs.iter().collect();
        dirs.sort_by_cached_key(|(name, _)| sort_key(name));
        for (name, dir) in dirs {
            let path = prefix.join(name);
            let is_open = expanded.contains(&path);
            rows.push(TreeRow {
                path: path.clone(),
                name: name.to_string_lossy().into_owned(),
                depth,
                is_dir: true,
                expanded: is_open,
            });
            if is_open {
                dir.emit(&path, depth + 1, expanded, rows);
            }
        }
        let mut files: Vec<_> = self.files.iter().collect();
        files.sort_by_cached_key(|name| sort_key(name));
        for name in files {
            rows.push(TreeRow {
                path: prefix.join(name),
                name: name.to_string_lossy().into_owned(),
                depth,
                is_dir: false,
                expanded: false,
            });
        }
    }
}

/// Index of the row for the directory containing row `index`, if it has one.
pub fn parent_row(rows: &[TreeRow], index: usize) -> Option<usize> {
    let parent = rows
        .get(index)?
        .path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())?;
    rows[..index]
        .iter()
        .rposition(|row| row.is_dir && row.path == parent)
}

/// Directories to have open at the start: none, so the tree begins as a
/// short top-level list.
pub fn initially_expanded(_files: &[PathBuf]) -> BTreeSet<PathBuf> {
    BTreeSet::new()
}

/// `path` as a relative path made only of normal components, or `None` for
/// anything else (absolute, `..`, empty).
pub fn clean_relative(path: &Path) -> Option<PathBuf> {
    let mut clean = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => clean.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    (!clean.as_os_str().is_empty()).then_some(clean)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(paths: &[&str]) -> Vec<PathBuf> {
        paths.iter().map(PathBuf::from).collect()
    }

    fn open(dirs: &[&str]) -> BTreeSet<PathBuf> {
        dirs.iter().map(PathBuf::from).collect()
    }

    fn names(rows: &[TreeRow]) -> Vec<String> {
        rows.iter()
            .map(|r| format!("{}{}", "  ".repeat(r.depth), r.name))
            .collect()
    }

    #[test]
    fn top_level_lists_directories_before_files_case_insensitively() {
        let list = files(&["zeta.md", "Beta/x.kt", "alpha/y.kt", "README.md", "gradlew"]);

        let rows = tree_rows(&list, &BTreeSet::new());

        assert_eq!(
            names(&rows),
            vec!["alpha", "Beta", "gradlew", "README.md", "zeta.md"]
        );
        assert!(rows[0].is_dir && rows[1].is_dir && !rows[2].is_dir);
    }

    #[test]
    fn collapsed_directories_hide_their_contents() {
        let list = files(&["src/main/App.kt", "src/main/Util.kt", "src/test/AppTest.kt"]);

        let collapsed = tree_rows(&list, &BTreeSet::new());
        let src_open = tree_rows(&list, &open(&["src"]));
        let both_open = tree_rows(&list, &open(&["src", "src/main"]));

        assert_eq!(names(&collapsed), vec!["src"]);
        assert_eq!(names(&src_open), vec!["src", "  main", "  test"]);
        assert_eq!(
            names(&both_open),
            vec!["src", "  main", "    App.kt", "    Util.kt", "  test"]
        );
    }

    #[test]
    fn an_open_directory_inside_a_closed_one_stays_hidden() {
        let list = files(&["a/b/c.txt"]);

        let rows = tree_rows(&list, &open(&["a/b"]));

        assert_eq!(names(&rows), vec!["a"]);
    }

    #[test]
    fn rows_carry_paths_depth_and_expanded_state() {
        let list = files(&["a/b.txt", "a/c/d.txt"]);

        let rows = tree_rows(&list, &open(&["a"]));

        assert_eq!(rows[0].path, PathBuf::from("a"));
        assert!(rows[0].is_dir && rows[0].expanded);
        assert_eq!(rows[1].path, PathBuf::from("a/c"));
        assert!(rows[1].is_dir && !rows[1].expanded);
        assert_eq!(
            (rows[2].path.clone(), rows[2].depth, rows[2].is_dir),
            (PathBuf::from("a/b.txt"), 1, false)
        );
    }

    #[test]
    fn odd_paths_are_skipped_and_duplicates_collapsed() {
        let list = files(&[
            "/abs/x.txt",
            "../up.txt",
            "",
            "ok.txt",
            "ok.txt",
            "./dot.txt",
        ]);

        let rows = tree_rows(&list, &BTreeSet::new());

        assert_eq!(names(&rows), vec!["dot.txt", "ok.txt"]);
    }

    #[test]
    fn empty_list_gives_no_rows() {
        assert!(tree_rows(&[], &BTreeSet::new()).is_empty());
    }

    #[test]
    fn parent_row_finds_the_containing_directory() {
        let list = files(&["a/b/c.txt", "a/d.txt", "z.txt"]);
        let rows = tree_rows(&list, &open(&["a", "a/b"]));
        // rows: a, a/b, a/b/c.txt, a/d.txt, z.txt

        assert_eq!(parent_row(&rows, 2), Some(1));
        assert_eq!(parent_row(&rows, 1), Some(0));
        assert_eq!(parent_row(&rows, 3), Some(0));
        assert_eq!(parent_row(&rows, 0), None, "top-level has no parent row");
        assert_eq!(parent_row(&rows, 4), None);
        assert_eq!(parent_row(&rows, 99), None);
    }

    #[test]
    fn clean_relative_accepts_only_plain_relative_paths() {
        assert_eq!(
            clean_relative(Path::new("a/b.txt")),
            Some(PathBuf::from("a/b.txt"))
        );
        assert_eq!(
            clean_relative(Path::new("./a/./b")),
            Some(PathBuf::from("a/b"))
        );
        assert_eq!(clean_relative(Path::new("/abs")), None);
        assert_eq!(clean_relative(Path::new("a/../b")), None);
        assert_eq!(clean_relative(Path::new("..")), None);
        assert_eq!(clean_relative(Path::new("")), None);
        assert_eq!(clean_relative(Path::new(".")), None);
    }

    #[test]
    fn the_tree_starts_collapsed() {
        assert!(initially_expanded(&files(&["a/b.txt"])).is_empty());
    }
}
