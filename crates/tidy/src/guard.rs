//! The only way a path gets deleted. Every other part of `clean` merely
//! proposes; this decides, and it is strict.
//!
//! A path may be deleted only if, at the moment of deletion, it is a real
//! file or directory (not a symbolic link) that lies strictly inside a base
//! folder, and that base folder is itself inside a root the command is
//! allowed to touch. Anything else is refused with a reason.

use std::collections::VecDeque;
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

/// Why a path was not deleted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    NotAbsolute,
    Missing,
    Symlink,
    OutsideBase,
    IsTheBase,
    BaseOutsideRoots,
    BaseIsLink,
    OtherVolume,
    TooBigToCheck,
    Failed(String),
}

impl Refusal {
    pub fn text(&self) -> String {
        match self {
            Refusal::NotAbsolute => "not an absolute path".to_string(),
            Refusal::Missing => "already gone".to_string(),
            Refusal::Symlink => "is a symbolic link: left alone".to_string(),
            Refusal::OutsideBase => "is not inside the folder it belongs to".to_string(),
            Refusal::IsTheBase => "is the folder itself, not something inside it".to_string(),
            Refusal::BaseOutsideRoots => "belongs to a folder LF-Mide may not clean".to_string(),
            Refusal::BaseIsLink => {
                "belongs to a folder that is a symbolic link: left alone".to_string()
            }
            Refusal::OtherVolume => "contains another disk or volume: left alone".to_string(),
            Refusal::TooBigToCheck => "is too big to check for other disks: left alone".to_string(),
            Refusal::Failed(why) => format!("could not be removed: {why}"),
        }
    }
}

/// Folders `clean` may delete from.
#[derive(Debug, Clone)]
pub struct Roots {
    allowed: Vec<PathBuf>,
}

impl Roots {
    /// `allowed` are resolved now; ones that do not exist are dropped.
    pub fn new(allowed: &[&Path]) -> Self {
        let mut resolved: Vec<PathBuf> = allowed
            .iter()
            .filter_map(|path| fs::canonicalize(path).ok())
            .filter(|path| path.parent().is_some()) // never `/`
            .collect();
        resolved.sort();
        resolved.dedup();
        Self { allowed: resolved }
    }

    fn contains(&self, path: &Path) -> bool {
        self.allowed.iter().any(|root| path.starts_with(root))
    }
}

/// Checks `target` against `base` and `roots` and deletes it if all is well.
/// The checks are repeated here even if the caller made them when planning:
/// the disk may have changed since.
pub fn delete(target: &Path, base: &Path, roots: &Roots) -> Result<(), Refusal> {
    if !target.is_absolute() || !base.is_absolute() {
        return Err(Refusal::NotAbsolute);
    }
    if target
        .components()
        .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err(Refusal::OutsideBase);
    }
    let meta = match fs::symlink_metadata(target) {
        Ok(meta) => meta,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Err(Refusal::Missing),
        Err(err) => return Err(Refusal::Failed(err.to_string())),
    };
    if meta.file_type().is_symlink() {
        return Err(Refusal::Symlink);
    }
    // A base that is itself a link (a DerivedData moved to another disk, say)
    // could lead anywhere: it is not the folder the roots vouch for.
    match fs::symlink_metadata(base) {
        Ok(m) if m.file_type().is_symlink() => return Err(Refusal::BaseIsLink),
        Ok(_) => {}
        Err(_) => return Err(Refusal::Missing),
    }
    let base = fs::canonicalize(base).map_err(|_| Refusal::Missing)?;
    if !roots.contains(&base) || base.parent().is_none() {
        return Err(Refusal::BaseOutsideRoots);
    }
    // Resolve the parent, not the target: the target itself is known not to
    // be a link, and its parent chain is what could lead somewhere else.
    let parent = target
        .parent()
        .and_then(|parent| fs::canonicalize(parent).ok())
        .ok_or(Refusal::Missing)?;
    let name = target.file_name().ok_or(Refusal::IsTheBase)?;
    let resolved = parent.join(name);
    if resolved == base {
        return Err(Refusal::IsTheBase);
    }
    if !resolved.starts_with(&base) {
        return Err(Refusal::OutsideBase);
    }
    if meta.is_dir() {
        match volume_check(&resolved, meta.dev()) {
            VolumeCheck::SameVolume => {}
            VolumeCheck::OtherVolume => return Err(Refusal::OtherVolume),
            VolumeCheck::TooBig => return Err(Refusal::TooBigToCheck),
        }
    }
    let result = if meta.is_dir() {
        fs::remove_dir_all(&resolved)
    } else {
        fs::remove_file(&resolved)
    };
    result.map_err(|err| Refusal::Failed(err.to_string()))
}

