//! Room codes: five characters players read out to each other.
//!
//! New codes avoid every character that is easy to mistake for another one:
//! `0 O`, `1 I`, `5 S`, `2 Z` and `8 B` ([`ALPHABET`], 26 characters, about
//! 11.8 million codes). Older servers also used `B S Z 2 5 8`, so those stay
//! valid in typed codes ([`LEGACY`]); `0 O 1 I` were never used.

/// Characters of new room codes.
pub const ALPHABET: &str = "ACDEFGHJKLMNPQRTUVWXY34679";

/// Characters older servers also put in codes (still accepted).
pub const LEGACY: &str = "BSZ258";

/// Length of a room code.
pub const LEN: usize = 5;

/// Whether `c` (any case) can appear in a room code, old or new.
pub fn valid_char(c: char) -> bool {
    let c = c.to_ascii_uppercase();
    ALPHABET.contains(c) || LEGACY.contains(c)
}

/// A typed code as servers store it: trimmed and upper-case.
pub fn normalize(code: &str) -> String {
    code.trim().to_ascii_uppercase()
}

/// A fresh code from `ALPHABET`, drawing bytes from `random` (rejection
/// sampling keeps every character equally likely).
pub fn generate(mut random: impl FnMut() -> u8) -> String {
    let set = ALPHABET.as_bytes();
    // largest multiple of the alphabet size that fits in a byte
    let limit = 256 - 256 % set.len();
    let mut out = String::with_capacity(LEN);
    while out.len() < LEN {
        let b = random() as usize;
        if b < limit {
            out.push(set[b % set.len()] as char);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_alphabet_has_no_lookalikes() {
        for c in "0O1I5S2Z8B".chars() {
            assert!(!ALPHABET.contains(c), "{c}");
        }
        assert_eq!(ALPHABET.len(), 26);
        assert!(
            ALPHABET
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        );
    }

    #[test]
    fn old_codes_stay_valid_and_case_does_not_matter() {
        assert!("K7S2B".chars().all(valid_char));
        assert!("acdef".chars().all(valid_char));
        assert!(!valid_char('0') && !valid_char('o') && !valid_char('1') && !valid_char('i'));
        assert_eq!(normalize(" k7s2b "), "K7S2B");
    }

    #[test]
    fn generated_codes_use_the_new_alphabet_only() {
        let mut n = 0u8;
        let code = generate(|| {
            n = n.wrapping_add(37);
            n
        });
        assert_eq!(code.len(), LEN);
        assert!(code.chars().all(|c| ALPHABET.contains(c)));
        // bytes past the last full alphabet cycle are skipped, not folded
        let mut bytes = [255u8, 254, 0, 1, 2, 3, 4].into_iter();
        assert_eq!(generate(|| bytes.next().unwrap_or(0)), "ACDEF");
    }
}
