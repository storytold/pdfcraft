//! The Arlington model as data: the compact table `cargo xtask arlington` writes from the pinned
//! model (one `=Name` line per type, then its rows), parsed once on first use.
//!
//! The field grammar is the model's own (`INTERNAL_GRAMMAR.md` in the model repository): a row
//! has one alternative per type in its Type field, and the complex fields (`[a];[b]`) hold one
//! bracketed part per alternative. Predicates stay as text here; [`crate::expr`] reads them.

use std::collections::HashMap;
use std::sync::OnceLock;

/// The table generated from the pinned model (see `xtask/src/arlington.rs`).
const TABLE: &str = include_str!("../data/arlington.tsv");

/// A PDF version, in tenths: 1.7 is 17 and 2.0 is 20.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version(pub u8);

impl Version {
    pub const V1_0: Version = Version(10);
    pub const V2_0: Version = Version(20);

    /// `"1.7"` → 17. Only the single-digit `major.minor` form the model and PDF headers use.
    pub fn parse(s: &str) -> Option<Version> {
        let (major, minor) = s.trim().split_once('.')?;
        let major: u8 = major.parse().ok().filter(|m| (1..=9).contains(m))?;
        let minor: u8 = minor.parse().ok().filter(|m: &u8| *m <= 9 && minor.len() == 1)?;
        Some(Version(major * 10 + minor))
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.0 / 10, self.0 % 10)
    }
}

/// The model's predefined types (column Type).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Basic {
    Array,
    Bitmask,
    Boolean,
    Date,
    Dictionary,
    Integer,
    Matrix,
    Name,
    NameTree,
    Null,
    Number,
    NumberTree,
    Rectangle,
    Stream,
    String,
    StringAscii,
    StringByte,
    StringText,
}

impl Basic {
    fn parse(s: &str) -> Option<Basic> {
        Some(match s {
            "array" => Basic::Array,
            "bitmask" => Basic::Bitmask,
            "boolean" => Basic::Boolean,
            "date" => Basic::Date,
            "dictionary" => Basic::Dictionary,
            "integer" => Basic::Integer,
            "matrix" => Basic::Matrix,
            "name" => Basic::Name,
            "name-tree" => Basic::NameTree,
            "null" => Basic::Null,
            "number" => Basic::Number,
            "number-tree" => Basic::NumberTree,
            "rectangle" => Basic::Rectangle,
            "stream" => Basic::Stream,
            "string" => Basic::String,
            "string-ascii" => Basic::StringAscii,
            "string-byte" => Basic::StringByte,
            "string-text" => Basic::StringText,
            _ => return None,
        })
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Basic::Array => "array",
            Basic::Bitmask => "bitmask",
            Basic::Boolean => "boolean",
            Basic::Date => "date",
            Basic::Dictionary => "dictionary",
            Basic::Integer => "integer",
            Basic::Matrix => "matrix",
            Basic::Name => "name",
            Basic::NameTree => "name-tree",
            Basic::Null => "null",
            Basic::Number => "number",
            Basic::NumberTree => "number-tree",
            Basic::Rectangle => "rectangle",
            Basic::Stream => "stream",
            Basic::String => "string",
            Basic::StringAscii => "string-ascii",
            Basic::StringByte => "string-byte",
            Basic::StringText => "string-text",
        }
    }
}

/// When a type, value or link applies, from the version predicates that wrap it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Gate {
    Always,
    /// `fn:SinceVersion(v, x)`: from `v` on.
    Since(Version),
    /// `fn:BeforeVersion(v, x)`: before `v` only.
    Before(Version),
    /// `fn:IsPDFVersion(v, x)`: in `v` only.
    Only(Version),
    /// `fn:Deprecated(v, x)`: allowed, but deprecated from `v` on.
    Deprecated(Version),
    /// `fn:Extension(name, x)`: under an extension, which this validator does not track.
    Extension,
}

impl Gate {
    /// Whether the thing it gates belongs to `v` (a deprecated thing still does).
    pub(crate) fn admits(self, v: Version) -> bool {
        match self {
            Gate::Always | Gate::Deprecated(_) | Gate::Extension => true,
            Gate::Since(s) => v >= s,
            Gate::Before(b) => v < b,
            Gate::Only(o) => v == o,
        }
    }
}

