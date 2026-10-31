//! The walk: from the trailer through every object the model links to, each checked against the
//! model type the link gives it.
//!
//! A key's Link column lists the types its value may have (`[ActionGoTo,ActionURI,…]`). Which one
//! applies is read off the object: the candidate whose fixed values (`/Type`, `/Subtype`, `/S`,
//! an array's first elements) and special-case rules the object meets, and whose keys it has.
//! When every candidate is contradicted by the object, the object is reported as matching none of
//! them and is not checked further, so one odd object doesn't turn into a cascade of findings.
//!
//! Objects are visited breadth-first, so a finding's path is the shortest way to it, and each
//! object is checked once per type (a merged form field and widget is checked as both).

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use crate::expr::{self, Env, Eval, Found, Mode, single_call};
use crate::model::{self, Alt, Basic, Gate, KeyKind, Model, Row, Shape, Since, TypeDef, Version, gated, split_top};
use crate::{Finding, Object, Options, Ref, Report, Rule, Severity, Source};

pub(crate) const W_REQUIRED: &str = "{type} lacks the required entry {key}";
pub(crate) const W_TYPE: &str = "{key} in {type} is {found}, but must be {expected}";
pub(crate) const W_VALUE: &str = "{key} in {type} is {value}, which is not one of {allowed}";
pub(crate) const W_VALUE_RULE: &str = "{key} in {type} is {value}, which breaks the rule {rule}";
pub(crate) const W_REQUIRED_VALUE: &str = "{key} in {type} must be {expected} when {rule}";
pub(crate) const W_INDIRECT: &str = "{key} in {type} must be an indirect reference";
pub(crate) const W_DIRECT: &str = "{key} in {type} must be a direct object, not a reference";
pub(crate) const W_SINCE: &str = "{key} in {type} is from PDF {since}, later than the file's PDF {version}";
pub(crate) const W_DEPRECATED: &str = "{key} in {type} is deprecated since PDF {since}";
pub(crate) const W_VALUE_SINCE: &str = "The value {value} of {key} in {type} is from PDF {since}, later than the file's PDF {version}";
pub(crate) const W_VALUE_DEPRECATED: &str = "The value {value} of {key} in {type} is deprecated since PDF {since}";
pub(crate) const W_TYPE_SINCE: &str = "{key} in {type} is a {found}, which is from PDF {since}, later than the file's PDF {version}";
pub(crate) const W_CONDITION: &str = "{key} in {type} breaks the rule {rule}";
pub(crate) const W_TOO_MANY: &str = "{type} has {count} elements, more than the {max} it allows";
pub(crate) const W_SETS: &str = "{type} has {count} elements, but they come in sets of {n}";
pub(crate) const W_MISSING_OBJECT: &str = "{key} in {type} refers to object {object}, which doesn't exist";
pub(crate) const W_NO_TYPE: &str = "{key} in {type} matches none of the types it may have ({candidates})";
pub(crate) const W_UNKNOWN_KEY: &str = "{key} is not an entry of {type} in the model";

/// Direct objects nested deeper than this inside one indirect object aren't followed.
const MAX_NESTING: usize = 64;
/// Name and number trees deeper than this aren't followed.
const MAX_TREE_DEPTH: usize = 64;
/// Elements (or entries) of one array (or dictionary) checked; the rest are noted as unchecked.
const MAX_ELEMENTS: usize = 50_000;
/// Indirect objects kept loaded for predicates that look elsewhere in the document.
const MAX_CACHED: usize = 100_000;
/// `/Parent` levels followed for inherited keys.
const MAX_INHERIT: usize = 64;
/// Paths longer than this keep only their end.
const MAX_PATH: usize = 160;
/// Work items between two calls of [`validate_with_progress`]'s callback: often enough to stop
/// within milliseconds, seldom enough to cost nothing.
const PROGRESS_EVERY: usize = 512;

/// The fields of a form, whose dictionaries are often merged with their widget annotation.
const FIELD_TYPES: &[&str] = &["FieldTx", "FieldBtnPush", "FieldBtnCheckbox", "FieldBtnRadio", "FieldChoice", "FieldSig", "Field"];

/// Up to three containing objects, nearest first, for `parent::` paths.
type Parents = Vec<Arc<Object>>;

enum Work {
    Object(Item),
    Tree(TreeItem),
}

/// An object to check as a model type.
struct Item {
    /// As found in its container: a reference, or the direct object.
    value: Object,
    ty: &'static str,
    path: String,
    holder: Option<Ref>,
    page: Option<usize>,
    rect: Option<[f64; 4]>,
    parents: Parents,
    nesting: usize,
    /// The version already reported for the key or link that led here: what it leads to is at
    /// least as new, so its own keys aren't reported again up to that version.
    newer: Option<Version>,
}

/// A node of a name or number tree whose leaves have one of `links` as their type.
struct TreeItem {
    node: Object,
    numbers: bool,
    links: Vec<(Gate, &'static str)>,
    /// The tree's key and the type holding it, for findings.
    key: String,
    ty: &'static str,
    path: String,
    holder: Option<Ref>,
    page: Option<usize>,
    parents: Parents,
    depth: usize,
    newer: Option<Version>,
}

/// The object being checked, and where it is.
struct Here<'a> {
    def: &'static TypeDef,
    obj: &'a Arc<Object>,
    path: &'a str,
    holder: Option<Ref>,
    page: Option<usize>,
    rect: Option<[f64; 4]>,
    parents: &'a [Arc<Object>],
    nesting: usize,
    newer: Option<Version>,
}

/// A value with references followed.
enum Resolved<'a> {
    Direct(&'a Object),
    Indirect(Ref, Arc<Object>),
    Missing(Ref),
}

impl Resolved<'_> {
    fn object(&self) -> Option<&Object> {
        match self {
            Resolved::Direct(o) => Some(o),
            Resolved::Indirect(_, o) => Some(o),
            Resolved::Missing(_) => None,
        }
    }
}

struct Walker<'s> {
    src: &'s dyn Source,
    model: &'static Model,
    opts: &'s Options,
    version: Version,
    trailer: Arc<Object>,
    queue: VecDeque<Work>,
    seen: HashSet<(u32, &'static str)>,
    cache: RefCell<HashMap<u32, Option<Arc<Object>>>>,
    exprs: RefCell<HashMap<&'static str, Option<Arc<expr::Expr>>>>,
    not_checked: RefCell<HashMap<String, usize>>,
    /// While choosing among candidate types, predicates that can't be evaluated aren't counted.
    quiet: std::cell::Cell<bool>,
    findings: [Vec<Finding>; 3],
    totals: [usize; 3],
    objects: usize,
    stopped: bool,
}

/// Validate the document `src` against the Arlington model.
pub fn validate(src: &dyn Source, opts: &Options) -> Report {
    walk(src, opts, &mut |_| true).0
}

