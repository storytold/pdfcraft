//! Parser and lookup tables for translation catalogs (`*.tsv`, format documented in `ja.tsv`).
//!
//! Validation is strict (unknown escapes, unknown `@` contexts, placeholder, ellipsis and plural
//! form mismatches, duplicates), but a bad line is only skipped and reported: the rest of the
//! catalog still loads and the skipped string shows in English. The tests insist the bundled
//! catalogs have no errors at all.

use std::collections::{HashMap, HashSet};

/// A catalog entry as read from the file.
#[derive(Debug, PartialEq, Eq)]
pub struct Entry {
    pub context: String,
    pub source: String,
    pub translation: String,
}

/// A parsed catalog. Values are owned for the life of the process (catalogs are built once).
#[derive(Debug, Default)]
pub struct Catalog {
    /// context-free strings: English source → translation (the hot path, looked up every frame)
    plain: HashMap<String, String>,
    /// strings with a disambiguating context: context → source → translation
    contextual: HashMap<String, HashMap<String, String>>,
    /// command-id keyed strings
    ids: HashMap<String, String>,
    /// plural messages: `one|other` → forms
    plurals: HashMap<String, Vec<String>>,
}

fn unescape(text: &str) -> Result<String, String> {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('\\') => out.push('\\'),
            Some(other) => return Err(format!("unknown escape \\{other}; use \\n, \\t or \\\\")),
            None => return Err("trailing backslash; use \\\\ for a literal backslash".into()),
        }
    }
    Ok(out)
}

/// `{name}` placeholders of a template, sorted (translators may reorder them).
pub fn placeholders(text: &str) -> Vec<&str> {
    let mut names = Vec::new();
    let mut rest = text;
    while let Some((_, after_open)) = rest.split_once('{') {
        let Some((name, after_close)) = after_open.split_once('}') else { break };
        names.push(name);
        rest = after_close;
    }
    names.sort_unstable();
    names
}

/// Check one entry against its English source. `forms` is the language's number of plural forms.
fn validate(entry: &Entry, forms: usize) -> Result<(), String> {
    let Entry { context, source, translation } = entry;
    match context.as_str() {
        // The source is a command id, not a template; the bundled-catalog tests check the id
        // exists and compare the translation with that command's English label.
        "@id" => Ok(()),
        "@plural" => {
            let english: Vec<&str> = source.split('|').collect();
            let [one, other] = english.as_slice() else { return Err("plural source must be one|other".into()) };
            if one.is_empty() || other.is_empty() || placeholders(one) != placeholders(other) {
                return Err("English plural forms must be nonempty and have matching placeholders".into());
            }
            let translated: Vec<&str> = translation.split('|').collect();
            if translated.len() != forms || translated.iter().any(|form| form.is_empty()) {
                return Err(format!("expected {forms} nonempty plural forms"));
            }
            if translated.iter().any(|form| placeholders(form) != placeholders(other)) {
                return Err("plural placeholders differ from the English source".into());
            }
            Ok(())
        }
        _ if context.starts_with('@') => Err(format!("unknown reserved context {context:?}")),
        _ => {
            if placeholders(source) != placeholders(translation) {
                return Err("placeholders differ from the English source".into());
            }
            if source.ends_with('…') != translation.ends_with('…') {
                return Err("trailing ellipsis differs from the English source".into());
            }
            Ok(())
        }
    }
}

fn parse_line(line: &str, forms: usize) -> Result<Entry, String> {
    let mut columns = line.split('\t');
    let (Some(context), Some(source), Some(translation), None) = (columns.next(), columns.next(), columns.next(), columns.next()) else {
        return Err("expected `context<TAB>source<TAB>translation`".into());
    };
    let entry = Entry { context: unescape(context)?, source: unescape(source)?, translation: unescape(translation)? };
    if entry.source.is_empty() || entry.translation.is_empty() {
        return Err("source and translation must be nonempty".into());
    }
    validate(&entry, forms)?;
    Ok(entry)
}

/// Read the entries of a catalog file for a language with `forms` plural forms. Malformed lines
/// and duplicates (the first entry wins) are returned as errors and skipped, so a bad translation
/// never breaks the UI.
pub fn parse_entries(text: &str, forms: usize) -> (Vec<Entry>, Vec<String>) {
    let mut entries = Vec::new();
    let mut errors = Vec::new();
    let mut seen = HashSet::new();
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let line_no = index.saturating_add(1);
        match parse_line(line, forms) {
            Ok(entry) if !seen.insert((entry.context.clone(), entry.source.clone())) => {
                errors.push(format!("line {line_no}: duplicate key {:?} / {:?}", entry.context, entry.source));
            }
            Ok(entry) => entries.push(entry),
            Err(error) => errors.push(format!("line {line_no}: {error}")),
        }
    }
    (entries, errors)
}

impl Catalog {
    /// Build a catalog from its valid entries; the errors of the skipped lines come back too.
    pub fn parse(text: &str, forms: usize) -> (Catalog, Vec<String>) {
        let (entries, errors) = parse_entries(text, forms);
        let mut c = Catalog::default();
        for Entry { context, source, translation } in entries {
            match context.as_str() {
                "" => {
                    c.plain.insert(source, translation);
                }
                "@id" => {
                    c.ids.insert(source, translation);
                }
                "@plural" => {
                    c.plurals.insert(source, translation.split('|').map(str::to_string).collect());
                }
                _ => {
                    c.contextual.entry(context).or_default().insert(source, translation);
                }
            }
        }
        (c, errors)
    }

    /// Rewrite every translation (each plural form on its own), e.g. into display order for a
    /// right-to-left language. Keys are untouched.
    pub fn map_translations(&mut self, f: impl Fn(&str) -> String) {
        let tables = std::iter::once(&mut self.plain).chain(self.contextual.values_mut()).chain(std::iter::once(&mut self.ids));
        for translation in tables.flat_map(HashMap::values_mut).chain(self.plurals.values_mut().flatten()) {
            *translation = f(translation);
        }
    }

    pub fn plain(&self, s: &str) -> Option<&str> {
        self.plain.get(s).map(String::as_str)
    }

    pub fn contextual(&self, ctx: &str, s: &str) -> Option<&str> {
        self.contextual.get(ctx)?.get(s).map(String::as_str)
    }

    pub fn id(&self, id: &str) -> Option<&str> {
        self.ids.get(id).map(String::as_str)
    }

    /// The plural form `index` of the message whose English forms are `one|other`.
    pub fn plural(&self, one: &str, other: &str, index: usize) -> Option<&str> {
        let forms = self.plurals.get(&format!("{one}|{other}"))?;
        forms.get(index.min(forms.len().saturating_sub(1))).map(String::as_str)
    }
}
