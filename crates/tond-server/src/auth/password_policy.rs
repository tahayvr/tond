// auth/password_policy.rs — the one password policy (setup, change, reset)
//
// Length-based, as NIST SP 800-63B recommends: at least 12 characters, at
// most 256, no composition rules ("one uppercase, one symbol…"), and a check
// against a compiled-in list of common / breached passwords. The list is
// bundled at build time (common_passwords.txt, ~10k entries; sources and
// licence in common_passwords.NOTICE); no network call is made.
//
// The list is kept lowercase and byte-sorted, and looked up by binary search
// directly over the embedded text: no set is built, nothing is allocated,
// and the ~80 KB sit in the binary's read-only data, paged in only when a
// password is checked. Idle RSS is unaffected however long the list grows.
//
// Normalisation (cheap and conventional, matching how the list is stored):
// the password is lowercased, and checked both whole and with leading and
// trailing digits, punctuation and spaces stripped, so "Password123!" hits
// "password". No leetspeak folding: the list already carries the common
// variants ("p@ssw0rd"), and folding would refuse many unique passwords.
//
// A password is refused when, lowercased, it
//   * is on the list,
//   * is a listed word wrapped in digits and symbols ("Password123!",
//     "!!monkey2024"),
//   * repeats one short chunk ("aaaaaaaaaaaa", "abcabcabcabc"), or
//   * is a straight run of characters ("123456789012", "abcdefghijkl").

/// Minimum length in characters.
pub const MIN_PASSWORD_CHARS: usize = 12;
/// Maximum length in characters (argon2 cost stays bounded).
pub const MAX_PASSWORD_CHARS: usize = 256;

static LIST_SOURCE: &str = include_str!("common_passwords.txt");

/// Whether `needle` is a whole line of the sorted, newline-separated `list`.
/// Binary search over byte offsets: pick the middle byte, back up to the start
/// of its line, compare, and halve. No allocation.
fn sorted_lines_contain(list: &str, needle: &str) -> bool {
    let bytes = list.as_bytes();
    let (mut lo, mut hi) = (0usize, bytes.len());
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let start = bytes[lo..mid]
            .iter()
            .rposition(|&b| b == b'\n')
            .map_or(lo, |i| lo + i + 1);
        let end = bytes[start..]
            .iter()
            .position(|&b| b == b'\n')
            .map_or(bytes.len(), |i| start + i);
        match list[start..end].cmp(needle) {
            std::cmp::Ordering::Equal => return true,
            std::cmp::Ordering::Less => lo = end + 1,
            std::cmp::Ordering::Greater => hi = start,
        }
    }
    false
}

fn is_common(candidate: &str) -> bool {
    sorted_lines_contain(LIST_SOURCE, candidate)
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
    let core = lower.trim_matches(|c: char| {
        c.is_ascii_digit() || c.is_ascii_punctuation() || c.is_whitespace()
    });
    if is_common(&lower)
        || (!core.is_empty() && is_common(core))
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
    fn the_bundled_list_is_sorted_unique_and_lowercase() {
        // Binary search depends on this; see common_passwords.NOTICE.
        let lines: Vec<&str> = LIST_SOURCE.lines().collect();
        assert!(lines.len() > 10_000, "{} entries", lines.len());
        for pair in lines.windows(2) {
            assert!(
                pair[0] < pair[1],
                "not sorted/unique (LC_ALL=C sort -u): {:?} then {:?}",
                pair[0],
                pair[1]
            );
        }
        for line in &lines {
            assert!(!line.is_empty(), "blank line");
            assert_eq!(line.trim(), *line, "whitespace around {line:?}");
            assert_eq!(line.to_lowercase(), *line, "{line:?} isn't lowercase");
            assert!(!line.contains('\r'), "CRLF line ending");
        }
    }

    #[test]
    fn every_listed_entry_is_found_and_others_are_not() {
        for line in LIST_SOURCE.lines() {
            assert!(is_common(line), "{line:?} not found by binary search");
        }
        for absent in ["", "zzzzzzzzzzzzzzzzzz", "000", "purple otter lamp drift"] {
            assert!(!is_common(absent), "{absent:?}");
        }
        assert!(sorted_lines_contain("a\nb\nc", "c"));
        assert!(sorted_lines_contain("a\nb\nc\n", "a"));
        assert!(!sorted_lines_contain("a\nb\nc\n", "bb"));
        assert!(!sorted_lines_contain("", "a"));
    }

    #[test]
    fn the_list_costs_no_heap_and_stays_small() {
        // The list lives in read-only data and is searched in place (no set is
        // built at startup); keep the binary from bloating if a much bigger
        // list is dropped in.
        assert!(
            LIST_SOURCE.len() < 2 * 1024 * 1024,
            "{} bytes",
            LIST_SOURCE.len()
        );
        assert!(is_common("password") && is_common("p@ssw0rd") && is_common("dragon"));
    }
}
