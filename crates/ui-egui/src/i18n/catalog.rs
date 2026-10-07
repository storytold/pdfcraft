//! Strict bundled-catalog parser. Runtime callers log errors and use the English fallback;
//! tests reject invalid catalogs before they can ship.

use std::collections::{BTreeSet, HashMap};

#[derive(Debug)]
pub(super) struct Entry {
    pub context: String,
    pub source: String,
    pub translation: String,
}

#[derive(Debug, Default)]
pub(super) struct Catalog {
    plain: HashMap<String, String>,
    contextual: HashMap<String, HashMap<String, String>>,
    ids: HashMap<String, String>,
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

pub(super) fn placeholders(text: &str) -> Vec<&str> {
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

fn validate_entry(entry: &Entry, forms: usize) -> Result<(), String> {
    let Entry { context, source, translation } = entry;
    match context.as_str() {
        "@id" => {
            // The source is a command id, not a template. Bundled-catalog tests validate the
            // translation against that command's English label and reject unknown ids.
            Ok(())
        }
        "@plural" => {
            let source_forms: Vec<_> = source.split('|').collect();
            let [one, other] = source_forms.as_slice() else { return Err("plural source must be one|other".into()) };
            if one.is_empty() || other.is_empty() || placeholders(one) != placeholders(other) {
                return Err("English plural forms must be nonempty and have matching placeholders".into());
            }
            let translations: Vec<_> = translation.split('|').collect();
            if translations.len() != forms || translations.iter().any(|text| text.is_empty()) {
                return Err(format!("expected {forms} nonempty plural forms"));
            }
            if translations.iter().any(|text| placeholders(text) != placeholders(other)) {
                return Err("plural placeholders differ from the English source".into());
            }
            Ok(())
        }
        _ => {
            if context.starts_with('@') {
                return Err(format!("unknown reserved context {context:?}"));
            }
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

pub(super) fn parse_entries(text: &str, forms: usize) -> Result<Vec<Entry>, String> {
    if forms == 0 {
        return Err("a language must have at least one plural form".into());
    }
    let mut entries = Vec::new();
    let mut seen = BTreeSet::new();
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let parse = || -> Result<Entry, String> {
            let mut columns = line.split('\t');
            let (Some(context), Some(source), Some(translation), None) = (columns.next(), columns.next(), columns.next(), columns.next()) else {
                return Err("expected context<TAB>source<TAB>translation".into());
            };
            let entry = Entry { context: unescape(context)?, source: unescape(source)?, translation: unescape(translation)? };
            if entry.source.is_empty() || entry.translation.is_empty() {
                return Err("source and translation must be nonempty".into());
            }
            validate_entry(&entry, forms)?;
            Ok(entry)
        };
        let entry = parse().map_err(|error| format!("line {}: {error}", index.saturating_add(1)))?;
        if !seen.insert((entry.context.clone(), entry.source.clone())) {
            return Err(format!("line {}: duplicate key {:?} / {:?}", index.saturating_add(1), entry.context, entry.source));
        }
        entries.push(entry);
    }
    Ok(entries)
}

impl Catalog {
    pub fn parse(text: &str, forms: usize) -> Result<Self, String> {
        let mut catalog = Self::default();
        for Entry { context, source, translation } in parse_entries(text, forms)? {
            match context.as_str() {
                "" => {
                    catalog.plain.insert(source, translation);
                }
                "@id" => {
                    catalog.ids.insert(source, translation);
                }
                "@plural" => {
                    catalog.plurals.insert(source, translation.split('|').map(str::to_owned).collect());
                }
                _ => {
                    catalog.contextual.entry(context).or_default().insert(source, translation);
                }
            }
        }
        Ok(catalog)
    }

    pub fn plain(&self, source: &str) -> Option<&str> {
        self.plain.get(source).map(String::as_str)
    }

    pub fn contextual(&self, context: &str, source: &str) -> Option<&str> {
        self.contextual.get(context)?.get(source).map(String::as_str)
    }

    pub fn id(&self, id: &str) -> Option<&str> {
        self.ids.get(id).map(String::as_str)
    }

    pub fn plural(&self, one: &str, other: &str, form: usize) -> Option<&str> {
        self.plurals.get(&format!("{one}|{other}"))?.get(form).map(String::as_str)
    }
}