/// Split `fn:SinceVersion(1.5,Crypt)`-style wrappers off an item: its gate and what it wraps.
/// Anything else is returned as it is with [`Gate::Always`].
pub(crate) fn gated(s: &str) -> (Gate, &str) {
    let s = s.trim();
    let Some((name, args)) = call(s) else { return (Gate::Always, s) };
    let parts = split_top(args, ',');
    match (name, parts.as_slice()) {
        ("Extension", [_, inner]) => (Gate::Extension, gated(inner).1),
        ("Extension", [_]) => (Gate::Extension, ""),
        (f @ ("SinceVersion" | "BeforeVersion" | "IsPDFVersion" | "Deprecated"), [v, inner]) => {
            let Some(v) = Version::parse(v) else { return (Gate::Always, s) };
            let (inner_gate, item) = gated(inner);
            if inner_gate == Gate::Extension {
                return (Gate::Extension, item);
            }
            let gate = match f {
                "SinceVersion" => Gate::Since(v),
                "BeforeVersion" => Gate::Before(v),
                "IsPDFVersion" => Gate::Only(v),
                _ => Gate::Deprecated(v),
            };
            (gate, item)
        }
        _ => (Gate::Always, s),
    }
}

/// `fn:Name(args)` → (`Name`, `args`) when the whole of `s` is one call.
pub(crate) fn call(s: &str) -> Option<(&str, &str)> {
    let rest = s.strip_prefix("fn:")?;
    let open = rest.find('(')?;
    let name = rest.get(..open)?;
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric()) || !rest.ends_with(')') {
        return None;
    }
    let args = rest.get(open + 1..rest.len() - 1)?;
    // The closing parenthesis must be the one that matches the opening one.
    let mut depth = 0i32;
    for c in args.chars() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth < 0 {
                    return None;
                }
            }
            _ => {}
        }
    }
    (depth == 0).then_some((name, args))
}

/// Split on `sep` outside brackets, parentheses and quoted strings.
pub(crate) fn split_top(s: &str, sep: char) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut depth, mut quoted, mut start) = (0i32, false, 0usize);
    for (i, c) in s.char_indices() {
        match c {
            '\'' => quoted = !quoted,
            '(' | '[' if !quoted => depth += 1,
            ')' | ']' if !quoted => depth -= 1,
            c if c == sep && depth == 0 && !quoted => {
                out.push(s.get(start..i).unwrap_or_default());
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    out.push(s.get(start..).unwrap_or_default());
    out
}

/// `[x]` → `x` when the brackets enclose all of `s`.
fn unbracket(s: &str) -> Option<&str> {
    let inner = s.strip_prefix('[')?.strip_suffix(']')?;
    let mut depth = 0i32;
    for c in inner.chars() {
        match c {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth < 0 {
                    return None;
                }
            }
            _ => {}
        }
    }
    (depth == 0).then_some(inner)
}

/// One part per alternative of a complex field (`[a];[b]`), brackets removed. A single
/// unbracketed value applies to every alternative; a field that doesn't fit the Type field
/// counts as empty rather than being guessed at.
fn per_alt(field: &'static str, n: usize) -> Vec<&'static str> {
    if field.is_empty() {
        return vec![""; n];
    }
    let parts = split_top(field, ';');
    if parts.len() == n && parts.iter().all(|p| unbracket(p).is_some()) {
        return parts.into_iter().map(|p| unbracket(p).unwrap_or_default()).collect();
    }
    if parts.len() == 1 {
        let one = unbracket(field).unwrap_or(field);
        return vec![one; n];
    }
    vec![""; n]
}

/// How a row's key reads (column Key).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KeyKind {
    Name,
    /// An array element at a fixed index.
    Index(usize),
    /// `N*`: one of a set of elements repeated from the first such index on.
    Repeat(usize),
    /// `*`: any other key or element.
    Wildcard,
}

/// When a key was introduced (column SinceVersion).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Since {
    Version(Version),
    /// Standard from this version, and earlier under an extension.
    ExtensionOr(Version),
    /// Only under an extension.
    Extension,
}

/// One alternative of a row: a type, with the parts of the other columns that go with it.
#[derive(Clone, Debug)]
pub(crate) struct Alt {
    pub basic: Basic,
    pub gate: Gate,
    /// Column IndirectReference for this alternative: `TRUE`, `FALSE` or a predicate.
    pub indirect: &'static str,
    /// Column PossibleValues for this alternative (brackets removed; empty when unconstrained).
    pub possible: &'static str,
    /// Column SpecialCase for this alternative.
    pub special: &'static str,
    /// Column Link: the types an array, dictionary, stream or tree value may have.
    pub links: Vec<(Gate, &'static str)>,
}

#[derive(Clone, Debug)]
pub(crate) struct Row {
    pub key: &'static str,
    pub kind: KeyKind,
    pub alts: Vec<Alt>,
    pub since: Since,
    pub deprecated: Option<Version>,
    /// Column Required: `TRUE`, `FALSE` or `fn:IsRequired(...)`.
    pub required: &'static str,
    pub inheritable: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Shape {
    Dict,
    Array,
}

#[derive(Clone, Debug)]
pub(crate) struct TypeDef {
    pub name: &'static str,
    pub rows: Vec<Row>,
    pub shape: Shape,
}

impl TypeDef {
    /// The row for array element `i`.
    pub(crate) fn row_for_index(&self, i: usize) -> Option<&Row> {
        if let Some(r) = self.rows.iter().find(|r| r.kind == KeyKind::Index(i)) {
            return Some(r);
        }
        let repeats: Vec<&Row> = self.rows.iter().filter(|r| matches!(r.kind, KeyKind::Repeat(_))).collect();
        if let Some(first) = repeats.iter().filter_map(|r| if let KeyKind::Repeat(n) = r.kind { Some(n) } else { None }).min()
            && i >= first
            && !repeats.is_empty()
        {
            return repeats.get((i - first) % repeats.len()).copied();
        }
        self.rows.iter().find(|r| r.kind == KeyKind::Wildcard)
    }

