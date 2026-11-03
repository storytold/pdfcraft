//! String helpers for untrusted text: cuts land on character boundaries.

/// The longest prefix of `s` that is at most `max` bytes and ends on a character boundary.
pub fn prefix(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s.get(..end).unwrap_or_default()
}

/// Cut `s` to at most `max` bytes on a character boundary. Returns whether anything was cut.
pub fn truncate(s: &mut String, max: usize) -> bool {
    let keep = prefix(s, max).len();
    let cut = keep < s.len();
    // `keep` is a character boundary (`prefix` only returns whole characters), so this can't panic.
    s.truncate(keep);
    cut
}

/// Append as much of `add` to `s` as keeps `s` within `max` bytes. Returns whether `add` was cut.
pub fn push_capped(s: &mut String, add: &str, max: usize) -> bool {
    let room = max.saturating_sub(s.len());
    let part = prefix(add, room);
    s.push_str(part);
    part.len() < add.len()
}

/// `s` cut to `max` bytes, as an owned string.
pub fn capped(s: &str, max: usize) -> String {
    prefix(s, max).to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cuts_on_character_boundaries() {
        assert_eq!(prefix("héllo", 2), "h");
        assert_eq!(prefix("héllo", 3), "hé");
        assert_eq!(prefix("日本", 4), "日");
        assert_eq!(prefix("abc", 10), "abc");
        assert_eq!(prefix("abc", 0), "");
        let mut s = String::from("日本語");
        assert!(truncate(&mut s, 7));
        assert_eq!(s, "日本");
        assert!(!truncate(&mut s, 100));
        let mut s = String::from("ab");
        assert!(push_capped(&mut s, "cdé", 4));
        assert_eq!(s, "abcd");
        assert!(push_capped(&mut s, "x", 4));
        assert_eq!(s, "abcd");
    }

    proptest::proptest! {
        #[test]
        fn prefix_is_total(s in ".*", max in 0usize..64) {
            let p = prefix(&s, max);
            proptest::prop_assert!(p.len() <= max);
            proptest::prop_assert!(s.starts_with(p));
        }
    }
}