/// [`validate`], calling `progress` every few hundred objects with how many have been checked so
/// far. When it returns `false` the walk stops and there is no report: for callers that validate
/// on a worker thread and let the user cancel.
pub fn validate_with_progress(src: &dyn Source, opts: &Options, progress: &mut dyn FnMut(usize) -> bool) -> Option<Report> {
    let (report, finished) = walk(src, opts, progress);
    finished.then_some(report)
}

/// The walk, and whether it finished (`false`: `progress` stopped it).
fn walk(src: &dyn Source, opts: &Options, progress: &mut dyn FnMut(usize) -> bool) -> (Report, bool) {
    let model = model::model();
    let trailer = Arc::new(match src.trailer() {
        d @ Object::Dict(_) => d,
        _ => Object::Dict(Vec::new()),
    });
    let mut w = Walker {
        src,
        model,
        opts,
        version: Version::V2_0,
        trailer: trailer.clone(),
        queue: VecDeque::new(),
        seen: HashSet::new(),
        cache: RefCell::new(HashMap::new()),
        exprs: RefCell::new(HashMap::new()),
        not_checked: RefCell::new(HashMap::new()),
        quiet: std::cell::Cell::new(false),
        findings: [Vec::new(), Vec::new(), Vec::new()],
        totals: [0; 3],
        objects: 0,
        stopped: false,
    };
    // The catalog's /Version overrides the header when it is later (ISO 32000-2 §7.2.2).
    let catalog_version = trailer.get(b"Root").and_then(|r| w.resolve(r).object().and_then(|c| c.get(b"Version").cloned())).and_then(|v| {
        match w.resolve(&v).object() {
            Some(Object::Name(n)) => std::str::from_utf8(n).ok().and_then(Version::parse),
            _ => None,
        }
    });
    let (version, assumed) = match (src.header_version(), catalog_version) {
        (Some(h), Some(c)) => (h.max(c), false),
        (Some(v), None) | (None, Some(v)) => (v, false),
        (None, None) => (Version::V2_0, true),
    };
    w.version = version;
    if model.skipped > 0 {
        // The committed table always reads whole (a test checks); a damaged build is said so.
        w.not_checked.borrow_mut().insert("model rows that could not be read".into(), model.skipped);
    }
    w.queue.push_back(Work::Object(Item {
        value: (*trailer).clone(),
        ty: "FileTrailer",
        path: "trailer".into(),
        holder: None,
        page: None,
        rect: None,
        parents: Vec::new(),
        nesting: 0,
        newer: None,
    }));
    let (mut popped, mut finished) = (0usize, true);
    while let Some(work) = w.queue.pop_front() {
        if w.objects >= opts.max_objects {
            w.stopped = true;
            break;
        }
        popped = popped.wrapping_add(1);
        if popped.is_multiple_of(PROGRESS_EVERY) && !progress(w.objects) {
            finished = false;
            break;
        }
        match work {
            Work::Object(item) => w.visit(item),
            Work::Tree(t) => w.tree_node(t),
        }
    }
    let mut not_checked: Vec<(String, usize)> = w.not_checked.into_inner().into_iter().collect();
    not_checked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let [errors, warnings, infos] = w.totals;
    let truncated = w.totals.iter().zip(&w.findings).any(|(t, f)| *t > f.len());
    let report = Report {
        version,
        version_assumed: assumed,
        findings: w.findings.into_iter().flatten().collect(),
        errors,
        warnings,
        infos,
        objects: w.objects,
        not_checked,
        stopped_early: w.stopped,
        truncated,
    };
    (report, finished)
}

