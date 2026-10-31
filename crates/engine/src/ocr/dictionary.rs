//! The UI dictionary format: one case-insensitive word per UTF-8 line.
use std::collections::BTreeSet;

pub const MAX_BYTES: usize = 4 * 1024 * 1024;
const MAX_WORDS: usize = 65536;

fn word(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() || value.len() > 1024 || value.chars().any(char::is_control) {
        return Err("dictionary words must be nonempty single lines of at most 1024 bytes".into());
    }
    let normalized = value.to_lowercase();
    if normalized.len() > 1024 {
        return Err("normalized dictionary words must be at most 1024 bytes".into());
    }
    Ok(normalized)
}

pub fn words(text: &str) -> Result<BTreeSet<String>, String> {
    if text.len() > MAX_BYTES {
        return Err("OCR dictionary exceeds 4 MiB".into());
    }
    let mut words = BTreeSet::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        words.insert(word(line)?);
        if words.len() > MAX_WORDS {
            return Err("OCR dictionary exceeds 65536 words".into());
        }
    }
    Ok(words)
}

/// Normalize, update and bound the complete dictionary before the caller writes it.
pub fn update(text: &str, add: &[&str], remove: &[&str]) -> Result<String, String> {
    if add.len() > MAX_WORDS || remove.len() > MAX_WORDS {
        return Err("OCR dictionary exceeds 65536 words".into());
    }
    let mut entries = words(text)?;
    for value in remove {
        entries.remove(&word(value)?);
    }
    for value in add {
        entries.insert(word(value)?);
        if entries.len() > MAX_WORDS {
            return Err("OCR dictionary exceeds 65536 words".into());
        }
    }
    let mut out = String::new();
    for entry in entries {
        if out.len().checked_add(entry.len()).and_then(|n| n.checked_add(1)).is_none_or(|n| n > MAX_BYTES) {
            return Err("OCR dictionary exceeds 4 MiB".into());
        }
        out.push_str(&entry);
        out.push('\n');
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dictionary_updates_round_trip_and_refuse_expanding_normalization() {
        let text = update("Quartz\n日本語\n", &["QUARTZ", "extra"], &["日本語"]).unwrap();
        assert_eq!(text, "extra\nquartz\n");
        assert_eq!(words(&text).unwrap(), BTreeSet::from(["extra".into(), "quartz".into()]));
        assert!(update("original\n", &[&"İ".repeat(512)], &[]).unwrap_err().contains("normalized"));
        assert!(update("original\n", &["two\nlines"], &[]).is_err());
    }

    #[test]
    fn dictionary_limits_cover_existing_and_added_entries() {
        let full: String = (0..65536).map(|i| format!("word{i}\n")).collect();
        assert_eq!(words(&full).unwrap().len(), 65536);
        assert!(update(&full, &["extra"], &[]).is_err());
        assert_eq!(update(&full, &["extra"], &["word0"]).unwrap().lines().count(), 65536);
        assert!(words(&"a".repeat(MAX_BYTES + 1)).is_err());
    }
}