    pub(crate) fn has_wildcard(&self) -> bool {
        self.rows.iter().any(|r| r.kind == KeyKind::Wildcard)
    }
}

pub(crate) struct Model {
    pub types: HashMap<&'static str, TypeDef>,
    /// Rows the parser could not read (zero for the committed table; a test checks).
    pub skipped: usize,
}

/// The model, parsed on first use.
pub(crate) fn model() -> &'static Model {
    static MODEL: OnceLock<Model> = OnceLock::new();
    MODEL.get_or_init(|| Model::parse(TABLE))
}

impl Model {
    pub(crate) fn get(&self, name: &str) -> Option<&TypeDef> {
        self.types.get(name)
    }

    pub(crate) fn parse(table: &'static str) -> Model {
        let mut types: HashMap<&'static str, TypeDef> = HashMap::new();
        let mut skipped = 0;
        let mut current: Option<TypeDef> = None;
        for line in table.lines() {
            if line.starts_with('#') || line.is_empty() {
                continue;
            }
            if let Some(name) = line.strip_prefix('=') {
                if let Some(done) = current.take() {
                    types.insert(done.name, finish(done));
                }
                current = Some(TypeDef { name, rows: Vec::new(), shape: Shape::Dict });
                continue;
            }
            let Some(def) = current.as_mut() else {
                skipped += 1;
                continue;
            };
            match parse_row(line) {
                Some(row) => def.rows.push(row),
                None => skipped += 1,
            }
        }
        if let Some(done) = current.take() {
            types.insert(done.name, finish(done));
        }
        Model { types, skipped }
    }
}

/// Work out whether a type is an array or a dictionary once its rows are in.
fn finish(mut def: TypeDef) -> TypeDef {
    let indexed = def.rows.iter().any(|r| matches!(r.kind, KeyKind::Index(_) | KeyKind::Repeat(_)));
    let named = def.rows.iter().any(|r| r.kind == KeyKind::Name);
    // A lone `*` row is a map or an array; the model names its array types (INTERNAL_GRAMMAR.md).
    let array_name = def.name.starts_with("ArrayOf") || def.name.ends_with("Array") || def.name.ends_with("ColorSpace");
    def.shape = if indexed || (!named && array_name) { Shape::Array } else { Shape::Dict };
    def
}

fn parse_row(line: &'static str) -> Option<Row> {
    let f: Vec<&'static str> = line.split('\t').collect();
    let [key, ty, since, deprecated, required, indirect, inheritable, _default, possible, special, link] = f.as_slice() else {
        return None;
    };
    let kind = if *key == "*" {
        KeyKind::Wildcard
    } else if let Some(n) = key.strip_suffix('*').filter(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())) {
        KeyKind::Repeat(n.parse().ok()?)
    } else if !key.is_empty() && key.bytes().all(|b| b.is_ascii_digit()) {
        KeyKind::Index(key.parse().ok()?)
    } else if !key.is_empty() {
        KeyKind::Name
    } else {
        return None;
    };
    let types: Vec<(Gate, Basic)> = split_top(ty, ';')
        .into_iter()
        .map(|t| {
            let (gate, inner) = gated(t);
            Basic::parse(inner).map(|b| (gate, b))
        })
        .collect::<Option<_>>()?;
    let n = types.len();
    if n == 0 {
        return None;
    }
    let indirect = per_alt(indirect, n);
    let possible = per_alt(possible, n);
    let special = per_alt(special, n);
    let links = per_alt(link, n);
    let alts = types
        .into_iter()
        .enumerate()
        .map(|(i, (gate, basic))| Alt {
            basic,
            gate,
            indirect: indirect.get(i).copied().unwrap_or("FALSE"),
            possible: possible.get(i).copied().unwrap_or_default(),
            special: special.get(i).copied().unwrap_or_default(),
            links: links
                .get(i)
                .filter(|l| !l.is_empty())
                .map(|l| split_top(l, ',').into_iter().map(gated).filter(|(_, name)| !name.is_empty()).collect())
                .unwrap_or_default(),
        })
        .collect();
    Some(Row {
        key,
        kind,
        alts,
        since: parse_since(since)?,
        deprecated: if deprecated.is_empty() { None } else { Some(Version::parse(deprecated)?) },
        required,
        inheritable: *inheritable == "TRUE",
    })
}

