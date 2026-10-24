//! The search against the real ripgrep (skipped when it is not installed).

use std::ffi::OsString;
use std::fs;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};

use lf_mide_nav::grep::grep;
use lf_mide_system::SystemRunner;

fn rg_available() -> bool {
    std::process::Command::new("rg")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

fn write(root: &Path, relative: &str, text: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

#[test]
fn hits_come_back_relative_to_the_project() {
    if !rg_available() {
        return;
    }
    let tmp = tempfile::TempDir::new().unwrap();
    write(
        tmp.path(),
        "shared/src/App.kt",
        "fun main() {\n    setActive(true)\n}\n",
    );
    write(tmp.path(), "notes.txt", "setActive twice: setActive\n");
    write(
        tmp.path(),
        "build/out.txt",
        "setActive in a generated folder\n",
    );

    let hits = grep(&SystemRunner::new(), tmp.path(), "setActive", 50).unwrap();

    let found: Vec<(String, u32)> = hits
        .iter()
        .map(|h| (h.path.display().to_string(), h.line))
        .collect();
    assert_eq!(
        found,
        [
            ("notes.txt".to_string(), 1),
            ("shared/src/App.kt".to_string(), 2)
        ]
    );
}

#[test]
fn a_project_folder_with_awkward_characters_is_searched_like_any_other() {
    if !rg_available() {
        return;
    }
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().join("my project [v2] -x \u{e9}");
    write(&root, "a.kt", "needle here\n");

    let hits = grep(&SystemRunner::new(), &root, "needle", 5).unwrap();

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].path, PathBuf::from("a.kt"));
}

#[test]
fn names_that_are_not_valid_text_are_found_with_their_exact_bytes() {
    if !rg_available() {
        return;
    }
    let tmp = tempfile::TempDir::new().unwrap();
    let odd = OsString::from_vec(b"caf\xe9.kt".to_vec());
    // Some file systems (macOS's APFS among them) refuse such a name: then
    // there is nothing to test here.
    if fs::write(tmp.path().join(&odd), "needle\n").is_err() {
        return;
    }
    write(tmp.path(), "plain.kt", "needle\n");

    let hits = grep(&SystemRunner::new(), tmp.path(), "needle", 10).unwrap();

    let paths: Vec<PathBuf> = hits.into_iter().map(|h| h.path).collect();
    assert!(paths.contains(&PathBuf::from(odd)), "{paths:?}");
    assert!(paths.contains(&PathBuf::from("plain.kt")));
}

#[test]
fn a_folder_whose_own_name_is_not_valid_text_is_searched() {
    if !rg_available() {
        return;
    }
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().join(OsString::from_vec(b"proj\xff".to_vec()));
    if fs::create_dir(&root).is_err() {
        return; // the file system does not allow such names
    }
    write(&root, "a.kt", "needle\n");

    let hits = grep(&SystemRunner::new(), &root, "needle", 5).unwrap();

    assert_eq!(hits.len(), 1);
}
