//! Check framework: pure results in, rendered report and exit code out.
#![allow(dead_code)] // constructors/variants are exercised from step 3 on

#[derive(Debug, PartialEq, Eq)]
pub enum Status {
    Pass,
    Warn(String),
    Fail(String),
}

#[derive(Debug, PartialEq, Eq)]
pub struct CheckResult {
    pub name: &'static str,
    pub status: Status,
}

impl CheckResult {
    pub fn pass(name: &'static str) -> Self {
        Self { name, status: Status::Pass }
    }

    pub fn warn(name: &'static str, why: impl Into<String>) -> Self {
        Self { name, status: Status::Warn(why.into()) }
    }

    pub fn fail(name: &'static str, why: impl Into<String>) -> Self {
        Self { name, status: Status::Fail(why.into()) }
    }

    fn label(&self) -> &'static str {
        match self.status {
            Status::Pass => "PASS",
            Status::Warn(_) => "WARN",
            Status::Fail(_) => "FAIL",
        }
    }

    fn detail(&self) -> Option<&str> {
        match &self.status {
            Status::Pass => None,
            Status::Warn(d) | Status::Fail(d) => Some(d),
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report(pub Vec<CheckResult>);

/// 0 = no failures (warnings allowed), 1 = any failure.
impl Report {
    pub fn exit_code(&self) -> u8 {
        if self.0.iter().any(|c| matches!(c.status, Status::Fail(_))) {
            super::exit::FAIL
        } else {
            super::exit::OK
        }
    }

    pub fn render(&self) -> String {
        let width = self
            .0
            .iter()
            .map(|c| c.name.len())
            .max()
            .unwrap_or_default();

        let mut out = String::new();
        for c in &self.0 {
            out.push_str(c.label());
            out.push_str("  ");
            out.push_str(&format!("{:<width$}", c.name));
            if let Some(d) = c.detail() {
                out.push_str("  ");
                out.push_str(d);
            }
            out.push('\n');
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Report {
        Report(vec![
            CheckResult::pass("quickshell"),
            CheckResult::warn("gum", "not installed (interactive TUI flows only)"),
            CheckResult::fail("hyprctl", "not found in PATH"),
        ])
    }

    #[test]
    fn exit_code_ok_without_failures() {
        let r = Report(vec![
            CheckResult::pass("git"),
            CheckResult::warn("upower", "missing"),
        ]);
        assert_eq!(r.exit_code(), super::super::exit::OK);
    }

    #[test]
    fn exit_code_fail_on_any_failure() {
        assert_eq!(sample().exit_code(), super::super::exit::FAIL);
    }

    #[test]
    fn exit_code_ok_on_empty_report() {
        assert_eq!(Report::default().exit_code(), super::super::exit::OK);
    }

    #[test]
    fn renders_aligned_table() {
        let out = sample().render();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "PASS  quickshell");
        assert_eq!(
            lines[1],
            "WARN  gum         not installed (interactive TUI flows only)"
        );
        assert_eq!(lines[2], "FAIL  hyprctl     not found in PATH");
    }

    #[test]
    fn empty_report_renders_empty_string() {
        assert_eq!(Report::default().render(), "");
    }
}