/// Column SinceVersion: a version, `fn:Extension(name[,v])`, or
/// `fn:Eval(fn:Extension(name,v) || v2)`.
fn parse_since(s: &str) -> Option<Since> {
    if let Some(v) = Version::parse(s) {
        return Some(Since::Version(v));
    }
    if let Some(("Extension", _)) = call(s) {
        return Some(Since::Extension);
    }
    if let Some(("Eval", inner)) = call(s) {
        let standard = inner.split(" || ").filter_map(Version::parse).min();
        return Some(standard.map_or(Since::Extension, Since::ExtensionOr));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_parse_and_print() {
        assert_eq!(Version::parse("1.7"), Some(Version(17)));
        assert_eq!(Version::parse("2.0"), Some(Version::V2_0));
        assert_eq!(Version(14).to_string(), "1.4");
        for bad in ["", "1", "1.", ".7", "1.77", "0.9", "x.y", "1.7.1", "-1.7"] {
            assert_eq!(Version::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn gates_unwrap_version_predicates() {
        assert_eq!(gated("Crypt"), (Gate::Always, "Crypt"));
        assert_eq!(gated("fn:SinceVersion(1.5,Crypt)"), (Gate::Since(Version(15)), "Crypt"));
        assert_eq!(gated("fn:Deprecated(1.4,Compatible)"), (Gate::Deprecated(Version(14)), "Compatible"));
        assert_eq!(gated("fn:IsPDFVersion(1.7,fn:Extension(ADBE_Extn3,A))"), (Gate::Extension, "A"));
        assert_eq!(gated("fn:BeforeVersion(2.0,X)"), (Gate::Before(Version::V2_0), "X"));
        // Not a version gate: kept whole.
        assert_eq!(gated("fn:Eval(@A>0)").0, Gate::Always);
        assert_eq!(gated("fn:SinceVersion(oops,X)").0, Gate::Always);
    }

    #[test]
    fn complex_fields_split_per_alternative() {
        assert_eq!(per_alt("[A,B];[C]", 2), vec!["A,B", "C"]);
        assert_eq!(per_alt("[Catalog]", 1), vec!["Catalog"]);
        assert_eq!(per_alt("FALSE", 3), vec!["FALSE"; 3]);
        assert_eq!(per_alt("fn:MustBeDirect()", 2), vec!["fn:MustBeDirect()"; 2]);
        assert_eq!(per_alt("[[0 1],[1 0]];[X]", 2), vec!["[0 1],[1 0]", "X"]);
        // Too many parts for the Type field: nothing is guessed.
        assert_eq!(per_alt("[A];[B];[C]", 2), vec![""; 2]);
        assert_eq!(split_top("a,fn:F(b,c),'d,e'", ','), vec!["a", "fn:F(b,c)", "'d,e'"]);
    }

    #[test]
    fn calls_must_cover_the_whole_text() {
        assert_eq!(call("fn:Eval(@A>0)"), Some(("Eval", "@A>0")));
        assert_eq!(call("fn:A(x) || fn:B(y)"), None);
        assert_eq!(call("fn:A(x"), None);
        assert_eq!(call("A(x)"), None);
    }

    /// The committed table reads without a skipped row, and the types the walk starts from are
    /// there with the shapes the model gives them.
    #[test]
    fn the_committed_model_parses_whole() {
        let m = model();
        assert_eq!(m.skipped, 0);
        assert_eq!(m.types.len(), 613);
        let trailer = m.get("FileTrailer").unwrap();
        assert_eq!(trailer.shape, Shape::Dict);
        assert!(trailer.rows.iter().any(|r| r.key == "Root" && r.required == "TRUE"));
        assert_eq!(m.get("ArrayOfAnnots").unwrap().shape, Shape::Array);
        assert_eq!(m.get("DestXYZArray").unwrap().shape, Shape::Array);
        assert_eq!(m.get("DestsMap").unwrap().shape, Shape::Dict);
        let catalog = m.get("Catalog").unwrap();
        let open = catalog.rows.iter().find(|r| r.key == "OpenAction").unwrap();
        assert_eq!(open.alts.len(), 2);
        assert!(open.alts[1].links.iter().any(|(g, n)| *n == "ActionGoToE" && *g == Gate::Since(Version(16))));
        // Every link names a type the model has.
        for def in m.types.values() {
            for row in &def.rows {
                for alt in &row.alts {
                    for (_, link) in &alt.links {
                        assert!(m.get(link).is_some(), "{}::{} links to unknown {link}", def.name, row.key);
                    }
                }
            }
        }
    }
}
