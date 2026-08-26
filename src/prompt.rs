//! TTY yes/no prompts — one implementation, both defaults.

/// Read a yes/no answer from stdin. Empty or unrecognized input takes the
/// default. Only call from a TTY.
pub fn confirm(question: &str, default_yes: bool) -> bool {
    use std::io::Write;
    let hint = if default_yes { "[Y/n]" } else { "[y/N]" };
    print!("{question} {hint} ");
    std::io::stdout().flush().ok();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).ok();
    match line.trim().to_ascii_lowercase().as_str() {
        "y" | "yes" => true,
        "n" | "no" => false,
        _ => default_yes,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn non_tty_stdin_takes_the_default() {
        // Under cargo test stdin is typically empty/closed → read_line yields "".
        assert!(super::confirm("ok?", true));
        assert!(!super::confirm("ok?", false));
    }
}
