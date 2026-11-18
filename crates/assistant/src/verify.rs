//! Checking the model's answer against the document.
//!
//! A quotation counts only when its words appear, in order, in the text of a page that was sent:
//! letter case, typographic quotes and dashes, runs of white space and words hyphenated across a
//! line break are compared loosely; nothing else is. What is shown is the page's own text at the
//! matched place, never the model's version of it. A quotation found on another page than the
//! one the model named is kept with the right page and says so; one found nowhere is dropped and
//! listed. A point left without any verified quotation is marked as unsupported.

use serde_json::{Value, json};

use crate::pages::{DocText, Origin};

/// Characters a quotation needs (after normalising) to be checked at all: shorter ones match by
/// chance.
pub const MIN_QUOTE_CHARS: usize = 12;

/// One verified quotation.
#[derive(Clone, Debug, PartialEq)]
pub struct Quote {
    /// 0-based.
    pub page: usize,
    /// The page's own text at the matched place.
    pub text: String,
    pub origin: Origin,
    /// The model named another page (1-based, as it wrote it).
    pub claimed_page: Option<i64>,
}

/// A quotation that was not found.
#[derive(Clone, Debug, PartialEq)]
pub struct Rejected {
    pub claimed_page: Option<i64>,
    pub text: String,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Point {
    /// The model's own words (a synthesis, not a quotation).
    pub statement: String,
    pub quotes: Vec<Quote>,
}

impl Point {
    pub fn supported(&self) -> bool {
        !self.quotes.is_empty()
    }

    /// Pages (0-based) this point's verified quotations come from, in order.
    pub fn pages(&self) -> Vec<usize> {
        let mut p: Vec<usize> = self.quotes.iter().map(|q| q.page).collect();
        p.sort_unstable();
        p.dedup();
        p
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Answer {
    pub points: Vec<Point>,
    pub rejected: Vec<Rejected>,
    /// The model said the text does not contain the answer.
    pub not_found: bool,
    /// The reply could not be read as the requested JSON (its start, for the person to see).
    pub unparsed: Option<String>,
    /// For a translation: each page sent (0-based) with the model's translation of it. A
    /// translation can't be checked word for word; only that it belongs to a page that was sent.
    pub translations: Vec<(usize, String)>,
}

/// The JSON schema of a translation reply.
pub fn translation_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "pages": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": { "page": { "type": "integer" }, "text": { "type": "string" } },
                    "required": ["page", "text"]
                }
            }
        },
        "required": ["pages"]
    })
}

/// Read a translation reply: pages the model names that weren't sent are dropped and listed.
pub fn check_translation(raw: &str, sent: &[usize]) -> Answer {
    let mut answer = Answer::default();
    let Some(v) = json_of(raw) else {
        answer.unparsed = Some(cut(raw, 600));
        return answer;
    };
    for item in v.get("pages").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]).iter().take(200) {
        let claimed = item.get("page").and_then(Value::as_i64);
        let text = item.get("text").and_then(Value::as_str).unwrap_or("").trim().to_owned();
        match claimed.and_then(|c| usize::try_from(c).ok()).and_then(|c| c.checked_sub(1)).filter(|p| sent.contains(p)) {
            Some(page) if !text.is_empty() => answer.translations.push((page, text)),
            _ => {
                answer
                    .rejected
                    .push(Rejected { claimed_page: claimed, text: cut(&text, 300), reason: "página no enviada o traducción vacía".into() })
            }
        }
    }
    answer.translations.sort_by_key(|(p, _)| *p);
    answer
}

/// The JSON schema asked of the model (Ollama's `format`).
pub fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "points": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "statement": { "type": "string" },
                        "quotes": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": { "page": { "type": "integer" }, "text": { "type": "string" } },
                                "required": ["page", "text"]
                            }
                        }
                    },
                    "required": ["statement", "quotes"]
                }
            },
            "not_found": { "type": "boolean" }
        },
        "required": ["points", "not_found"]
    })
}

/// The JSON object in a reply that may wrap it in prose or a code fence.
fn json_of(raw: &str) -> Option<Value> {
    if let Ok(v) = serde_json::from_str::<Value>(raw.trim()) {
        return Some(v);
    }
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    serde_json::from_str(raw.get(start..=end)?).ok()
}

