//! Custom redaction code sets (Acrobat's Redaction Properties ▸ Edit Codes): sets the user
//! creates, renames and deletes, with codes added, renamed and removed, kept beside the read-only
//! built-in sets and moved between computers as a file.
//!
//! The file is PdfCraft's own small, versioned JSON format:
//!
//! ```json
//! { "format": "pdfcraft-redaction-codes", "version": 1,
//!   "sets": [ { "name": "Agency", "codes": ["A-1", "A-2"] } ] }
//! ```
//!
//! Files and saved settings are untrusted. Reading one checks everything before anything is kept
//! and refuses the whole of it with what was wrong: its size, the number of sets and of codes in a
//! set, the length of names and codes, empty names, names or codes that repeat, control
//! characters, unknown keys and versions newer than this one reads.

use serde_json::{Map, Value, json};

use super::{CODE_SETS, CodeSet};

/// The `format` value every code set file starts with.
pub const FILE_FORMAT: &str = "pdfcraft-redaction-codes";
/// The newest file version this build reads and the one it writes.
pub const FILE_VERSION: u64 = 1;
/// At most this many custom sets.
pub const MAX_SETS: usize = 50;
/// At most this many codes in one set.
pub const MAX_CODES: usize = 200;
/// The longest set name, in characters.
pub const MAX_NAME_CHARS: usize = 80;
/// The longest code, in characters.
pub const MAX_CODE_CHARS: usize = 40;
/// The largest code set file read (4 MiB). The largest library the bounds above allow is written
/// within it.
pub const MAX_FILE_BYTES: usize = 4 << 20;

/// What was wrong with a set name, a code, an edit or a file.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CodeSetError {
    #[error("a {0} can't be empty")]
    Empty(Part),
    #[error("the {part} {text:?} starts or ends with a space")]
    Spaces { part: Part, text: String },
    #[error("the {part} {text:?} holds a control character (such as a tab or a line break)")]
    Control { part: Part, text: String },
    #[error("the {part} {text:?} is longer than {max} characters")]
    TooLong { part: Part, text: String, max: usize },
    #[error("{0:?} is a built-in code set, which can't be changed, renamed, deleted or exported")]
    BuiltIn(String),
    #[error("{0:?} is the name of a built-in code set; choose another name")]
    BuiltInName(String),
    #[error("there is already a code set named {0:?}")]
    DuplicateSet(String),
    #[error("there is no custom code set named {0:?}")]
    NoSuchSet(String),
    #[error("the code set {set:?} already has the code {code:?}")]
    DuplicateCode { set: String, code: String },
    #[error("the code set {set:?} has no code {code:?}")]
    NoSuchCode { set: String, code: String },
    #[error("at most {} custom code sets are allowed", MAX_SETS)]
    TooManySets,
    #[error("the code set {0:?} can't hold more than {max} codes", max = MAX_CODES)]
    TooManyCodes(String),
    #[error("the file is larger than the {} bytes a code set file can be", MAX_FILE_BYTES)]
    FileTooLarge,
    #[error("not a redaction code set file: {0}")]
    Malformed(String),
    #[error("version {0} of the code set file is newer than this version of PdfCraft reads (up to {newest})", newest = FILE_VERSION)]
    NewerVersion(u64),
    #[error("{at}: {error}")]
    At { at: String, error: Box<CodeSetError> },
}

/// Which kind of text an error is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Name,
    Code,
}

impl std::fmt::Display for Part {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Part::Name => "code set name",
            Part::Code => "redaction code",
        })
    }
}

fn check_text(part: Part, text: &str) -> Result<(), CodeSetError> {
    let max = match part {
        Part::Name => MAX_NAME_CHARS,
        Part::Code => MAX_CODE_CHARS,
    };
    if text.is_empty() {
        return Err(CodeSetError::Empty(part));
    }
    // Only the first characters go into the message: the text may be anything a file held.
    let shown = || text.chars().take(max).collect::<String>();
    if text.chars().any(char::is_control) {
        return Err(CodeSetError::Control { part, text: shown() });
    }
    if text.chars().count() > max {
        return Err(CodeSetError::TooLong { part, text: shown(), max });
    }
    if text.trim() != text {
        return Err(CodeSetError::Spaces { part, text: text.to_string() });
    }
    Ok(())
}

