//! Gradle memory settings worth having, and adding the missing ones.
//!
//! Settings go into the user's own `~/.gradle/gradle.properties`, which
//! applies to every project and is not part of any repository. A key that is
//! already set there or in the project is left alone.

use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use lf_mide_core::props;

/// The largest properties file read or rewritten.
const MAX_FILE_BYTES: u64 = 1024 * 1024;
const BACKUP_SUFFIX: &str = ".lf-mide-backup";
const HEADER: &str = "# Added by `lf-mide tune`";
const NEW_FILE_MODE: u32 = 0o644;

/// A setting LF-Mide suggests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Setting {
    pub key: &'static str,
    pub value: &'static str,
    pub why: &'static str,
    /// Not added unless asked for: it can break some builds.
    pub optional: bool,
}

pub const SETTINGS: [Setting; 5] = [
    Setting {
        key: "org.gradle.jvmargs",
        value: "-Xmx3g -XX:+UseParallelGC -Dfile.encoding=UTF-8",
        why: "A ceiling for the Gradle daemon's memory; the parallel collector also uses less than the default.",
        optional: false,
    },
    Setting {
        key: "kotlin.daemon.jvmargs",
        value: "-Xmx2g",
        why: "Without it the Kotlin compiler daemon copies Gradle's size, so you pay for two big heaps.",
        optional: false,
    },
    Setting {
        key: "org.gradle.daemon.idletimeout",
        value: "1800000",
        why: "Stops an idle daemon after 30 minutes instead of 3 hours, giving its memory back sooner.",
        optional: false,
    },
    Setting {
        key: "org.gradle.caching",
        value: "true",
        why: "Reuses earlier build results: less compiling, less waiting.",
        optional: false,
    },
    Setting {
        key: "org.gradle.configuration-cache",
        value: "true",
        why: "Skips Gradle's configuration step on repeat builds. Some plugins do not support it yet, so it is only added when asked for.",
        optional: true,
    },
];

/// Where a key already has a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    User,
    Project,
}

/// A suggested setting and what is there now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub setting: Setting,
    pub current: Option<(Source, String)>,
}

impl Row {
    pub fn is_missing(&self) -> bool {
        self.current.is_none()
    }
}

/// Compares the suggestions with the two files (their text, if they exist).
/// The project's value wins over the user's, as in Gradle.
pub fn review(user: Option<&str>, project: Option<&str>) -> Vec<Row> {
    SETTINGS
        .iter()
        .map(|setting| {
            let found = |text: Option<&str>, source| {
                text.and_then(|t| props::get(t, setting.key))
                    .map(|value| (source, value))
            };
            Row {
                setting: *setting,
                current: found(project, Source::Project).or_else(|| found(user, Source::User)),
            }
        })
        .collect()
}

/// What to add: the missing, non-optional settings, plus the optional ones
/// if `with_optional`.
pub fn additions(rows: &[Row], with_optional: bool) -> Vec<(&'static str, &'static str)> {
    rows.iter()
        .filter(|row| row.is_missing() && (with_optional || !row.setting.optional))
        .map(|row| (row.setting.key, row.setting.value))
        .collect()
}

#[derive(Debug, PartialEq, Eq)]
pub enum TuneError {
    /// The file is a symbolic link (dotfile managers do this): not written
    /// through; add the lines by hand.
    Symlink(PathBuf),
    TooLarge(PathBuf),
    NotText(PathBuf),
    Io(String),
}

impl std::fmt::Display for TuneError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TuneError::Symlink(p) => write!(
                f,
                "{} is a symbolic link: add the lines yourself",
                p.display()
            ),
            TuneError::TooLarge(p) => write!(f, "{} is unusually large: not touched", p.display()),
            TuneError::NotText(p) => write!(f, "{} is not plain text: not touched", p.display()),
            TuneError::Io(why) => write!(f, "{why}"),
        }
    }
}

/// What `apply` did.
#[derive(Debug, PartialEq, Eq)]
pub struct Applied {
    pub added: Vec<String>,
    pub backup: Option<PathBuf>,
}