impl Walker<'_> {
    fn load(&self, r: Ref) -> Option<Arc<Object>> {
        if let Some(hit) = self.cache.borrow().get(&r.num) {
            return hit.clone();
        }
        let loaded = self.src.get(r).map(Arc::new);
        let mut cache = self.cache.borrow_mut();
        if cache.len() < MAX_CACHED {
            cache.insert(r.num, loaded.clone());
        }
        loaded
    }

    fn resolve<'a>(&self, o: &'a Object) -> Resolved<'a> {
        match o {
            Object::Ref(r) => match self.load(*r) {
                // A reference to a reference is not a PDF object; it reads as nothing.
                Some(obj) if !matches!(*obj, Object::Ref(_)) => Resolved::Indirect(*r, obj),
                _ => Resolved::Missing(*r),
            },
            other => Resolved::Direct(other),
        }
    }

    fn note(&self, what: &str) {
        if self.quiet.get() {
            return;
        }
        *self.not_checked.borrow_mut().entry(what.to_string()).or_insert(0) += 1;
    }

    fn parsed(&self, text: &'static str) -> Option<Arc<expr::Expr>> {
        if let Some(hit) = self.exprs.borrow().get(text) {
            return hit.clone();
        }
        let e = expr::parse(text).map(Arc::new);
        if e.is_none() {
            self.note("unreadable predicate");
        }
        self.exprs.borrow_mut().insert(text, e.clone());
        e
    }

    /// Evaluate `text` with `holder` as the object holding the row and `current` as its value.
    fn eval(&self, text: &'static str, mode: Mode, holder: &Object, parents: &[Arc<Object>], current: Option<&Object>) -> Option<bool> {
        let e = self.parsed(text)?;
        let env = Ctx { w: self, holder, parents, current };
        Eval { env: &env, mode }.truth(&e)
    }

    fn report(&mut self, here: &Here, severity: Severity, rule: Rule, key: &str, wording: &'static str, mut args: Vec<(&'static str, String)>) {
        let at = match severity {
            Severity::Error => 0,
            Severity::Warning => 1,
            Severity::Info => 2,
        };
        if let Some(total) = self.totals.get_mut(at) {
            *total += 1;
        }
        let Some(list) = self.findings.get_mut(at) else { return };
        if list.len() >= self.opts.max_findings {
            return;
        }
        args.push(("type", here.def.name.to_string()));
        args.push(("key", key.to_string()));
        list.push(Finding {
            severity,
            rule,
            object: here.holder,
            page: here.page,
            rect: here.rect,
            path: here.path.to_string(),
            type_name: here.def.name.to_string(),
            key: key.to_string(),
            wording,
            args,
        });
    }

    // ── objects ────────────────────────────────────────────────────────────────────────────

    fn visit(&mut self, item: Item) {
        let Item { value, ty, path, holder, page, rect, parents, nesting, newer } = item;
        let (obj, holder, mut page, nesting) = match value {
            Object::Ref(r) => {
                if !self.seen.insert((r.num, ty)) {
                    return;
                }
                let Some(obj) = self.load(r) else { return };
                (obj, Some(r), self.src.page_index(r).or(page), 0)
            }
            other => (Arc::new(other), holder, page, nesting),
        };
        let Some(def) = self.model.get(ty) else {
            self.note("type missing from the model");
            return;
        };
        self.objects += 1;
        // An annotation names its page (/P), which may be met before its page is.
        if page.is_none()
            && let Some(Object::Ref(p)) = obj.get(b"P")
        {
            page = self.src.page_index(*p);
        }
        let rect = if def.name.starts_with("Annot") { obj.get(b"Rect").and_then(|r| self.rect(r)) } else { None }.or(rect);
        let here = Here { def, obj: &obj, path: &path, holder, page, rect, parents: &parents, nesting, newer };
        match (&*obj, def.shape) {
            (Object::Dict(_) | Object::Stream(_), Shape::Dict) => self.check_dict(&here),
            (Object::Array(items), Shape::Array) => self.check_array(&here, items),
            _ => {}
        }
    }

    fn rect(&self, o: &Object) -> Option<[f64; 4]> {
        let Object::Array(a) = self.resolve(o).object()?.clone() else { return None };
        let n: Vec<f64> = a.iter().map(|v| number(self.resolve(v).object()?)).collect::<Option<_>>()?;
        let [x0, y0, x1, y1] = n.as_slice() else { return None };
        Some([x0.min(*x1), y0.min(*y1), x0.max(*x1), y0.max(*y1)])
    }

    fn check_dict(&mut self, here: &Here) {
        let Some(entries) = here.obj.entries() else { return };
        let def = here.def;
        for row in def.rows.iter().filter(|r| r.kind == KeyKind::Name) {
            match entries.iter().find(|(k, _)| k.as_slice() == row.key.as_bytes()) {
                Some((_, v)) => self.check_value(here, row, row.key, v, false),
                None => self.check_absent(here, row, row.key, None),
            }
        }
        let wildcard = def.rows.iter().find(|r| r.kind == KeyKind::Wildcard);
        for (n, (k, v)) in entries.iter().enumerate() {
            if n >= MAX_ELEMENTS {
                self.note("entries past the limit");
                break;
            }
            if def.rows.iter().any(|r| r.kind == KeyKind::Name && r.key.as_bytes() == k.as_slice()) || matches!(v, Object::Null) {
                continue;
            }
            let key = String::from_utf8_lossy(k);
            match wildcard {
                Some(row) => self.check_value(here, row, &key, v, false),
                None if self.opts.unknown_keys && !self.companion_knows(here, k) => {
                    self.report(here, Severity::Info, Rule::UnknownKey, &key, W_UNKNOWN_KEY, Vec::new());
                }
                None => {}
            }
        }
    }

    /// A form field and its widget annotation are often one dictionary: each type's keys are at
    /// home in the other.
    fn companion_knows(&self, here: &Here, key: &[u8]) -> bool {
        let knows = |ty: &str| self.model.get(ty).is_some_and(|d| d.rows.iter().any(|r| r.kind == KeyKind::Name && r.key.as_bytes() == key));
        let widget = matches!(here.obj.get(b"Subtype"), Some(Object::Name(n)) if n.as_slice() == b"Widget");
        if FIELD_TYPES.contains(&here.def.name) && widget {
            return knows("AnnotWidget");
        }
        if here.def.name == "AnnotWidget" {
            return FIELD_TYPES.iter().any(|t| knows(t));
        }
        false
    }

    fn check_array(&mut self, here: &Here, items: &[Object]) {
        let def = here.def;
        let len = items.len();
        // `N*` rows: a set of elements repeated from the first such index on.
        let repeats: Vec<&'static Row> = def.rows.iter().filter(|r| matches!(r.kind, KeyKind::Repeat(_))).collect();
        let first = repeats.iter().filter_map(|r| if let KeyKind::Repeat(n) = r.kind { Some(n) } else { None }).min();
        // The row of the repeating set the next element starts looking at.
        let mut at = 0;
        for (i, v) in items.iter().enumerate() {
            if i >= MAX_ELEMENTS {
                self.note("elements past the limit");
                break;
            }
            let row = match first {
                Some(first) if i >= first && !def.rows.iter().any(|r| r.kind == KeyKind::Index(i)) => self.repeat_row(&repeats, &mut at, v),
                _ => def.row_for_index(i),
            };
            match row {
                Some(row) => self.check_value(here, row, &i.to_string(), v, true),
                None => {
                    let max = def.rows.iter().filter(|r| matches!(r.kind, KeyKind::Index(_))).count();
                    self.report(
                        here,
                        Severity::Warning,
                        Rule::ArrayLength,
                        "",
                        W_TOO_MANY,
                        vec![("count", len.to_string()), ("max", max.to_string())],
                    );
                    break;
                }
            }
        }
        for row in &def.rows {
            if let KeyKind::Index(i) = row.kind
                && i >= len
            {
                self.check_absent(here, row, row.key, None);
            }
        }
        // A set whose rows are all required comes whole (INTERNAL_GRAMMAR.md, column Key); one
        // with optional rows may leave them out.
        if let Some(first) = first
            && !repeats.is_empty()
            && repeats.iter().all(|r| r.required == "TRUE")
            && len > first
            && !(len - first).is_multiple_of(repeats.len())
        {
            self.report(here, Severity::Error, Rule::ArrayLength, "", W_SETS, vec![("count", len.to_string()), ("n", repeats.len().to_string())]);
        }
    }

    /// The row of the repeating set `repeats` that element `v` is, `*at` being the row after the
    /// previous element's. A row not marked required may be left out, as an attribute object's
    /// revision number is: when `v` doesn't fit it, the rows after it are tried, up to the next
    /// required one. When none fits, `v` is checked against the row it should have been.
    fn repeat_row(&self, repeats: &[&'static Row], at: &mut usize, v: &Object) -> Option<&'static Row> {
        let n = repeats.len();
        let start = at.checked_rem(n)?;
        let value = self.resolve(v);
        let fits = |row: &'static Row| value.object().is_none_or(|o| pick_alt(row, o, self.version).is_some());
        for step in 0..n {
            let k = start.saturating_add(step) % n;
            let row = repeats.get(k).copied()?;
            if fits(row) {
                *at = k.saturating_add(1);
                return Some(row);
            }
            if row.required == "TRUE" {
                break;
            }
        }
        *at = start.saturating_add(1);
        repeats.get(start).copied()
    }

    /// A key the object doesn't have (or whose value is null, or a reference to nothing).
    fn check_absent(&mut self, here: &Here, row: &'static Row, key: &str, missing: Option<Ref>) {
        if let Some(r) = missing {
            self.report(here, Severity::Warning, Rule::MissingObject, key, W_MISSING_OBJECT, vec![("object", r.to_string())]);
        }
        // Not required before the version that introduced it, nor under an extension.
        match row.since {
            Since::Version(v) | Since::ExtensionOr(v) if v > self.version => return,
            Since::Extension => return,
            _ => {}
        }
        let required = match row.required {
            "TRUE" => Some(true),
            "FALSE" => Some(false),
            text => match single_call(text, "IsRequired") {
                Some(_) => self.eval(text, Mode::Condition, here.obj, here.parents, None),
                None => {
                    self.note("unreadable predicate");
                    None
                }
            },
        };
        if required != Some(true) {
            return;
        }
        if row.inheritable && (self.inherited(here.obj, row.key.as_bytes()) || self.form_default(here, row.key)) {
            return;
        }
        self.report(here, Severity::Error, Rule::Required, key, W_REQUIRED, Vec::new());
    }

    /// A field's /DA and /Q inherit, at the end of the /Parent chain, from the interactive form
    /// dictionary's document-wide defaults (ISO 32000-2, the form dictionary's DA and Q).
    fn form_default(&self, here: &Here, key: &str) -> bool {
        if !FIELD_TYPES.contains(&here.def.name) || !matches!(key, "DA" | "Q") {
            return false;
        }
        let path: Vec<String> = ["trailer", "Root", "AcroForm", key].into_iter().map(String::from).collect();
        let ctx = Ctx { w: self, holder: here.obj, parents: &[], current: None };
        matches!(ctx.find(&path), Found::Present(_))
    }

    /// Whether an ancestor (`/Parent`, as for pages) supplies `key`.
    fn inherited(&self, obj: &Object, key: &[u8]) -> bool {
        let mut seen = HashSet::new();
        let mut at = obj.get(b"Parent").cloned();
        for _ in 0..MAX_INHERIT {
            let Some(Object::Ref(r)) = at else { return false };
            if !seen.insert(r.num) {
                return false;
            }
            let Some(parent) = self.load(r) else { return false };
            if parent.get(key).is_some_and(|v| !matches!(v, Object::Null)) {
                return true;
            }
            at = parent.get(b"Parent").cloned();
        }
        false
    }

    fn check_value(&mut self, here: &Here, row: &'static Row, key: &str, raw: &Object, in_array: bool) {
        let resolved = self.resolve(raw);
        let (value, reference) = match &resolved {
            Resolved::Missing(r) => {
                let r = *r;
                return self.check_absent(here, row, key, Some(r));
            }
            Resolved::Direct(o) => (*o, None),
            Resolved::Indirect(r, o) => (&**o, Some(*r)),
        };
        // A dictionary entry whose value is null is absent (ISO 32000-2 §7.3.7); an array
        // element is not, and the model says where null may stand.
        if matches!(value, Object::Null) && !in_array {
            return self.check_absent(here, row, key, None);
        }
        let version = self.version;
        // What the value leads to is at least as new as anything said of it here.
        let mut newer = here.newer;
        match row.since {
            Since::Version(v) if v > version => {
                if !covers(newer, v) {
                    self.report(
                        here,
                        Severity::Warning,
                        Rule::Version,
                        key,
                        W_SINCE,
                        vec![("since", v.to_string()), ("version", version.to_string())],
                    );
                }
                newer = newer.max(Some(v));
            }
            Since::ExtensionOr(v) if v > version => self.note("Extension"),
            Since::Extension => self.note("Extension"),
            _ => {}
        }
        if let Some(d) = row.deprecated
            && version >= d
        {
            self.report(here, Severity::Warning, Rule::Deprecated, key, W_DEPRECATED, vec![("since", d.to_string())]);
        }
        let Some(alt) = pick_alt(row, value, version) else {
            let expected: Vec<&str> = row.alts.iter().map(|a| a.basic.as_str()).collect();
            self.report(
                here,
                Severity::Error,
                Rule::Type,
                key,
                W_TYPE,
                vec![("found", kind(value).to_string()), ("expected", expected.join(" or "))],
            );
            return;
        };
        match alt.gate {
            Gate::Since(v) | Gate::Only(v) if v > version => {
                if !covers(newer, v) {
                    self.report(
                        here,
                        Severity::Warning,
                        Rule::Version,
                        key,
                        W_TYPE_SINCE,
                        vec![("found", alt.basic.as_str().to_string()), ("since", v.to_string()), ("version", version.to_string())],
                    );
                }
                newer = newer.max(Some(v));
            }
            Gate::Extension => self.note("Extension"),
            _ => {}
        }
        self.check_indirect(here, alt, key, reference.is_some());
        self.check_format(here, alt, key, value);
        self.check_possible(here, alt, key, value, newer);
        self.check_special(here, alt, key, value);
        match alt.basic {
            Basic::Array | Basic::Dictionary | Basic::Stream if !alt.links.is_empty() => self.follow(here, alt, key, raw, value, reference, newer),
            Basic::NameTree | Basic::NumberTree if !alt.links.is_empty() && (here.nesting < MAX_NESTING || reference.is_some()) => {
                self.queue.push_back(Work::Tree(TreeItem {
                    node: raw.clone(),
                    numbers: alt.basic == Basic::NumberTree,
                    links: alt.links.clone(),
                    key: key.to_string(),
                    ty: here.def.name,
                    path: child_path(here.path, key),
                    holder: here.holder,
                    page: here.page,
                    parents: parents_of(here),
                    depth: 0,
                    newer,
                }));
            }
            _ => {}
        }
    }

    /// Queue `value` for checking as whichever of `alt`'s linked types it is.
    #[allow(clippy::too_many_arguments)]
    fn follow(&mut self, here: &Here, alt: &Alt, key: &str, raw: &Object, value: &Object, reference: Option<Ref>, mut newer: Option<Version>) {
        if reference.is_none() && here.nesting >= MAX_NESTING {
            self.note("nesting past the limit");
            return;
        }
        match self.select(&alt.links, value) {
            Pick::Type(ty, gate) => {
                if let Gate::Since(v) | Gate::Only(v) = gate
                    && v > self.version
                {
                    if !covers(newer, v) {
                        self.report(
                            here,
                            Severity::Warning,
                            Rule::Version,
                            key,
                            W_TYPE_SINCE,
                            vec![("found", ty.to_string()), ("since", v.to_string()), ("version", self.version.to_string())],
                        );
                    }
                    newer = newer.max(Some(v));
                }
                self.queue.push_back(Work::Object(Item {
                    value: raw.clone(),
                    ty,
                    path: child_path(here.path, key),
                    holder: here.holder,
                    page: here.page,
                    rect: here.rect,
                    parents: parents_of(here),
                    nesting: here.nesting + 1,
                    newer,
                }));
            }
            Pick::Ambiguous(names) => {
                let shown: Vec<&str> = names.iter().take(6).copied().collect();
                let more = if names.len() > shown.len() { ", …" } else { "" };
                self.report(here, Severity::Warning, Rule::UnknownType, key, W_NO_TYPE, vec![("candidates", format!("{}{more}", shown.join(", ")))]);
            }
            Pick::Nothing => {}
        }
    }

    fn tree_node(&mut self, t: TreeItem) {
        if t.depth > MAX_TREE_DEPTH {
            self.note("tree depth past the limit");
            return;
        }
        let node = match self.resolve(&t.node) {
            Resolved::Direct(o) => Arc::new(o.clone()),
            Resolved::Indirect(r, o) => {
                if !self.seen.insert((r.num, "(tree node)")) {
                    return;
                }
                o
            }
            Resolved::Missing(_) => return,
        };
        self.objects += 1;
        let Some(def) = self.model.get(t.ty) else { return };
        let holder = match &t.node {
            Object::Ref(r) => Some(*r),
            _ => t.holder,
        };
        let parents: Parents = std::iter::once(node.clone()).chain(t.parents.iter().take(2).cloned()).collect();
        let here = Here { def, obj: &node, path: &t.path, holder, page: t.page, rect: None, parents: &t.parents, nesting: 0, newer: t.newer };
        if node.entries().is_none() {
            self.report(
                &here,
                Severity::Error,
                Rule::Type,
                &t.key,
                W_TYPE,
                vec![("found", kind(&node).to_string()), ("expected", "dictionary".into())],
            );
            return;
        }
        if let Some(kids) = node.get(b"Kids") {
            let kids = match self.resolve(kids).object() {
                Some(Object::Array(a)) => a.clone(),
                _ => Vec::new(),
            };
            for kid in kids.into_iter().take(MAX_ELEMENTS) {
                self.queue.push_back(Work::Tree(TreeItem {
                    node: kid,
                    numbers: t.numbers,
                    links: t.links.clone(),
                    key: t.key.clone(),
                    ty: t.ty,
                    path: t.path.clone(),
                    holder,
                    page: t.page,
                    parents: t.parents.clone(),
                    depth: t.depth + 1,
                    newer: t.newer,
                }));
            }
        }
        let leaves_key: &[u8] = if t.numbers { b"Nums" } else { b"Names" };
        let Some(leaves) = node.get(leaves_key) else { return };
        let leaves = match self.resolve(leaves).object() {
            Some(Object::Array(a)) => a.clone(),
            _ => return,
        };
        if !leaves.len().is_multiple_of(2) {
            self.report(&here, Severity::Error, Rule::ArrayLength, &t.key, W_SETS, vec![("count", leaves.len().to_string()), ("n", "2".into())]);
        }
        for pair in leaves.chunks(2).take(MAX_ELEMENTS) {
            let [index, leaf] = pair else { continue };
            let label = match self.resolve(index).object() {
                Some(Object::String { bytes, .. }) if !t.numbers => String::from_utf8_lossy(bytes).into_owned(),
                Some(Object::Int(i)) if t.numbers => i.to_string(),
                other => {
                    let expected = if t.numbers { "integer" } else { "string" };
                    let found = other.map_or("nothing", kind);
                    self.report(&here, Severity::Error, Rule::Type, &t.key, W_TYPE, vec![("found", found.into()), ("expected", expected.into())]);
                    continue;
                }
            };
            let value = match self.resolve(leaf) {
                Resolved::Missing(_) => continue,
                r => match r.object() {
                    Some(Object::Null) | None => continue,
                    Some(o) => o.clone(),
                },
            };
            let path = child_path(&child_path(&t.path, &String::from_utf8_lossy(leaves_key)), &label);
            match self.select(&t.links, &value) {
                Pick::Type(ty, _) => self.queue.push_back(Work::Object(Item {
                    value: leaf.clone(),
                    ty,
                    path,
                    holder,
                    page: t.page,
                    rect: None,
                    parents: parents.clone(),
                    nesting: 0,
                    newer: t.newer,
                })),
                Pick::Ambiguous(names) => {
                    let shown: Vec<&str> = names.iter().take(6).copied().collect();
                    self.report(&here, Severity::Warning, Rule::UnknownType, &label, W_NO_TYPE, vec![("candidates", shown.join(", "))]);
                }
                Pick::Nothing => {
                    let expected: Vec<&str> = t.links.iter().map(|(_, n)| *n).take(6).collect();
                    self.report(
                        &here,
                        Severity::Error,
                        Rule::Type,
                        &label,
                        W_TYPE,
                        vec![("found", kind(&value).into()), ("expected", expected.join(" or "))],
                    );
                }
            }
        }
    }

    // ── the columns ────────────────────────────────────────────────────────────────────────

    fn check_indirect(&mut self, here: &Here, alt: &'static Alt, key: &str, is_ref: bool) {
        let text = alt.indirect.trim();
        // Some(true): must be an indirect reference; Some(false): must be direct.
        let must_be_indirect = match text {
            "TRUE" => Some(true),
            "FALSE" | "" => None,
            _ => {
                let (indirect, cond) = match (single_call(text, "MustBeDirect"), single_call(text, "MustBeIndirect")) {
                    (Some(c), _) => (false, c),
                    (_, Some(c)) => (true, c),
                    _ => {
                        self.note("unreadable predicate");
                        return;
                    }
                };
                let applies = cond.trim().is_empty() || self.eval(cond, Mode::Condition, here.obj, here.parents, None) == Some(true);
                applies.then_some(indirect)
            }
        };
        match must_be_indirect {
            Some(true) if !is_ref => self.report(here, Severity::Error, Rule::Indirect, key, W_INDIRECT, Vec::new()),
            Some(false) if is_ref => self.report(here, Severity::Error, Rule::Direct, key, W_DIRECT, Vec::new()),
            _ => {}
        }
    }

    /// The shapes behind some type names: a rectangle is four numbers, a matrix six, a date a
    /// date string (ISO 32000-2 §7.9.4), an ASCII string ASCII.
    fn check_format(&mut self, here: &Here, alt: &'static Alt, key: &str, value: &Object) {
        let bad = match (alt.basic, value) {
            (Basic::Rectangle, Object::Array(a)) => (a.len() != 4 || !self.all_numbers(a)).then(|| format!("an array of {} elements", a.len())),
            (Basic::Matrix, Object::Array(a)) => (a.len() != 6 || !self.all_numbers(a)).then(|| format!("an array of {} elements", a.len())),
            (Basic::StringAscii, Object::String { .. }) => (!well_formed(alt.basic, value)).then(|| "a string with non-ASCII bytes".to_string()),
            (Basic::Date, Object::String { .. }) => (!well_formed(alt.basic, value)).then(|| show(value)),
            _ => None,
        };
        if let Some(found) = bad {
            // A malformed date is common and harmless to readers; a malformed box is not.
            let severity = if alt.basic == Basic::Date { Severity::Warning } else { Severity::Error };
            self.report(here, severity, Rule::Type, key, W_TYPE, vec![("found", found), ("expected", alt.basic.as_str().to_string())]);
        }
    }

    fn all_numbers(&self, a: &[Object]) -> bool {
        a.iter().all(|v| self.resolve(v).object().and_then(number).is_some())
    }

    fn check_possible(&mut self, here: &Here, alt: &'static Alt, key: &str, value: &Object, newer: Option<Version>) {
        let list = alt.possible.trim();
        if list.is_empty() {
            return;
        }
        let version = self.version;
        let mut literals: Vec<(Gate, &str)> = Vec::new();
        let mut evals: Vec<&'static str> = Vec::new();
        let mut required: Vec<(Gate, &'static str, &'static str)> = Vec::new();
        let (mut wildcard, mut unknown) = (false, false);
        for item in split_top(alt.possible, ',') {
            let item = item.trim();
            if item == "*" {
                wildcard = true;
                continue;
            }
            if single_call(item, "Eval").is_some() {
                evals.push(item);
                continue;
            }
            let (gate, inner) = gated(item);
            if let Some(args) = single_call(inner, "RequiredValue") {
                if let [cond, lit] = split_top(args, ',').as_slice() {
                    required.push((gate, cond, lit));
                    literals.push((gate, lit));
                } else {
                    unknown = true;
                }
            } else if inner.starts_with("fn:") || inner.starts_with('[') {
                for name in expr::predicate_names(inner).take(1) {
                    self.note(name);
                }
                if inner.starts_with('[') {
                    self.note("array values");
                }
                unknown = true;
            } else {
                literals.push((gate, inner));
            }
        }
        let primitive = !matches!(value, Object::Array(_) | Object::Dict(_) | Object::Stream(_));
        let mut matched = false;
        if primitive {
            for (gate, lit) in &literals {
                if literal_matches(lit, value) != Some(true) {
                    continue;
                }
                matched = true;
                let shown = show(value);
                match *gate {
                    Gate::Since(v) | Gate::Only(v) if version < v && !covers(newer, v) => self.report(
                        here,
                        Severity::Warning,
                        Rule::Version,
                        key,
                        W_VALUE_SINCE,
                        vec![("value", shown), ("since", v.to_string()), ("version", version.to_string())],
                    ),
                    Gate::Deprecated(v) | Gate::Before(v) if version >= v => self.report(
                        here,
                        Severity::Warning,
                        Rule::Deprecated,
                        key,
                        W_VALUE_DEPRECATED,
                        vec![("value", shown), ("since", v.to_string())],
                    ),
                    Gate::Only(v) if version > v => self.report(
                        here,
                        Severity::Warning,
                        Rule::Deprecated,
                        key,
                        W_VALUE_DEPRECATED,
                        vec![("value", shown), ("since", v.to_string())],
                    ),
                    Gate::Extension => self.note("Extension"),
                    _ => {}
                }
                break;
            }
            if !matched && wildcard && matches!(value, Object::Name(_)) {
                matched = true;
            }
        }
        // A value whose condition holds is required to be exactly that value.
        for (gate, cond, lit) in &required {
            if !gate.admits(version) || !primitive {
                continue;
            }
            if self.eval(cond, Mode::Condition, here.obj, here.parents, Some(value)) == Some(true) && literal_matches(lit, value) == Some(false) {
                self.report(
                    here,
                    Severity::Error,
                    Rule::Value,
                    key,
                    W_REQUIRED_VALUE,
                    vec![("expected", show_literal(lit)), ("rule", cond.to_string())],
                );
                return;
            }
        }
        let mut failed = None;
        for e in &evals {
            match self.eval(e, Mode::Assertion, here.obj, here.parents, Some(value)) {
                Some(true) => matched = true,
                Some(false) => failed = failed.or(Some(*e)),
                None => unknown = true,
            }
        }
        if matched || unknown {
            return;
        }
        if let Some(rule) = failed
            && literals.is_empty()
        {
            let rule = single_call(rule, "Eval").unwrap_or(rule);
            self.report(here, Severity::Error, Rule::Value, key, W_VALUE_RULE, vec![("value", show(value)), ("rule", rule.to_string())]);
        } else if primitive && !literals.is_empty() && !wildcard {
            let shown: Vec<String> = literals.iter().take(8).map(|(_, l)| show_literal(l)).collect();
            let more = if literals.len() > shown.len() { ", …" } else { "" };
            self.report(
                here,
                Severity::Error,
                Rule::Value,
                key,
                W_VALUE,
                vec![("value", show(value)), ("allowed", format!("{}{more}", shown.join(", ")))],
            );
        }
    }

    fn check_special(&mut self, here: &Here, alt: &'static Alt, key: &str, value: &Object) {
        let text = alt.special.trim();
        if text.is_empty() {
            return;
        }
        let Some(rule) = single_call(alt.special.trim(), "Eval") else {
            // Ignore, IsMeaningful and the other non-assertions say when a rule applies, which
            // this validator doesn't act on.
            if let Some(name) = expr::predicate_names(text).next() {
                self.note(name);
            }
            return;
        };
        if self.eval(alt.special.trim(), Mode::Assertion, here.obj, here.parents, Some(value)) == Some(false) {
            self.report(here, Severity::Warning, Rule::Condition, key, W_CONDITION, vec![("rule", rule.to_string())]);
        }
    }

    // ── choosing a type ────────────────────────────────────────────────────────────────────

    fn select(&self, links: &[(Gate, &'static str)], value: &Object) -> Pick {
        let shape = match value {
            Object::Array(_) => Shape::Array,
            Object::Dict(_) | Object::Stream(_) => Shape::Dict,
            _ => return Pick::Nothing,
        };
        let candidates: Vec<(Gate, &'static TypeDef)> =
            links.iter().filter_map(|(g, n)| self.model.get(n).map(|d| (*g, d))).filter(|(_, d)| d.shape == shape).collect();
        match candidates.as_slice() {
            [] => return Pick::Nothing,
            [(g, d)] => return Pick::Type(d.name, *g),
            _ => {}
        }
        self.quiet.set(true);
        let scored: Vec<(i64, bool)> = candidates.iter().map(|(g, d)| self.score(*g, d, value)).collect();
        self.quiet.set(false);
        // A candidate the object contradicts is out however well its other keys fit: a widget
        // merged with its text field shares more keys with a free-text annotation than with
        // AnnotWidget, but its /Subtype says what it is.
        let mut best: Option<(usize, i64)> = None;
        for (i, (s, contradicted)) in scored.iter().enumerate() {
            if !contradicted && best.is_none_or(|(_, b)| *s > b) {
                best = Some((i, *s));
            }
        }
        match best.and_then(|(i, _)| candidates.get(i)) {
            Some((g, d)) => Pick::Type(d.name, *g),
            None => Pick::Ambiguous(candidates.iter().map(|(_, d)| d.name).collect()),
        }
    }

    /// How well `value` fits `def`, and whether it contradicts it.
    fn score(&self, gate: Gate, def: &'static TypeDef, value: &Object) -> (i64, bool) {
        let mut score = 0i64;
        let mut contradicted = false;
        if !gate.admits(self.version) {
            score -= 3;
        }
        let holder = value;
        for row in &def.rows {
            let found = match (row.kind, value) {
                (KeyKind::Name, _) => value.get(row.key.as_bytes()),
                (KeyKind::Index(i), Object::Array(a)) => a.get(i),
                _ => continue,
            };
            let Some(found) = found.filter(|v| !matches!(v, Object::Null)) else {
                if row.required == "TRUE" {
                    score -= 4;
                }
                continue;
            };
            score += 1;
            let resolved = self.resolve(found);
            let Some(v) = resolved.object() else { continue };
            let Some(alt) = pick_alt(row, v, self.version) else {
                score -= 2;
                continue;
            };
            if let Some(lits) = fixed_values(alt) {
                if lits.iter().any(|l| literal_matches(l, v) == Some(true)) {
                    score += 10;
                } else if row.required == "TRUE" {
                    // A required key with fixed values (/Subtype, /S, /FT) says what the object is.
                    contradicted = true;
                } else {
                    // An optional one out of range (/H /Glow on a link) is a wrong value, reported
                    // when the object is checked, not a sign of another type.
                    score -= 8;
                }
            }
            if let Some(rule) = Some(alt.special.trim()).filter(|s| single_call(s, "Eval").is_some()) {
                match self.eval(rule, Mode::Assertion, holder, &[], Some(v)) {
                    Some(true) => score += 2,
                    Some(false) => score -= 50,
                    None => {}
                }
            }
        }
        match value {
            Object::Dict(d) | Object::Stream(d) if !def.has_wildcard() => {
                let unknown = d.iter().filter(|(k, _)| !def.rows.iter().any(|r| r.key.as_bytes() == k.as_slice())).count();
                score -= i64::try_from(unknown).unwrap_or(i64::MAX / 2);
            }
            Object::Array(a) if !def.rows.iter().any(|r| matches!(r.kind, KeyKind::Wildcard | KeyKind::Repeat(_))) && a.len() > def.rows.len() => {
                score -= 5;
            }
            _ => {}
        }
        (score, contradicted)
    }
}

enum Pick {
    Type(&'static str, Gate),
    /// Every candidate is contradicted by the object.
    Ambiguous(Vec<&'static str>),
    /// No candidate has the object's shape.
    Nothing,
}

/// The plain values a row allows (`[Annot]`, `[Page,Template]`, `[1]`), when the list is only
/// such values: what identifies a type among its siblings.
fn fixed_values(alt: &Alt) -> Option<Vec<&'static str>> {
    let list = alt.possible.trim();
    if list.is_empty() || !matches!(alt.basic, Basic::Name | Basic::Integer) {
        return None;
    }
    let mut out = Vec::new();
    for item in split_top(alt.possible, ',') {
        let (_, inner) = gated(item.trim());
        if inner == "*" || inner.starts_with("fn:") || inner.starts_with('[') || inner.is_empty() {
            return None;
        }
        out.push(inner);
    }
    Some(out)
}

/// The alternative of `row` whose type fits `value`, preferring one the file's version admits and,
/// for a string, one whose format it has: an annotation's /M is `date;string-text`, so a string
/// that is no date is text, not a malformed date.
fn pick_alt(row: &'static Row, value: &Object, version: Version) -> Option<&'static Alt> {
    let fits = |a: &&Alt| accepts(a.basic, value);
    let formed = |a: &&Alt| fits(a) && well_formed(a.basic, value);
    row.alts
        .iter()
        .filter(formed)
        .find(|a| a.gate.admits(version))
        .or_else(|| row.alts.iter().filter(fits).find(|a| a.gate.admits(version)))
        .or_else(|| row.alts.iter().find(fits))
}

/// Whether a string has the format its type asks for: a date (ISO 32000-2 §7.9.4), ASCII.
fn well_formed(basic: Basic, o: &Object) -> bool {
    match (basic, o) {
        (Basic::Date, Object::String { bytes, .. }) => is_date(bytes),
        (Basic::StringAscii, Object::String { bytes, .. }) => bytes.is_ascii(),
        _ => true,
    }
}

fn accepts(basic: Basic, o: &Object) -> bool {
    match basic {
        Basic::Array | Basic::Rectangle | Basic::Matrix => matches!(o, Object::Array(_)),
        Basic::Bitmask | Basic::Integer => matches!(o, Object::Int(_)),
        Basic::Number => matches!(o, Object::Int(_) | Object::Real(_)),
        Basic::Boolean => matches!(o, Object::Bool(_)),
        Basic::Date | Basic::String | Basic::StringAscii | Basic::StringByte | Basic::StringText => matches!(o, Object::String { .. }),
        Basic::Dictionary | Basic::NameTree | Basic::NumberTree => matches!(o, Object::Dict(_)),
        Basic::Stream => matches!(o, Object::Stream(_)),
        Basic::Name => matches!(o, Object::Name(_)),
        Basic::Null => matches!(o, Object::Null),
    }
}

fn kind(o: &Object) -> &'static str {
    match o {
        Object::Null => "null",
        Object::Bool(_) => "boolean",
        Object::Int(_) => "integer",
        Object::Real(_) => "number",
        Object::String { .. } => "string",
        Object::Name(_) => "name",
        Object::Array(_) => "array",
        Object::Dict(_) => "dictionary",
        Object::Stream(_) => "stream",
        Object::Ref(_) => "reference",
    }
}

fn number(o: &Object) -> Option<f64> {
    match o {
        Object::Int(i) => Some(*i as f64),
        Object::Real(r) if r.is_finite() => Some(*r),
        _ => None,
    }
}

/// Whether a model literal (`Page`, `1`, `0.5`, `'text'`, `true`, `null`) is `value`; `None`
/// when the literal can't be compared with it.
fn literal_matches(lit: &str, value: &Object) -> Option<bool> {
    let lit = lit.trim();
    Some(match value {
        Object::Name(n) => n.as_slice() == lit.as_bytes(),
        Object::Int(i) => match (lit.parse::<i64>(), lit.parse::<f64>()) {
            (Ok(l), _) => *i == l,
            (_, Ok(l)) => (*i as f64 - l).abs() < 1e-9,
            _ => false,
        },
        Object::Real(r) => match lit.parse::<f64>() {
            Ok(l) => (r - l).abs() < 1e-9,
            Err(_) => false,
        },
        Object::Bool(b) => match lit {
            "true" => *b,
            "false" => !*b,
            _ => false,
        },
        Object::String { bytes, .. } => match lit.strip_prefix('\'').and_then(|l| l.strip_suffix('\'')) {
            Some(s) => bytes.as_slice() == s.as_bytes(),
            None => false,
        },
        Object::Null => lit == "null",
        _ => return None,
    })
}

/// A value as a finding shows it: names with their slash, strings in parentheses, cut short.
fn show(o: &Object) -> String {
    let s = match o {
        Object::Name(n) => format!("/{}", String::from_utf8_lossy(n)),
        Object::Int(i) => i.to_string(),
        Object::Real(r) => r.to_string(),
        Object::Bool(b) => b.to_string(),
        Object::String { bytes, .. } => format!("({})", String::from_utf8_lossy(bytes)),
        other => kind(other).to_string(),
    };
    cut(&s, 60).into_owned()
}

fn show_literal(lit: &str) -> String {
    let lit = lit.trim();
    if lit.parse::<f64>().is_ok() || matches!(lit, "true" | "false" | "null") {
        lit.to_string()
    } else if let Some(s) = lit.strip_prefix('\'').and_then(|l| l.strip_suffix('\'')) {
        format!("({s})")
    } else {
        format!("/{lit}")
    }
}

/// `s` cut to at most `max` characters, at a character boundary.
fn cut(s: &str, max: usize) -> Cow<'_, str> {
    match s.char_indices().nth(max) {
        Some((at, _)) => Cow::Owned(format!("{}…", s.get(..at).unwrap_or_default())),
        None => Cow::Borrowed(s),
    }
}

/// `D:YYYYMMDDHHmmSSOHH'mm` with everything after the year optional (ISO 32000-2 §7.9.4).
/// Readers accept the apostrophe after the offset minutes, and the `D:` prefix missing is
/// tolerated by most, so it is accepted as well.
fn is_date(bytes: &[u8]) -> bool {
    let s = bytes.strip_prefix(b"D:").unwrap_or(bytes);
    let digits = s.iter().take_while(|b| b.is_ascii_digit()).count();
    if !(4..=14).contains(&digits) || !digits.is_multiple_of(2) {
        return false;
    }
    let field = |at: usize| -> Option<u32> { std::str::from_utf8(s.get(at..at + 2)?).ok()?.parse().ok() };
    let ok = |at: usize, lo: u32, hi: u32| digits <= at || field(at).is_some_and(|v| (lo..=hi).contains(&v));
    if !(ok(4, 1, 12) && ok(6, 1, 31) && ok(8, 0, 23) && ok(10, 0, 59) && ok(12, 0, 59)) {
        return false;
    }
    let rest = s.get(digits..).unwrap_or_default();
    match rest.first() {
        None => true,
        Some(b'Z') => rest.len() == 1 || rest.get(1..).is_some_and(|r| r.iter().all(|b| matches!(b, b'0' | b'\''))),
        Some(b'+' | b'-') => {
            let t = rest.get(1..).unwrap_or_default();
            let hh = t.iter().take_while(|b| b.is_ascii_digit()).count();
            hh == 2 && t.get(2..).is_some_and(|r| r.is_empty() || r.iter().all(|b| b.is_ascii_digit() || *b == b'\''))
        }
        _ => false,
    }
}

/// Whether a version already reported (`newer`) says all there is to say of something from `v`.
fn covers(newer: Option<Version>, v: Version) -> bool {
    newer.is_some_and(|n| v <= n)
}

fn parents_of(here: &Here) -> Parents {
    std::iter::once(here.obj.clone()).chain(here.parents.iter().take(2).cloned()).collect()
}

fn child_path(parent: &str, key: &str) -> String {
    let path = format!("{parent}/{key}");
    if path.len() <= MAX_PATH {
        return path;
    }
    // Keep the end, which says where the problem is; cut at a character boundary.
    let mut start = path.len() - MAX_PATH / 2;
    while !path.is_char_boundary(start) {
        start += 1;
    }
    format!("…{}", path.get(start..).unwrap_or_default())
}

/// What a predicate sees: the object holding the row, its parents, the row's value, the
/// trailer, and the document through the walker.
struct Ctx<'a> {
    w: &'a Walker<'a>,
    holder: &'a Object,
    parents: &'a [Arc<Object>],
    current: Option<&'a Object>,
}

impl Ctx<'_> {
    /// Follow `segs` from `at`; only the object at the end is copied.
    fn walk(&self, at: &Object, segs: &[&str]) -> Found {
        let Some((seg, rest)) = segs.split_first() else { return Found::Present(at.clone()) };
        if *seg == "*" {
            return Found::Unknown;
        }
        let next = match at {
            Object::Dict(_) | Object::Stream(_) => at.get(seg.as_bytes()),
            Object::Array(a) => match seg.parse::<usize>() {
                Ok(i) => a.get(i),
                Err(_) => return Found::Unknown,
            },
            _ => return Found::Unknown,
        };
        match next {
            None | Some(Object::Null) => Found::Absent,
            Some(Object::Ref(r)) => match self.w.load(*r) {
                Some(o) if !matches!(*o, Object::Null) => self.walk(&o, rest),
                _ => Found::Absent,
            },
            Some(o) => self.walk(o, rest),
        }
    }
}

impl Env for Ctx<'_> {
    fn version(&self) -> Version {
        self.w.version
    }

    fn find(&self, path: &[String]) -> Found {
        let segs: Vec<&str> = path.iter().map(String::as_str).collect();
        match segs.split_first() {
            Some((&"trailer", rest)) => {
                // `trailer::Catalog` is the model's name for `trailer::Root`.
                let rest: Vec<&str> = rest.iter().enumerate().map(|(i, s)| if i == 0 && *s == "Catalog" { "Root" } else { *s }).collect();
                self.walk(&self.w.trailer, &rest)
            }
            Some((&"parent", _)) => {
                let up = segs.iter().take_while(|s| **s == "parent").count();
                match up.checked_sub(1).and_then(|i| self.parents.get(i)) {
                    Some(p) => self.walk(p, segs.get(up..).unwrap_or_default()),
                    None => Found::Unknown,
                }
            }
            Some(_) => self.walk(self.holder, &segs),
            None => Found::Unknown,
        }
    }

    fn current(&self) -> Option<&Object> {
        self.current
    }

    fn holder(&self) -> Option<&Object> {
        Some(self.holder)
    }

    fn file_size(&self) -> Option<u64> {
        self.w.src.file_size()
    }

    fn page_count(&self) -> Option<usize> {
        self.w.src.page_count()
    }

    fn unsupported(&self, name: &str) {
        self.w.note(name);
    }
}

#[cfg(test)]
pub(crate) fn date_ok(s: &str) -> bool {
    is_date(s.as_bytes())
}