/// Two set names are the same when they differ only in letter case.
fn same_name(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

/// A set the user made: its name and its codes, in the order the overlay text lists them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustomCodeSet {
    name: String,
    codes: Vec<String>,
}

impl CustomCodeSet {
    /// A checked set: a valid name not taken by a built-in set and valid codes, each once, within the bounds.
    pub fn new(name: &str, codes: &[&str]) -> Result<Self, CodeSetError> {
        check_text(Part::Name, name)?;
        if CODE_SETS.iter().any(|b| same_name(b.id, name) || same_name(b.name, name)) {
            return Err(CodeSetError::BuiltInName(name.to_string()));
        }
        let mut set = Self { name: name.to_string(), codes: Vec::new() };
        for code in codes {
            set.push(code)?;
        }
        Ok(set)
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn codes(&self) -> &[String] {
        &self.codes
    }

    fn push(&mut self, code: &str) -> Result<(), CodeSetError> {
        check_text(Part::Code, code)?;
        if self.codes.iter().any(|c| c == code) {
            return Err(CodeSetError::DuplicateCode { set: self.name.clone(), code: code.to_string() });
        }
        if self.codes.len() >= MAX_CODES {
            return Err(CodeSetError::TooManyCodes(self.name.clone()));
        }
        self.codes.push(code.to_string());
        Ok(())
    }

    fn position(&self, code: &str) -> Result<usize, CodeSetError> {
        self.codes.iter().position(|c| c == code).ok_or_else(|| CodeSetError::NoSuchCode { set: self.name.clone(), code: code.to_string() })
    }
}

/// A built-in set or a custom one, as the Redaction Properties list them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetRef<'a> {
    BuiltIn(CodeSet),
    Custom(&'a CustomCodeSet),
}

impl<'a> SetRef<'a> {
    /// What names the set in settings and tools: a built-in set's id, a custom set's name.
    pub fn key(&self) -> &'a str {
        match self {
            SetRef::BuiltIn(s) => s.id,
            SetRef::Custom(s) => &s.name,
        }
    }

    pub fn name(&self) -> &'a str {
        match self {
            SetRef::BuiltIn(s) => s.name,
            SetRef::Custom(s) => &s.name,
        }
    }

    pub fn built_in(&self) -> bool {
        matches!(self, SetRef::BuiltIn(_))
    }

    pub fn codes(&self) -> Vec<&'a str> {
        match self {
            SetRef::BuiltIn(s) => s.codes.to_vec(),
            SetRef::Custom(s) => s.codes.iter().map(String::as_str).collect(),
        }
    }

    pub fn contains(&self, code: &str) -> bool {
        self.codes().contains(&code)
    }

    /// The overlay text for the picked codes: in the set's order, once each, joined by ", ".
    /// Fails with the first picked code that isn't in the set.
    pub fn overlay<'p, S: AsRef<str>>(&self, picked: &'p [S]) -> Result<String, &'p str> {
        let codes = self.codes();
        if let Some(bad) = picked.iter().map(AsRef::as_ref).find(|p| !codes.contains(p)) {
            return Err(bad);
        }
        Ok(codes.into_iter().filter(|c| picked.iter().any(|p| p.as_ref() == *c)).collect::<Vec<_>>().join(", "))
    }
}

/// One change Edit Codes makes. Built-in sets refuse every one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CodeEdit {
    CreateSet { name: String },
    RenameSet { from: String, to: String },
    DeleteSet { name: String },
    AddCode { set: String, code: String },
    RenameCode { set: String, from: String, to: String },
    RemoveCode { set: String, code: String },
}

/// The custom code sets, beside the built-in [`CODE_SETS`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CodeLibrary {
    sets: Vec<CustomCodeSet>,
}

impl CodeLibrary {
    /// The custom sets, in the order they were made or imported.
    pub fn custom(&self) -> &[CustomCodeSet] {
        &self.sets
    }

