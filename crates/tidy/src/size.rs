//! How much disk something uses, counted like `du`: space actually
//! allocated, symbolic links not followed.

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

/// Directory entries visited before giving up on a tree (a runaway or
/// hostile tree must not hang the command).
const MAX_ENTRIES: u64 = 5_000_000;
const BLOCK: u64 = 512;

/// Allocated bytes under `path` (the file itself, or the whole tree). A
/// symlink counts for the link only. Unreadable parts count as zero.
pub fn disk_use(path: &Path) -> u64 {
    let mut visited = 0;
    walk(path, &mut visited)
}

fn walk(path: &Path, visited: &mut u64) -> u64 {
    *visited += 1;
    if *visited > MAX_ENTRIES {
        return 0;
    }
    let Ok(meta) = fs::symlink_metadata(path) else {
        return 0;
    };
    let own = meta.blocks().saturating_mul(BLOCK);
    if !meta.is_dir() {
        return own;
    }
    let Ok(entries) = fs::read_dir(path) else {
        return own;
    };
    entries.flatten().fold(own, |sum, entry| {
        sum.saturating_add(walk(&entry.path(), visited))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_counts_its_allocated_space_and_a_missing_path_counts_nothing() {
        let tmp = tempfile::TempDir::new().unwrap();
        fs::write(tmp.path().join("a.bin"), vec![1u8; 100_000]).unwrap();

        let used = disk_use(&tmp.path().join("a.bin"));

        assert!((100_000..200_000).contains(&used), "{used}");
        assert_eq!(disk_use(&tmp.path().join("nope")), 0);
    }

    #[test]
    fn a_tree_adds_up_its_files() {
        let tmp = tempfile::TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join("a/b")).unwrap();
        fs::write(tmp.path().join("a/one.bin"), vec![1u8; 50_000]).unwrap();
        fs::write(tmp.path().join("a/b/two.bin"), vec![1u8; 70_000]).unwrap();

        assert!(disk_use(tmp.path()) >= 120_000);
    }

    #[test]
    fn symbolic_links_are_not_followed() {
        let outside = tempfile::TempDir::new().unwrap();
        fs::write(outside.path().join("big.bin"), vec![1u8; 2_000_000]).unwrap();
        let tmp = tempfile::TempDir::new().unwrap();
        std::os::unix::fs::symlink(outside.path(), tmp.path().join("link")).unwrap();

        assert!(
            disk_use(tmp.path()) < 100_000,
            "the linked folder must not count"
        );
    }

    #[test]
    fn a_sparse_file_counts_what_is_allocated_not_its_length() {
        let tmp = tempfile::TempDir::new().unwrap();
        let file = fs::File::create(tmp.path().join("sparse.img")).unwrap();
        file.set_len(5_000_000_000).unwrap();

        assert!(disk_use(tmp.path()) < 1_000_000);
    }
}