fn cut(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

pub fn check(raw: &str, doc: &DocText, sent: &[usize]) -> Answer {
    let mut answer = Answer::default();
    let Some(v) = json_of(raw) else {
        answer.unparsed = Some(cut(raw, 600));
        return answer;
    };
    answer.not_found = v.get("not_found").and_then(Value::as_bool).unwrap_or(false);
    let indexed: Vec<(usize, Normalized)> = sent.iter().filter_map(|&p| doc.page(p).map(|s| (p, Normalized::new(&s.text)))).collect();
    let points = v.get("points").and_then(Value::as_array).cloned().unwrap_or_default();
    for item in points.iter().take(40) {
        let statement = item.get("statement").and_then(Value::as_str).unwrap_or("").trim().to_owned();
        let mut quotes = Vec::new();
        for q in item.get("quotes").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]).iter().take(20) {
            let text = q.get("text").and_then(Value::as_str).unwrap_or("").to_owned();
            let claimed = q.get("page").and_then(Value::as_i64);
            match locate(&text, claimed, &indexed) {
                Ok((page, found)) => {
                    let origin = doc.page(page).map_or(Origin::Extracted, |p| p.origin.clone());
                    let moved = claimed.filter(|c| usize::try_from(*c).ok() != Some(page + 1));
                    if !quotes.iter().any(|x: &Quote| x.page == page && x.text == found) {
                        quotes.push(Quote { page, text: found, origin, claimed_page: moved });
                    }
                }
                Err(reason) => answer.rejected.push(Rejected { claimed_page: claimed, text: cut(&text, 300), reason }),
            }
        }
        if !statement.is_empty() || !quotes.is_empty() {
            answer.points.push(Point { statement, quotes });
        }
    }
    answer
}

fn locate(quote: &str, claimed: Option<i64>, pages: &[(usize, Normalized)]) -> Result<(usize, String), String> {
    let q = Normalized::new(quote);
    if q.chars.len() < MIN_QUOTE_CHARS {
        return Err("demasiado breve para verificarla".into());
    }
    let claimed = claimed.and_then(|c| usize::try_from(c).ok()).and_then(|c| c.checked_sub(1));
    // The page the model named first, then the others.
    let order = pages.iter().filter(|(p, _)| Some(*p) == claimed).chain(pages.iter().filter(|(p, _)| Some(*p) != claimed));
    for (page, text) in order {
        if let Some(found) = text.find(&q) {
            return Ok((*page, found));
        }
    }
    Err(match claimed {
        Some(c) if pages.iter().any(|(p, _)| *p == c) => format!("no figura en la página {}", c + 1),
        Some(c) => format!("la página {} no fue enviada al modelo", c + 1),
        None => "no figura en las páginas enviadas".into(),
    })
}

/// Text folded for comparison, keeping where each folded character came from.
struct Normalized {
    original: Vec<char>,
    chars: Vec<char>,
    /// For each folded character, its index in `original`.
    at: Vec<usize>,
}

impl Normalized {
    fn new(s: &str) -> Normalized {
        let original: Vec<char> = s.chars().collect();
        let mut chars = Vec::with_capacity(original.len());
        let mut at = Vec::with_capacity(original.len());
        let mut i = 0;
        while i < original.len() {
            let c = original.get(i).copied().unwrap_or(' ');
            // A word hyphenated across a line break: drop the hyphen and the break.
            if matches!(c, '-' | '\u{ad}' | '\u{2010}') {
                let mut j = i + 1;
                let mut newline = false;
                while let Some(&n) = original.get(j) {
                    if !n.is_whitespace() {
                        break;
                    }
                    newline |= n == '\n';
                    j += 1;
                }
                if newline || c == '\u{ad}' {
                    i = j;
                    continue;
                }
            }
            let mapped = match c {
                '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{2032}' | '`' | '´' => '\'',
                '\u{201c}' | '\u{201d}' | '\u{201e}' | '«' | '»' => '"',
                '\u{2010}'..='\u{2015}' | '\u{2212}' => '-',
                c if c.is_whitespace() => ' ',
                c => c,
            };
            if mapped == ' ' {
                if chars.last().is_some_and(|l| *l != ' ') {
                    chars.push(' ');
                    at.push(i);
                }
            } else {
                for f in pdfcraft_render::text::fold_case(&mapped.to_string()).chars() {
                    chars.push(f);
                    at.push(i);
                }
            }
            i += 1;
        }
        if chars.last() == Some(&' ') {
            chars.pop();
            at.pop();
        }
        Normalized { original, chars, at }
    }

    /// The quotation `q` (split at ellipses, each part in order) in this text, as this text's own
    /// characters from the first part's start to the last part's end.
    fn find(&self, q: &Normalized) -> Option<String> {
        let qs: String = q.chars.iter().collect();
        let parts: Vec<Vec<char>> = qs
            .split(['…'])
            .flat_map(|p| p.split("..."))
            .map(|p| p.trim_matches(|c: char| c == ' ' || c == '"' || c == '\'').chars().collect::<Vec<char>>())
            .filter(|p| p.len() >= 4)
            .collect();
        if parts.iter().map(Vec::len).sum::<usize>() < MIN_QUOTE_CHARS {
            return None;
        }
        let mut from = 0;
        let (mut start, mut end) = (None, 0);
        for part in &parts {
            let pos = find_slice(self.chars.get(from..)?, part)? + from;
            start.get_or_insert(pos);
            end = pos + part.len();
            from = end;
        }
        let (a, b) = (*self.at.get(start?)?, *self.at.get(end.checked_sub(1)?)?);
        Some(self.original.get(a..=b)?.iter().collect::<String>().split_whitespace().collect::<Vec<_>>().join(" "))
    }
}

fn find_slice(hay: &[char], needle: &[char]) -> Option<usize> {
    if needle.is_empty() || needle.len() > hay.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}
