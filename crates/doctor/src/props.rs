//! Minimal reader for Java `.properties` files (`gradle.properties`,
//! `local.properties`, `gradle-wrapper.properties`).

/// Value of the first `key=value` line for `key`. Skips `#`/`!` comments and
/// un-escapes `\:` and `\=`.
pub fn get(text: &str, key: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('#') && !line.starts_with('!'))
        .find_map(|line| {
            let (name, value) = line.split_once('=')?;
            (name.trim() == key).then(|| value.trim().replace("\\:", ":").replace("\\=", "="))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_value_and_trims_whitespace() {
        let text = "a=1\n  sdk.dir = /Users/x/sdk  \nb=2\n";

        assert_eq!(get(text, "sdk.dir"), Some("/Users/x/sdk".into()));
        assert_eq!(get(text, "a"), Some("1".into()));
    }

    #[test]
    fn ignores_comments_and_missing_keys() {
        let text = "# sdk.dir=/commented\n! also=comment\nother=1\n";

        assert_eq!(get(text, "sdk.dir"), None);
        assert_eq!(get(text, "also"), None);
    }

    #[test]
    fn unescapes_colons_and_keeps_equals_in_values() {
        let text = "distributionUrl=https\\://services.gradle.org/gradle-9.5.1-bin.zip\nargs=-Xmx4g -Dx=y\n";

        assert_eq!(
            get(text, "distributionUrl"),
            Some("https://services.gradle.org/gradle-9.5.1-bin.zip".into())
        );
        assert_eq!(get(text, "args"), Some("-Xmx4g -Dx=y".into()));
    }

    #[test]
    fn does_not_match_key_prefixes() {
        assert_eq!(get("sdk.dir.extra=1\n", "sdk.dir"), None);
    }
}
