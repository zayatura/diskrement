//! The `[y/N]` burn confirmation (`Prompt.scala`).

use std::io::Write as _;

/// Ask, then read one line. `assume_yes` skips the read entirely.
pub fn confirm(message: &str, assume_yes: bool) -> bool {
    confirm_from(message, assume_yes, || {
        let mut line = String::new();
        match std::io::stdin().read_line(&mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(line),
        }
    })
}

/// `confirm` with the read injected.
///
/// Anything but `y`/`yes` (case-insensitive, trimmed) refuses, and so does EOF
/// or a read error — an unattended run must never burn by accident.
pub fn confirm_from(
    message: &str,
    assume_yes: bool,
    read_line: impl FnOnce() -> Option<String>,
) -> bool {
    if assume_yes {
        return true;
    }
    print!("{message} [y/N] ");
    let _ = std::io::stdout().flush();
    match read_line() {
        Some(line) => {
            let t = line.trim().to_lowercase();
            t == "y" || t == "yes"
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assume_yes_never_reads() {
        assert!(confirm_from("Burn?", true, || panic!("must not read")));
    }

    #[test]
    fn only_y_and_yes_mean_yes() {
        for answer in ["y", "Y", "yes", "YES", " yes \n"] {
            assert!(
                confirm_from("Burn?", false, || Some(answer.to_string())),
                "{answer:?} should confirm"
            );
        }
        for answer in ["n", "no", "", "sure", "yep"] {
            assert!(
                !confirm_from("Burn?", false, || Some(answer.to_string())),
                "{answer:?} should refuse"
            );
        }
    }

    /// EOF or a read error is "no": an unattended run must never burn.
    #[test]
    fn eof_is_a_refusal() {
        assert!(!confirm_from("Burn?", false, || None));
    }
}
