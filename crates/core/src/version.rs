//! Extracting and comparing tool versions from free-form command output.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl Version {
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// Finds the first `N.N` or `N.N.N` in `text` (`NVIM v0.12.6` → 0.12.6,
    /// `Xcode 26.5` → 26.5.0). Returns `None` when there is no dotted number.
    pub fn find_in(text: &str) -> Option<Version> {
        text.split(|c: char| !(c.is_ascii_digit() || c == '.'))
            .find_map(Self::parse_dotted)
    }

    /// Parses a token made only of digits and dots; needs at least `N.N`.
    fn parse_dotted(token: &str) -> Option<Version> {
        let mut parts = token.trim_matches('.').split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        let patch = parts.next().map_or(Some(0), |p| p.parse().ok())?;
        Some(Version {
            major,
            minor,
            patch,
        })
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_versions_in_real_tool_output() {
        let cases = [
            ("NVIM v0.12.6\nBuild type: Release", Version::new(0, 12, 6)),
            ("Xcode 26.5\nBuild version 17F42", Version::new(26, 5, 0)),
            (
                "Android Debug Bridge version 1.0.41",
                Version::new(1, 0, 41),
            ),
            (
                "openjdk version \"21.0.12\" 2026-01-20",
                Version::new(21, 0, 12),
            ),
            ("ripgrep 15.2.0 (rev abc)", Version::new(15, 2, 0)),
            ("git version 2.50.1 (Apple Git-155)", Version::new(2, 50, 1)),
        ];

        for (text, expected) in cases {
            assert_eq!(Version::find_in(text), Some(expected), "for {text:?}");
        }
    }

    #[test]
    fn returns_none_without_a_dotted_number() {
        assert_eq!(Version::find_in("no version here 12 and 7"), None);
        assert_eq!(Version::find_in(""), None);
    }

    #[test]
    fn ignores_trailing_dot_and_orders_numerically() {
        assert_eq!(Version::find_in("v1.2."), Some(Version::new(1, 2, 0)));
        assert!(Version::new(0, 12, 0) > Version::new(0, 9, 5));
        assert!(Version::new(17, 0, 16) < Version::new(21, 0, 0));
    }

    #[test]
    fn displays_as_dotted_triple() {
        assert_eq!(Version::new(0, 12, 6).to_string(), "0.12.6");
    }
}