fn io_error(error: io::Error) -> TuneError {
    TuneError::Io(error.to_string())
}

/// Reads `path` if it is a plain, reasonably small text file.
pub fn read_properties(path: &Path) -> Result<Option<String>, TuneError> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(io_error(err)),
    };
    if meta.file_type().is_symlink() {
        return Err(TuneError::Symlink(path.to_path_buf()));
    }
    if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
        return Err(TuneError::TooLarge(path.to_path_buf()));
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .and_then(|f| f.take(MAX_FILE_BYTES).read_to_end(&mut bytes))
        .map_err(io_error)?;
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| TuneError::NotText(path.to_path_buf()))
}

/// Reads `path` for review, following a symbolic link (a dotfiles setup):
/// looking is harmless; only writing refuses links.
pub fn read_for_review(path: &Path) -> Result<Option<String>, TuneError> {
    let meta = match fs::metadata(path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(io_error(err)),
    };
    if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
        return Err(TuneError::TooLarge(path.to_path_buf()));
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .and_then(|f| f.take(MAX_FILE_BYTES).read_to_end(&mut bytes))
        .map_err(io_error)?;
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| TuneError::NotText(path.to_path_buf()))
}

/// Appends `lines` (`key=value`) to the properties file at `path`. The old
/// file is kept once as `<name>.lf-mide-backup`; the new one is written
/// beside it and moved into place, so a crash cannot leave half a file.
pub fn apply(path: &Path, lines: &[(&str, &str)]) -> Result<Applied, TuneError> {
    if lines.is_empty() {
        return Ok(Applied {
            added: Vec::new(),
            backup: None,
        });
    }
    let existing = read_properties(path)?;
    let dir = path
        .parent()
        .ok_or_else(|| TuneError::Io("no folder to write to".to_string()))?;
    fs::create_dir_all(dir).map_err(io_error)?;

    let backup = match &existing {
        Some(text) => make_backup(path, text)?,
        None => None,
    };
    let mut content = existing.clone().unwrap_or_default();
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    if !content.is_empty() {
        content.push('\n');
    }
    content.push_str(HEADER);
    content.push('\n');
    for (key, value) in lines {
        content.push_str(&format!("{key}={value}\n"));
    }

    let mode = fs::metadata(path)
        .map(|m| m.permissions().mode() & 0o777)
        .unwrap_or(NEW_FILE_MODE);
    write_atomically(path, &content, mode)?;
    Ok(Applied {
        added: lines.iter().map(|(key, _)| (*key).to_string()).collect(),
        backup,
    })
}

fn make_backup(path: &Path, text: &str) -> Result<Option<PathBuf>, TuneError> {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(BACKUP_SUFFIX);
    let backup = path.with_file_name(name);
    // The backup is as private as the original: gradle.properties often
    // holds passwords and tokens.
    let mode = fs::metadata(path)
        .map(|m| m.permissions().mode() & 0o777)
        .unwrap_or(NEW_FILE_MODE);
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(&backup)
    {
        Ok(mut file) => {
            file.write_all(text.as_bytes()).map_err(io_error)?;
            Ok(Some(backup))
        }
        // An earlier run already kept the original; keep that one.
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => Ok(None),
        Err(err) => Err(io_error(err)),
    }
}

