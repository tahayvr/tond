// auth/password_policy.rs — the one password policy (setup, change, reset)
//
// Length-based, as NIST SP 800-63B recommends: at least 12 characters, at
// most 256, no composition rules ("one uppercase, one symbol…"), and a check
// against a compiled-in list of common / breached passwords. The list is
// bundled at build time (common_passwords.txt); no network call is made.
//
// A password is refused when, lowercased, it
//   * is on the list,
//   * is a listed word wrapped in digits and symbols ("Password123!",
//     "!!monkey2024"),
//   * repeats one short chunk ("aaaaaaaaaaaa", "abcabcabcabc"), or
//   * is a straight run of characters ("123456789012", "abcdefghijkl").

use std::collections::HashSet;
use std::sync::OnceLock;

/// Minimum length in characters.
pub const MIN_PASSWORD_CHARS: usize = 12;
/// Maximum length in characters (argon2 cost stays bounded).
pub const MAX_PASSWORD_CHARS: usize = 256;

static LIST_SOURCE: &str = include_str!("common_passwords.txt");

fn common_passwords() -> &'static HashSet<&'static str> {
    static SET: OnceLock<HashSet<&'static str>> = OnceLock::new();
    SET.get_or_init(|| {
        LIST_SOURCE
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .collect()
    })
}

/// Why a password was refused. `Display` is a friendly, actionable message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasswordProblem {
    TooShort,
    TooLong,
    Common,
}

impl std::fmt::Display for PasswordProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PasswordProblem::TooShort => write!(
                f,
                "Use at least {MIN_PASSWORD_CHARS} characters. A few random words make a strong password that's easy to remember."
            ),
            PasswordProblem::TooLong => {
                write!(f, "Use at most {MAX_PASSWORD_CHARS} characters.")
            }
            PasswordProblem::Common => write!(
                f,
                "This password is too common: it's on lists of breached passwords that attackers try first. Pick something unique, like a few random words."
            ),
        }
    }
}

/// Check a new password against the policy.
pub fn check_password(password: &str) -> Result<(), PasswordProblem> {
    let len = password.chars().count();
    if len < MIN_PASSWORD_CHARS {
        return Err(PasswordProblem::TooShort);
    }
    if len > MAX_PASSWORD_CHARS {
        return Err(PasswordProblem::TooLong);
    }
    let lower = password.to_lowercase();
    let list = common_passwords();
    let core = lower.trim_matches(|c: char| {
        c.is_ascii_digit() || c.is_ascii_punctuation() || c.is_whitespace()
    });
    if list.contains(lower.as_str())
        || (!core.is_empty() && list.contains(core))
        || is_repetition(&lower)
        || is_straight_run(&lower)
    {
        return Err(PasswordProblem::Common);
    }
    Ok(())
}

/// Whether the string is one chunk of up to 4 characters repeated.
fn is_repetition(s: &str) -> bool {
    let chars: Vec<char> = s.chars().collect();
    (1..=4).any(|unit| {
        chars.len().is_multiple_of(unit) && chars.chunks(unit).all(|chunk| chunk == &chars[..unit])
    })
}

/// Whether every character follows the previous one by +1 (or all by -1),
/// with digits wrapping 9 → 0.
fn is_straight_run(s: &str) -> bool {
    let chars: Vec<char> = s.chars().collect();
    let step = |a: char, b: char| -> Option<i32> {
        match (a, b) {
            ('9', '0') => Some(1),
            ('0', '9') => Some(-1),
            _ => Some(b as i32 - a as i32),
        }
    };
    let steps: Vec<Option<i32>> = chars.windows(2).map(|w| step(w[0], w[1])).collect();
    steps.iter().all(|s| *s == Some(1)) || steps.iter().all(|s| *s == Some(-1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn length_bounds() {
        assert_eq!(check_password("short"), Err(PasswordProblem::TooShort));
        assert_eq!(
            check_password("elevenchars"),
            Err(PasswordProblem::TooShort),
            "11 characters is one too few"
        );
        assert_eq!(check_password("twelve chars"), Ok(()));
        assert_eq!(
            check_password(&"xy".repeat(129)),
            Err(PasswordProblem::TooLong)
        );
        // Characters, not bytes: 12 emoji are 12 characters.
        assert_eq!(check_password("🦀🌊🔑🎈🚲🌵🍉🎸🦉🧩🛶🪁"), Ok(()));
    }

    #[test]
    fn no_composition_rules() {
        assert_eq!(check_password("purple otter lamp drift"), Ok(()));
        assert_eq!(check_password("alllowercaseletters"), Ok(()));
    }

    #[test]
    fn common_passwords_are_refused_with_or_without_decoration() {
        for pw in [
            "passwordpassword",
            "Password123456",
            "P@ssw0rd2024!!",
            "!!monkey202425",
            "iloveyou1234",
            "qwertyuiop123",
            "correcthorsebatterystaple",
            "Welcome123456",
            "1q2w3e4r5t6y",
        ] {
            assert_eq!(check_password(pw), Err(PasswordProblem::Common), "{pw}");
        }
    }

    #[test]
    fn repetitions_and_runs_are_refused() {
        for pw in [
            "aaaaaaaaaaaa",
            "abcabcabcabc",
            "abababababab",
            "123456789012",
            "abcdefghijklm",
            "zyxwvutsrqpo",
        ] {
            assert_eq!(check_password(pw), Err(PasswordProblem::Common), "{pw}");
        }
    }

    #[test]
    fn messages_are_actionable() {
        assert!(
            PasswordProblem::TooShort
                .to_string()
                .contains("12 characters")
        );
        assert!(PasswordProblem::Common.to_string().contains("breached"));
    }

    #[test]
    fn the_bundled_list_is_loaded() {
        assert!(common_passwords().len() > 300);
        assert!(common_passwords().contains("password"));
        assert!(!common_passwords().iter().any(|l| l.starts_with('#')));
    }
}
