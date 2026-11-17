//! `spell_check_comments`: the words in comment text that a word list does not know.
use crate::{Args, Automation, Result, failed};
use pdfcraft_spell::Dictionary;
use serde_json::{Value, json};

const DEFAULT_LIMIT: usize = 1000;
const MAX_USER_WORDS: usize = 10_000;

impl Automation {
    pub(crate) fn spell_check_comments(&self, a: &Args) -> Result<Value> {
        let path = self.resolve(a.str("dictionary")?, false)?;
        let bytes = crate::read_input(&path)?;
        let text = String::from_utf8(bytes).map_err(|_| failed("the dictionary is not UTF-8 text: give a word list with one word per line"))?;
        let mut dictionary = Dictionary::from_word_list(&text);
        if dictionary.is_empty() {
            return Err(failed("the dictionary holds no words: give a word list with one word per line"));
        }
        let user_words = match a.get("words") {
            Some(_) => a.strs("words")?,
            None => Vec::new(),
        };
        if user_words.len() > MAX_USER_WORDS {
            return Err(Args::wrong("words", "at most 10000 words"));
        }
        let rejected: Vec<&str> = user_words.iter().copied().filter(|w| !dictionary.add_word(w)).collect();
        let only = a.opt_int("page")?.map(|p| (p.max(1) - 1) as usize);
        let limit = a.opt_int("limit")?.map_or(DEFAULT_LIMIT, |l| l.clamp(1, 10_000) as usize);

        let mut results = Vec::new();
        let mut checked = 0usize;
        let mut total = 0usize;
        for comment in self.comments(a)?.iter().filter(|c| only.is_none_or(|p| c.page == p)) {
            let Some(text) = comment.contents.as_deref().filter(|t| !t.trim().is_empty()) else { continue };
            checked += 1;
            for m in dictionary.check(text) {
                total += 1;
                if results.len() < limit {
                    results.push(json!({
                        "page": comment.page + 1, "index": comment.index + 1, "id": comment.name,
                        "word": m.word, "start": m.start, "end": m.end, "suggestions": m.suggestions,
                    }));
                }
            }
        }
        Ok(json!({
            "checked": checked, "count": total, "misspellings": results, "truncated": total > results.len(),
            "dictionary_words": dictionary.len(), "dictionary_truncated": dictionary.truncated(), "rejected_words": rejected,
        }))
    }
}