    /// Every set: the built-in ones first, then the custom ones.
    pub fn all(&self) -> impl Iterator<Item = SetRef<'_>> {
        CODE_SETS.into_iter().map(SetRef::BuiltIn).chain(self.sets.iter().map(SetRef::Custom))
    }

    /// The set a key names: a built-in set's id, or a custom set's name as it is written.
    pub fn find(&self, key: &str) -> Option<SetRef<'_>> {
        self.all().find(|s| s.key() == key)
    }

    fn custom_index(&self, name: &str) -> Result<usize, CodeSetError> {
        if CODE_SETS.iter().any(|b| b.id == name || b.name == name) {
            return Err(CodeSetError::BuiltIn(name.to_string()));
        }
        self.sets.iter().position(|s| s.name == name).ok_or_else(|| CodeSetError::NoSuchSet(name.to_string()))
    }

    /// A name free for a set: valid, not a built-in set's, and not another custom set's
    /// (`except` is the set being renamed).
    fn check_new_name(&self, name: &str, except: Option<usize>) -> Result<(), CodeSetError> {
        CustomCodeSet::new(name, &[])?;
        if self.sets.iter().enumerate().any(|(i, s)| Some(i) != except && same_name(&s.name, name)) {
            return Err(CodeSetError::DuplicateSet(name.to_string()));
        }
        Ok(())
    }

    /// Make one change. Nothing changes when it fails.
    pub fn apply(&mut self, edit: &CodeEdit) -> Result<(), CodeSetError> {
        match edit {
            CodeEdit::CreateSet { name } => {
                self.check_new_name(name, None)?;
                if self.sets.len() >= MAX_SETS {
                    return Err(CodeSetError::TooManySets);
                }
                self.sets.push(CustomCodeSet { name: name.clone(), codes: Vec::new() });
            }
            CodeEdit::RenameSet { from, to } => {
                let i = self.custom_index(from)?;
                self.check_new_name(to, Some(i))?;
                if let Some(s) = self.sets.get_mut(i) {
                    s.name = to.clone();
                }
            }
            CodeEdit::DeleteSet { name } => {
                let i = self.custom_index(name)?;
                self.sets.remove(i);
            }
            CodeEdit::AddCode { set, code } => {
                let i = self.custom_index(set)?;
                if let Some(s) = self.sets.get_mut(i) {
                    s.push(code)?;
                }
            }
            CodeEdit::RenameCode { set, from, to } => {
                let i = self.custom_index(set)?;
                let Some(s) = self.sets.get_mut(i) else { return Err(CodeSetError::NoSuchSet(set.clone())) };
                let at = s.position(from)?;
                check_text(Part::Code, to)?;
                if s.codes.iter().enumerate().any(|(j, c)| j != at && c == to) {
                    return Err(CodeSetError::DuplicateCode { set: set.clone(), code: to.clone() });
                }
                if let Some(c) = s.codes.get_mut(at) {
                    *c = to.clone();
                }
            }
            CodeEdit::RemoveCode { set, code } => {
                let i = self.custom_index(set)?;
                let Some(s) = self.sets.get_mut(i) else { return Err(CodeSetError::NoSuchSet(set.clone())) };
                let at = s.position(code)?;
                s.codes.remove(at);
            }
        }
        Ok(())
    }

    /// Add sets read from a file. A set whose name a custom set already has replaces it when
    /// `replace` is on and is refused otherwise; all are added or none. Returns the names added.
    pub fn import(&mut self, sets: Vec<CustomCodeSet>, replace: bool) -> Result<Vec<String>, CodeSetError> {
        let mut next = self.sets.clone();
        let mut names = Vec::new();
        for set in sets {
            match next.iter().position(|s| same_name(&s.name, &set.name)) {
                Some(i) if replace => {
                    if let Some(slot) = next.get_mut(i) {
                        *slot = set.clone();
                    }
                }
                Some(_) => return Err(CodeSetError::DuplicateSet(set.name)),
                None => {
                    if next.len() >= MAX_SETS {
                        return Err(CodeSetError::TooManySets);
                    }
                    next.push(set.clone());
                }
            }
            names.push(set.name);
        }
        self.sets = next;
        Ok(names)
    }

    /// The custom sets as file content; `only` picks some by name (built-in sets can't be
    /// exported: they are in every copy of PdfCraft).
    pub fn export(&self, only: Option<&[&str]>) -> Result<Vec<u8>, CodeSetError> {
        let sets: Vec<&CustomCodeSet> = match only {
            None => self.sets.iter().collect(),
            Some(names) => {
                let mut picked = Vec::new();
                for name in names {
                    let i = self.custom_index(name)?;
                    if let Some(s) = self.sets.get(i)
                        && !picked.contains(&s)
                    {
                        picked.push(s);
                    }
                }
                picked
            }
        };
        let mut bytes = serde_json::to_vec_pretty(&file_json(&sets)).map_err(|e| CodeSetError::Malformed(e.to_string()))?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    /// The sets of a code set file, checked. The whole file is refused with the first problem.
    pub fn read_file(bytes: &[u8]) -> Result<Vec<CustomCodeSet>, CodeSetError> {
        if bytes.len() > MAX_FILE_BYTES {
            return Err(CodeSetError::FileTooLarge);
        }
        let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
        let value: Value = serde_json::from_slice(bytes).map_err(|e| CodeSetError::Malformed(format!("it isn't JSON ({e})")))?;
        read_value(&value)
    }

    /// The library as saved in the settings: the file's own form.
    pub fn to_json(&self) -> Value {
        file_json(&self.sets.iter().collect::<Vec<_>>())
    }

    /// The library saved by [`to_json`](Self::to_json), checked as a file is.
    pub fn from_json(value: &Value) -> Result<Self, CodeSetError> {
        let mut library = Self::default();
        library.import(read_value(value)?, false)?;
        Ok(library)
    }
}