/// Folders visited when looking for another volume.
const MAX_VOLUME_CHECK_ENTRIES: usize = 2_000_000;

/// What looking for another disk inside a tree found.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum VolumeCheck {
    SameVolume,
    OtherVolume,
    TooBig,
}

/// Whether every folder under `path` is on the same device as `device`.
/// A disk mounted inside the tree would otherwise be emptied too; a tree too
/// big to check is refused for its own reason.
pub(crate) fn volume_check(path: &Path, device: u64) -> VolumeCheck {
    let mut queue = VecDeque::from([path.to_path_buf()]);
    let mut visited = 0usize;
    while let Some(dir) = queue.pop_front() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            visited += 1;
            if visited > MAX_VOLUME_CHECK_ENTRIES {
                return VolumeCheck::TooBig;
            }
            let Ok(meta) = fs::symlink_metadata(entry.path()) else {
                continue;
            };
            if !meta.is_dir() {
                continue;
            }
            if meta.dev() != device {
                return VolumeCheck::OtherVolume;
            }
            queue.push_back(entry.path());
        }
    }
    VolumeCheck::SameVolume
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    struct Fixture {
        _tmp: tempfile::TempDir,
        home: PathBuf,
        base: PathBuf,
        roots: Roots,
    }

    fn fixture() -> Fixture {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = fs::canonicalize(tmp.path()).unwrap();
        let base = home.join("cache");
        fs::create_dir_all(base.join("keep-out")).unwrap();
        let roots = Roots::new(&[&home]);
        Fixture {
            _tmp: tmp,
            home,
            base,
            roots,
        }
    }

    #[test]
    fn something_inside_the_base_is_deleted_whole() {
        let f = fixture();
        fs::create_dir_all(f.base.join("dd/deep")).unwrap();
        fs::write(f.base.join("dd/deep/x.bin"), b"x").unwrap();

        assert_eq!(delete(&f.base.join("dd"), &f.base, &f.roots), Ok(()));

        assert!(!f.base.join("dd").exists());
        assert!(f.base.exists(), "the base stays");
    }

    #[test]
    fn a_single_file_can_be_deleted_too() {
        let f = fixture();
        fs::write(f.base.join("old.log"), b"x").unwrap();

        assert_eq!(delete(&f.base.join("old.log"), &f.base, &f.roots), Ok(()));
        assert!(!f.base.join("old.log").exists());
    }

    #[test]
    fn the_base_itself_is_never_deleted() {
        let f = fixture();

        assert_eq!(delete(&f.base, &f.base, &f.roots), Err(Refusal::IsTheBase));
        assert!(f.base.exists());
    }

    #[test]
    fn a_symlink_is_left_alone_and_what_it_points_to_is_untouched() {
        let f = fixture();
        let outside = tempfile::TempDir::new().unwrap();
        fs::write(outside.path().join("precious.txt"), b"keep").unwrap();
        symlink(outside.path(), f.base.join("link")).unwrap();

        assert_eq!(
            delete(&f.base.join("link"), &f.base, &f.roots),
            Err(Refusal::Symlink)
        );

        assert!(outside.path().join("precious.txt").exists());
        assert!(fs::symlink_metadata(f.base.join("link")).is_ok());
    }

    #[test]
    fn a_symlink_inside_a_deleted_folder_is_removed_without_following_it() {
        let f = fixture();
        let outside = tempfile::TempDir::new().unwrap();
        fs::write(outside.path().join("precious.txt"), b"keep").unwrap();
        fs::create_dir_all(f.base.join("dd")).unwrap();
        symlink(outside.path(), f.base.join("dd/link")).unwrap();

        assert_eq!(delete(&f.base.join("dd"), &f.base, &f.roots), Ok(()));

        assert!(outside.path().join("precious.txt").exists());
    }

    #[test]
    fn a_target_reached_through_a_symlinked_parent_is_refused() {
        let f = fixture();
        let outside = tempfile::TempDir::new().unwrap();
        fs::create_dir_all(outside.path().join("victim")).unwrap();
        symlink(outside.path(), f.base.join("gate")).unwrap();

        let result = delete(&f.base.join("gate/victim"), &f.base, &f.roots);

        assert_eq!(result, Err(Refusal::OutsideBase));
        assert!(outside.path().join("victim").exists());
    }

    #[test]
    fn paths_that_climb_out_are_refused() {
        let f = fixture();
        fs::write(f.home.join("secret.txt"), b"keep").unwrap();

        let sneaky = f.base.join("../secret.txt");

        assert_eq!(
            delete(&sneaky, &f.base, &f.roots),
            Err(Refusal::OutsideBase)
        );
        assert!(f.home.join("secret.txt").exists());
    }

    #[test]
    fn a_sibling_that_merely_shares_the_prefix_is_outside() {
        let f = fixture();
        let sibling = f.home.join("cache-other");
        fs::create_dir_all(&sibling).unwrap();

        assert_eq!(
            delete(&sibling, &f.base, &f.roots),
            Err(Refusal::OutsideBase)
        );
        assert!(sibling.exists());
    }

    #[test]
    fn a_base_outside_the_allowed_roots_is_refused() {
        let f = fixture();
        let elsewhere = tempfile::TempDir::new().unwrap();
        fs::create_dir_all(elsewhere.path().join("x")).unwrap();

        let result = delete(&elsewhere.path().join("x"), elsewhere.path(), &f.roots);

        assert_eq!(result, Err(Refusal::BaseOutsideRoots));
        assert!(elsewhere.path().join("x").exists());
    }

    #[test]
    fn the_filesystem_root_is_never_an_allowed_root() {
        let tmp = tempfile::TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join("x")).unwrap();
        let roots = Roots::new(&[Path::new("/")]);

        let result = delete(&tmp.path().join("x"), tmp.path(), &roots);

        assert_eq!(result, Err(Refusal::BaseOutsideRoots));
        assert!(tmp.path().join("x").exists());
    }

    #[test]
    fn relative_paths_and_missing_ones_are_refused_without_effect() {
        let f = fixture();

        assert_eq!(
            delete(Path::new("relative"), &f.base, &f.roots),
            Err(Refusal::NotAbsolute)
        );
        assert_eq!(
            delete(&f.base.join("nope"), &f.base, &f.roots),
            Err(Refusal::Missing)
        );
    }

    #[test]
    fn a_base_that_is_a_symlink_is_refused() {
        let f = fixture();
        let real = f.home.join("real-dd");
        fs::create_dir_all(real.join("App")).unwrap();
        symlink(&real, f.home.join("dd-link")).unwrap();

        let result = delete(
            &f.home.join("dd-link/App"),
            &f.home.join("dd-link"),
            &f.roots,
        );

        assert_eq!(result, Err(Refusal::BaseIsLink));
        assert!(real.join("App").exists());
    }

    #[test]
    fn an_ordinary_tree_stays_on_one_volume_and_the_filesystem_root_does_not() {
        let f = fixture();
        fs::create_dir_all(f.base.join("a/b/c")).unwrap();
        let device = fs::metadata(&f.base).unwrap().dev();

        assert_eq!(volume_check(&f.base, device), VolumeCheck::SameVolume);
        let root_device = fs::metadata("/").unwrap().dev();
        assert_eq!(
            volume_check(Path::new("/"), root_device),
            VolumeCheck::OtherVolume,
            "/dev and others are mounts"
        );
    }

    #[test]
    fn refusals_have_readable_text() {
        for refusal in [
            Refusal::NotAbsolute,
            Refusal::Missing,
            Refusal::Symlink,
            Refusal::OutsideBase,
            Refusal::IsTheBase,
            Refusal::BaseOutsideRoots,
            Refusal::BaseIsLink,
            Refusal::OtherVolume,
            Refusal::TooBigToCheck,
            Refusal::Failed("denied".into()),
        ] {
            assert!(!refusal.text().is_empty());
        }
    }
}
