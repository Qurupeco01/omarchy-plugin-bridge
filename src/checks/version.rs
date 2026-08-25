//! Semantic version extraction from tool output strings (pure).

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// Extracts the first `major.minor[.patch]` run found anywhere in `input`.
/// Tolerates prefixes ("Quickshell 0.3.1 (revision ...)") and suffixes
/// ("5.3.15(1)-release", "0.4.1-42-gdeadbeef").
pub fn parse(input: &str) -> Option<Version> {
    let mut run = String::new();
    for c in input.chars() {
        if c.is_ascii_digit() || (c == '.' && !run.is_empty()) {
            run.push(c);
        } else if !run.is_empty() {
            if let Some(v) = try_run(&run) {
                return Some(v);
            }
            run.clear();
        }
    }
    try_run(&run)
}

fn try_run(run: &str) -> Option<Version> {
    let parts: Vec<Option<u32>> = run
        .split('.')
        .map(|p| p.parse().ok())
        .take(3)
        .collect();
    match parts.as_slice() {
        [Some(maj), Some(min)] => Some(Version { major: *maj, minor: *min, patch: 0 }),
        [Some(maj), Some(min), Some(pat)] => {
            Some(Version { major: *maj, minor: *min, patch: *pat })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(maj: u32, min: u32, pat: u32) -> Version {
        Version { major: maj, minor: min, patch: pat }
    }

    #[test]
    fn real_tool_outputs() {
        assert_eq!(
            parse("Quickshell 0.3.1 (revision , distributed by Arch Linux)"),
            Some(v(0, 3, 1))
        );
        assert_eq!(parse("git version 2.55.0"), Some(v(2, 55, 0)));
        assert_eq!(
            parse("GNU bash, version 5.3.15(1)-release (x86_64-pc-linux-gnu)"),
            Some(v(5, 3, 15))
        );
        assert_eq!(
            parse("Hyprland 0.56.2 built from branch v0.56.2 at commit efb5099 clean"),
            Some(v(0, 56, 2))
        );
    }

    #[test]
    fn two_component_and_commit_suffix() {
        assert_eq!(parse("0.4"), Some(v(0, 4, 0)));
        assert_eq!(parse("0.4.1-42-gdeadbeef"), Some(v(0, 4, 1)));
    }

    #[test]
    fn garbage_yields_none() {
        assert_eq!(parse("no version here"), None);
        assert_eq!(parse(""), None);
        assert_eq!(parse("..."), None);
    }

    #[test]
    fn ordering_is_semantic() {
        assert!(v(0, 10, 0) > v(0, 3, 99));
        assert!(v(1, 0, 0) > v(0, 99, 99));
    }
}
