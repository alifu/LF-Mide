//! Fuzzy file-name matching for the finder.

use std::path::PathBuf;

use lf_mide_core::FileMatch;
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

/// The best `limit` files for `query`, best first. Words in the query
/// (split on spaces) must all match; matching is case-insensitive unless the
/// query contains an uppercase letter. An empty query returns the first
/// `limit` files unchanged.
pub fn rank(files: &[PathBuf], query: &str, limit: usize) -> Vec<FileMatch> {
    if limit == 0 {
        return Vec::new();
    }
    if query.trim().is_empty() {
        return files
            .iter()
            .take(limit)
            .map(|path| FileMatch {
                path: path.clone(),
                score: 0,
                indices: Vec::new(),
            })
            .collect();
    }

    let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
    let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
    let mut buffer = Vec::new();

    let mut scored: Vec<(u32, &PathBuf)> = files
        .iter()
        .filter_map(|path| {
            let text = path.to_string_lossy();
            let score = pattern.score(Utf32Str::new(&text, &mut buffer), &mut matcher)?;
            Some((score, path))
        })
        .collect();
    // Best first: higher score, then shorter path, then path order (a total
    // order, so the result is the same however the list was partitioned).
    let best_first = |a: &(u32, &PathBuf), b: &(u32, &PathBuf)| {
        b.0.cmp(&a.0)
            .then(a.1.as_os_str().len().cmp(&b.1.as_os_str().len()))
            .then(a.1.cmp(b.1))
    };
    if scored.len() > limit {
        // Only the best `limit` need ordering: partition, then sort those.
        scored.select_nth_unstable_by(limit - 1, best_first);
        scored.truncate(limit);
    }
    scored.sort_by(best_first);

    // Highlight positions are only worth computing for what is shown.
    scored
        .into_iter()
        .map(|(score, path)| {
            let text = path.to_string_lossy();
            let mut indices = Vec::new();
            pattern.indices(
                Utf32Str::new(&text, &mut buffer),
                &mut matcher,
                &mut indices,
            );
            indices.sort_unstable();
            indices.dedup();
            FileMatch {
                path: path.clone(),
                score,
                indices,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(paths: &[&str]) -> Vec<PathBuf> {
        paths.iter().map(PathBuf::from).collect()
    }

    fn top(matches: &[FileMatch]) -> Vec<String> {
        matches
            .iter()
            .map(|m| m.path.display().to_string())
            .collect()
    }

    const PROJECT: &[&str] = &[
        "gradle/libs.versions.toml",
        "README.md",
        "shared/src/commonMain/kotlin/feature/catalog/ItemDetailScreen.kt",
        "shared/src/commonMain/kotlin/feature/catalog/ItemDetailViewModel.kt",
        "shared/src/commonMain/kotlin/feature/catalog/ItemListScreen.kt",
        "iosApp/iosApp/ContentView.swift",
    ];

    #[test]
    fn an_empty_query_returns_the_first_files_in_order() {
        let list = files(PROJECT);

        let matches = rank(&list, "", 3);
        let blank = rank(&list, "   ", 2);

        assert_eq!(top(&matches), vec![PROJECT[0], PROJECT[1], PROJECT[2]]);
        assert!(matches.iter().all(|m| m.indices.is_empty() && m.score == 0));
        assert_eq!(blank.len(), 2);
    }

    #[test]
    fn matches_scattered_characters_and_ignores_files_without_them() {
        let matches = rank(&files(PROJECT), "itemdet", 10);

        let names = top(&matches);
        assert_eq!(names.len(), 2, "{names:?}");
        assert!(names.iter().all(|n| n.contains("ItemDetail")));
    }

    #[test]
    fn an_exact_file_name_beats_a_loose_match() {
        let list = files(&["docs/read/me/notes.md", "README.md", "src/rea/dme.txt"]);

        let matches = rank(&list, "readme", 10);

        assert_eq!(matches[0].path, PathBuf::from("README.md"));
    }

    #[test]
    fn every_word_of_the_query_must_match() {
        let matches = rank(&files(PROJECT), "item view", 10);

        assert_eq!(
            top(&matches),
            vec!["shared/src/commonMain/kotlin/feature/catalog/ItemDetailViewModel.kt"]
        );
    }

    #[test]
    fn lowercase_ignores_case_but_an_uppercase_letter_is_exact() {
        let list = files(&["src/App.kt", "src/app.kt"]);

        let lower = rank(&list, "app", 10);
        let upper = rank(&list, "App", 10);

        assert_eq!(lower.len(), 2);
        assert_eq!(top(&upper), vec!["src/App.kt"]);
    }

    #[test]
    fn respects_the_limit_and_returns_nothing_when_nothing_matches() {
        let list = files(PROJECT);

        assert_eq!(rank(&list, "kt", 1).len(), 1);
        assert!(rank(&list, "zzzqqq", 10).is_empty());
        assert!(rank(&list, "kt", 0).is_empty());
        assert!(rank(&[], "kt", 10).is_empty());
    }

    #[test]
    fn reports_which_characters_matched_for_highlighting() {
        let matches = rank(&files(&["a/Item.kt"]), "ikt", 5);

        let m = &matches[0];
        let chars: Vec<char> = "a/Item.kt".chars().collect();
        assert_eq!(m.indices.len(), 3);
        assert!(
            m.indices.windows(2).all(|w| w[0] < w[1]),
            "sorted and unique: {:?}",
            m.indices
        );
        let picked: String = m
            .indices
            .iter()
            .map(|i| chars[*i as usize].to_ascii_lowercase())
            .collect();
        assert_eq!(picked, "ikt");
        assert!(m.score > 0);
    }

    #[test]
    fn results_are_ordered_by_score_then_shorter_path() {
        let list = files(&["x/long/dir/App.kt", "App.kt", "y/App.kt"]);

        let matches = rank(&list, "app", 10);

        assert_eq!(
            top(&matches),
            vec!["App.kt", "y/App.kt", "x/long/dir/App.kt"]
        );
        assert!(matches.windows(2).all(|w| w[0].score >= w[1].score));
    }

    #[test]
    fn a_small_limit_returns_exactly_the_prefix_of_the_full_ranking() {
        // Guards the partial-sort shortcut: top-N must equal the first N of
        // the complete, fully sorted ranking, ties included.
        let list: Vec<PathBuf> = (0..400)
            .map(|i| PathBuf::from(format!("mod{}/feature{}/Screen{}View.kt", i % 7, i % 13, i)))
            .collect();

        let full = rank(&list, "view", 1000);
        for limit in [1, 5, 37, 399] {
            let top = rank(&list, "view", limit);

            assert_eq!(top, full[..limit].to_vec(), "limit {limit}");
        }
        assert_eq!(full.len(), 400);
    }

    #[test]
    fn non_utf8_paths_do_not_panic() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let list = vec![PathBuf::from(OsStr::from_bytes(b"caf\xe9/menu.kt"))];

        let matches = rank(&list, "menu", 5);

        assert_eq!(matches.len(), 1);
    }
}
