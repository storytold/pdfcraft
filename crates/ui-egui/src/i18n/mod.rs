//! Interface localization. Catalogs translate presentation only; command ids, document text,
//! filenames and automation output stay stable. Missing entries fall back to English.
//!
//! Add a catalog and a registry entry to add a language. See `docs/localization.md` for the
//! TSV format, lookup conventions, validation and the intentionally partial translation scope.

mod catalog;

use std::sync::OnceLock;

use catalog::Catalog;

#[derive(Clone, Copy)]
enum PluralRule {
    OneOther,
    Single,
}

impl PluralRule {
    fn forms(self) -> usize {
        match self {
            Self::OneOther => 2,
            Self::Single => 1,
        }
    }

    fn select(self, count: u64) -> usize {
        match self {
            Self::OneOther => usize::from(count != 1),
            Self::Single => 0,
        }
    }
}

struct LanguageInfo {
    code: &'static str,
    name: &'static str,
    source: &'static str,
    plural: PluralRule,
    catalog: OnceLock<Catalog>,
}

// English is always the first entry and the default; its source text needs no catalog.
static LANGUAGES: [LanguageInfo; 2] = [
    LanguageInfo { code: "en", name: "English", source: "", plural: PluralRule::OneOther, catalog: OnceLock::new() },
    LanguageInfo { code: "ja", name: "日本語", source: include_str!("ja.tsv"), plural: PluralRule::Single, catalog: OnceLock::new() },
];

/// A supported language, resolved through the registry. Serialized settings use its code.
#[derive(Clone, Copy)]
pub struct Language(&'static LanguageInfo);

impl Default for Language {
    fn default() -> Self {
        Self::EN
    }
}

impl PartialEq for Language {
    fn eq(&self, other: &Self) -> bool {
        self.code() == other.code()
    }
}

impl Eq for Language {}

impl std::fmt::Debug for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Language").field(&self.code()).finish()
    }
}

impl serde::Serialize for Language {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.code())
    }
}

impl<'de> serde::Deserialize<'de> for Language {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let code = <String as serde::Deserialize>::deserialize(deserializer)?;
        Self::parse(&code).ok_or_else(|| serde::de::Error::custom(format!("unsupported language {code:?}; allowed: {}", Self::codes())))
    }
}

impl Language {
    pub const EN: Self = Self(&LANGUAGES[0]);

    pub fn all() -> impl Iterator<Item = Self> {
        LANGUAGES.iter().map(Self)
    }

    pub fn code(self) -> &'static str {
        self.0.code
    }

    pub fn name(self) -> &'static str {
        self.0.name
    }

    /// Canonical setting/control codes. Unknown codes never change the current preference.
    pub fn parse(code: &str) -> Option<Self> {
        Self::all().find(|language| language.code() == code)
    }

    pub(crate) fn codes() -> String {
        Self::all().map(Self::code).collect::<Vec<_>>().join(", ")
    }

    fn catalog(self) -> &'static Catalog {
        self.0.catalog.get_or_init(|| {
            Catalog::parse(self.0.source, self.0.plural.forms()).unwrap_or_else(|error| {
                log::warn!("invalid {} interface catalog: {error}; using English", self.code());
                Catalog::default()
            })
        })
    }

    /// Plain English source text; untranslated text is returned unchanged.
    pub fn tr(self, text: &str) -> &str {
        self.catalog().plain(text).unwrap_or(text)
    }

    /// Disambiguate text such as "Light"; fall back to the plain entry, then English.
    pub fn tr_ctx<'a>(self, context: &str, text: &'a str) -> &'a str {
        self.catalog().contextual(context, text).unwrap_or_else(|| self.tr(text))
    }

    /// Stable command id; fall back to the English label's translation, then the label.
    pub fn tr_id<'a>(self, id: &str, label: &'a str) -> &'a str {
        self.catalog().id(id).unwrap_or_else(|| self.tr(label))
    }

    /// Plural-aware message. Both English forms use the same named placeholders.
    /// `{n}` is filled with the count; other placeholders can be filled with [`fmt`].
    pub fn trn(self, count: u64, one: &str, other: &str) -> String {
        let text = self.catalog().plural(one, other, self.0.plural.select(count)).unwrap_or(if count == 1 { one } else { other });
        fmt(text, &[("n", &count.to_string())])
    }
}

/// Fill named placeholders once. Unknown placeholders remain visible; inserted user text is
/// never interpreted as another template, so a filename containing `{n}` stays intact.
pub fn fmt(template: &str, args: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some((prefix, after_open)) = rest.split_once('{') {
        out.push_str(prefix);
        let Some((name, after_close)) = after_open.split_once('}') else {
            out.push('{');
            out.push_str(after_open);
            return out;
        };
        if let Some((_, value)) = args.iter().find(|(key, _)| *key == name) {
            out.push_str(value);
        } else {
            out.push('{');
            out.push_str(name);
            out.push('}');
        }
        rest = after_close;
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests;