fn write_atomically(path: &Path, content: &str, mode: u32) -> Result<(), TuneError> {
    let mut temp_name = path.file_name().unwrap_or_default().to_os_string();
    temp_name.push(format!(".lf-mide-tmp-{}", std::process::id()));
    let temp = path.with_file_name(temp_name);
    // Only a file created here is ever removed here: if the name is taken,
    // that file belongs to somebody else.
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(&temp)
        .map_err(io_error)?;
    let result = (|| {
        file.write_all(content.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result.map_err(io_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(rows: &[(&str, &str)]) -> Vec<String> {
        rows.iter().map(|(k, _)| (*k).to_string()).collect()
    }

    #[test]
    fn nothing_set_anywhere_means_every_setting_is_missing() {
        let rows = review(None, None);

        assert_eq!(rows.len(), SETTINGS.len());
        assert!(rows.iter().all(Row::is_missing));
    }

    #[test]
    fn a_value_in_either_file_counts_and_the_project_wins() {
        let user = "org.gradle.jvmargs=-Xmx1g\norg.gradle.caching=false\n";
        let project = "org.gradle.jvmargs=-Xmx6g\nkotlin.daemon.jvmargs=-Xmx1g\n";

        let rows = review(Some(user), Some(project));

        let by_key = |key: &str| rows.iter().find(|r| r.setting.key == key).unwrap();
        assert_eq!(
            by_key("org.gradle.jvmargs").current,
            Some((Source::Project, "-Xmx6g".into()))
        );
        assert_eq!(
            by_key("org.gradle.caching").current,
            Some((Source::User, "false".into()))
        );
        assert_eq!(
            by_key("kotlin.daemon.jvmargs").current,
            Some((Source::Project, "-Xmx1g".into()))
        );
        assert!(by_key("org.gradle.daemon.idletimeout").is_missing());
    }

    #[test]
    fn a_commented_out_setting_is_still_missing() {
        let rows = review(Some("# org.gradle.caching=true\n"), None);

        assert!(
            rows.iter()
                .find(|r| r.setting.key == "org.gradle.caching")
                .unwrap()
                .is_missing()
        );
    }

    #[test]
    fn the_optional_setting_is_added_only_when_asked_for() {
        let rows = review(None, None);

        let plain = additions(&rows, false);
        let all = additions(&rows, true);

        assert!(!keys(&plain).contains(&"org.gradle.configuration-cache".to_string()));
        assert!(keys(&all).contains(&"org.gradle.configuration-cache".to_string()));
        assert_eq!(plain.len() + 1, all.len());
    }

    #[test]
    fn what_is_already_set_is_never_added() {
        let rows = review(
            Some("org.gradle.jvmargs=-Xmx1g\n"),
            Some("org.gradle.caching=false\n"),
        );

        let keys = keys(&additions(&rows, true));

        assert!(!keys.contains(&"org.gradle.jvmargs".to_string()));
        assert!(!keys.contains(&"org.gradle.caching".to_string()));
    }

    #[test]
    fn the_suggested_heaps_are_fixed_numbers_with_no_repo_data_in_them() {
        for setting in SETTINGS {
            assert!(
                !setting.value.contains(['\n', '\r', '\\']),
                "{}",
                setting.key
            );
        }
    }

    // --- writing ---

    fn user_file(tmp: &tempfile::TempDir) -> PathBuf {
        tmp.path().join(".gradle/gradle.properties")
    }

    #[test]
    fn a_missing_file_is_created_with_the_lines_under_a_header() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = user_file(&tmp);

        let applied = apply(&path, &[("a.b", "1"), ("c.d", "two words")]).unwrap();

        assert_eq!(applied.added, ["a.b", "c.d"]);
        assert_eq!(applied.backup, None);
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "# Added by `lf-mide tune`\na.b=1\nc.d=two words\n"
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o644
        );
    }

    #[test]
    fn an_existing_file_is_kept_byte_for_byte_with_the_new_lines_after_it() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = user_file(&tmp);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "mine=1\n# note").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();

        let applied = apply(&path, &[("a.b", "1")]).unwrap();

        let text = fs::read_to_string(&path).unwrap();
        assert!(
            text.starts_with("mine=1\n# note\n\n# Added by `lf-mide tune`\n"),
            "{text:?}"
        );
        assert!(text.ends_with("a.b=1\n"));
        let backup = applied.backup.unwrap();
        assert_eq!(fs::read_to_string(backup).unwrap(), "mine=1\n# note");
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600,
            "mode kept"
        );
    }

    #[test]
    fn the_backup_is_as_private_as_the_original() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = user_file(&tmp);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "signing.password=hunter2\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();

        let applied = apply(&path, &[("a.b", "1")]).unwrap();

        let backup = applied.backup.unwrap();
        assert_eq!(
            fs::metadata(backup).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn a_temp_file_name_that_is_already_taken_is_not_ours_to_remove() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = user_file(&tmp);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let taken = path.with_file_name(format!(
            "gradle.properties.lf-mide-tmp-{}",
            std::process::id()
        ));
        fs::write(&taken, "somebody else's file").unwrap();

        let result = apply(&path, &[("a.b", "1")]);

        assert!(result.is_err());
        assert_eq!(fs::read_to_string(&taken).unwrap(), "somebody else's file");
        assert!(!path.exists());
    }

    #[test]
    fn a_symlinked_file_can_be_read_for_review_but_not_written() {
        let tmp = tempfile::TempDir::new().unwrap();
        let real = tmp.path().join("dot/gradle.properties");
        fs::create_dir_all(real.parent().unwrap()).unwrap();
        fs::write(&real, "org.gradle.caching=true\n").unwrap();
        let link = tmp.path().join("gradle.properties");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        assert_eq!(
            read_for_review(&link).unwrap().as_deref(),
            Some("org.gradle.caching=true\n")
        );
        assert!(matches!(read_properties(&link), Err(TuneError::Symlink(_))));
        assert!(matches!(
            apply(&link, &[("a", "1")]),
            Err(TuneError::Symlink(_))
        ));
    }

    #[test]
    fn the_first_backup_is_the_one_that_stays() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = user_file(&tmp);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "original=1\n").unwrap();

        apply(&path, &[("a.b", "1")]).unwrap();
        let second = apply(&path, &[("c.d", "2")]).unwrap();

        assert_eq!(second.backup, None);
        let backup = path.with_file_name("gradle.properties.lf-mide-backup");
        assert_eq!(fs::read_to_string(backup).unwrap(), "original=1\n");
        assert!(fs::read_to_string(&path).unwrap().contains("c.d=2"));
    }

    #[test]
    fn nothing_to_add_touches_nothing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = user_file(&tmp);

        let applied = apply(&path, &[]).unwrap();

        assert!(applied.added.is_empty());
        assert!(!path.exists() && !path.parent().unwrap().exists());
    }

    #[test]
    fn a_symlinked_file_is_never_written_through() {
        let tmp = tempfile::TempDir::new().unwrap();
        let real = tmp.path().join("dotfiles/gradle.properties");
        fs::create_dir_all(real.parent().unwrap()).unwrap();
        fs::write(&real, "keep=1\n").unwrap();
        let path = user_file(&tmp);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&real, &path).unwrap();

        let result = apply(&path, &[("a.b", "1")]);

        assert!(matches!(result, Err(TuneError::Symlink(_))));
        assert_eq!(fs::read_to_string(&real).unwrap(), "keep=1\n");
    }

    #[test]
    fn huge_or_binary_files_are_not_touched() {
        let tmp = tempfile::TempDir::new().unwrap();
        let big = tmp.path().join("big.properties");
        fs::write(&big, vec![b'a'; (MAX_FILE_BYTES + 1) as usize]).unwrap();
        let binary = tmp.path().join("bin.properties");
        fs::write(&binary, [0xff, 0xfe, 0x00, 0x80]).unwrap();

        assert!(matches!(
            apply(&big, &[("a", "1")]),
            Err(TuneError::TooLarge(_))
        ));
        assert!(matches!(
            apply(&binary, &[("a", "1")]),
            Err(TuneError::NotText(_))
        ));
        assert_eq!(fs::read(&binary).unwrap(), [0xff, 0xfe, 0x00, 0x80]);
    }

    #[test]
    fn no_temporary_file_is_left_behind() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = user_file(&tmp);

        apply(&path, &[("a.b", "1")]).unwrap();

        let names: Vec<String> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["gradle.properties"]);
    }

    #[test]
    fn running_review_and_apply_twice_adds_each_line_once() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = user_file(&tmp);

        for _ in 0..2 {
            let text = read_properties(&path).unwrap();
            let rows = review(text.as_deref(), None);
            apply(&path, &additions(&rows, false)).unwrap();
        }

        let text = fs::read_to_string(&path).unwrap();
        assert_eq!(text.matches("org.gradle.jvmargs=").count(), 1);
        assert_eq!(text.matches("lf-mide tune").count(), 1);
    }
}