fn file_json(sets: &[&CustomCodeSet]) -> Value {
    json!({
        "format": FILE_FORMAT,
        "version": FILE_VERSION,
        "sets": sets.iter().map(|s| json!({ "name": s.name, "codes": s.codes })).collect::<Vec<_>>(),
    })
}

fn malformed(what: impl Into<String>) -> CodeSetError {
    CodeSetError::Malformed(what.into())
}

fn only_keys(object: &Map<String, Value>, allowed: &[&str], at: &str) -> Result<(), CodeSetError> {
    match object.keys().find(|k| !allowed.contains(&k.as_str())) {
        Some(k) => {
            let k: String = k.chars().take(MAX_NAME_CHARS).collect();
            Err(malformed(format!("{at} has an unknown key {k:?} (allowed: {})", allowed.join(", "))))
        }
        None => Ok(()),
    }
}

fn read_value(value: &Value) -> Result<Vec<CustomCodeSet>, CodeSetError> {
    let Some(top) = value.as_object() else { return Err(malformed("it isn't a JSON object")) };
    if top.get("format").and_then(Value::as_str) != Some(FILE_FORMAT) {
        return Err(malformed(format!("\"format\" must be {FILE_FORMAT:?}")));
    }
    match top.get("version").map(|v| (v, v.as_u64())) {
        Some((_, Some(v))) if v > FILE_VERSION => return Err(CodeSetError::NewerVersion(v)),
        Some((_, Some(v))) if v >= 1 => {}
        _ => return Err(malformed("\"version\" must be a whole number from 1")),
    }
    only_keys(top, &["format", "version", "sets"], "the file")?;
    let Some(list) = top.get("sets").and_then(Value::as_array) else { return Err(malformed("\"sets\" must be a list")) };
    if list.len() > MAX_SETS {
        return Err(CodeSetError::TooManySets);
    }
    let mut sets: Vec<CustomCodeSet> = Vec::with_capacity(list.len());
    for (n, entry) in list.iter().enumerate() {
        let at = format!("set {}", n + 1);
        let within = |error: CodeSetError| CodeSetError::At { at: at.clone(), error: Box::new(error) };
        let Some(object) = entry.as_object() else { return Err(within(malformed("it isn't a JSON object"))) };
        only_keys(object, &["name", "codes"], &at)?;
        let Some(name) = object.get("name").and_then(Value::as_str) else { return Err(within(malformed("\"name\" must be text"))) };
        let Some(codes) = object.get("codes").and_then(Value::as_array) else { return Err(within(malformed("\"codes\" must be a list"))) };
        if codes.len() > MAX_CODES {
            let name: String = name.chars().take(MAX_NAME_CHARS).collect();
            return Err(within(CodeSetError::TooManyCodes(name)));
        }
        let mut texts = Vec::with_capacity(codes.len());
        for (k, code) in codes.iter().enumerate() {
            match code.as_str() {
                Some(c) => texts.push(c),
                None => return Err(within(malformed(format!("code {} must be text", k + 1)))),
            }
        }
        let set = CustomCodeSet::new(name, &texts).map_err(within)?;
        if sets.iter().any(|s| same_name(&s.name, &set.name)) {
            return Err(within(CodeSetError::DuplicateSet(set.name)));
        }
        sets.push(set);
    }
    Ok(sets)
}
