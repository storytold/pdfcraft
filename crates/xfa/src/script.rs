//! The form as scripts see it (XFA 3.3 part 6): the instance tree with values and presence, the
//! events a template declares, the presence overrides scripts make (kept in the PDF so they
//! survive undo and reopening), and laying the form out again after scripts changed it.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use pdfcraft_cos::{Dict, Document, Object, PdfString};

use crate::data::{DataNode, data_path, parse_datasets};
use crate::model::{Access, Field, Node, Presence, Props, Script, SomIndices, Template};
use crate::{Report, XfaError};

/// Private key on the AcroForm: presence scripts set, by SOM path.
pub const OVERRIDES_KEY: &[u8] = b"PCXfaOverrides";

const MAX_DEPTH: usize = 64;
const MAX_EVENTS: usize = 20_000;
/// Most `/Fields` and `/Kids` entries [`fields_by_som`] looks at in all: fields already read, and
/// entries that are not references, included.
const MAX_FIELD_STEPS: usize = 1 << 20;
/// Most bytes of script the events hold together: each instance of a repeated subform holds its
/// own copy, so a long script repeated many times stops here (as much as the XFA packets hold).
pub(crate) const MAX_EVENT_SCRIPT_BYTES: usize = 64 << 20;
/// Most objects of the live form built (the scripting engine's snapshot holds as many):
/// nested repeating subforms multiply, so the walk stops here and says so.
pub const MAX_FORM_NODES: usize = 200_000;
const MAX_OVERRIDES: usize = 10_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeKind {
    Subform,
    Field,
    ExclGroup,
    Draw,
    Area,
}

/// One object of the live form.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FormNode {
    pub name: String,
    pub som: String,
    pub kind: Option<NodeKind>,
    pub value: String,
    pub numeric: bool,
    pub presence: String,
    pub access: String,
    pub repeatable: bool,
    pub occur_min: usize,
    pub occur_max: Option<usize>,
    pub index: usize,
    pub children: Vec<FormNode>,
    /// Fields: the display picture (see [`Field::display_picture`]), for `formattedValue`.
    pub display_picture: Option<String>,
    /// The object's attributes and property elements as the template gives them, for scripts to
    /// read: shared with the template and every other instance of the object, never copied.
    pub props: Arc<Props>,
    /// The template's tree [`Self::props`] was made from, before the scripts' changes (the same
    /// tree when there are none): changes are applied to it again, never over earlier ones.
    pub template_props: Arc<Props>,
}

/// A script the template runs for an event of one node instance.
#[derive(Clone, Debug, PartialEq)]
pub struct ScriptEvent {
    pub som: String,
    /// `initialize`, `calculate`, `validate`, `click`, `change`, `exit`, …
    pub activity: String,
    pub script: String,
    pub formcalc: bool,
    /// Validate scripts: the message shown when they fail.
    pub message: Option<String>,
}

/// The live form as [`form_tree`] builds it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LiveForm {
    /// The root subform's instance tree.
    pub root: FormNode,
    /// Every scripted event, in document order.
    pub events: Vec<ScriptEvent>,
    /// The form was larger than [`MAX_FORM_NODES`] objects (or [`MAX_EVENTS`] events, or
    /// [`MAX_EVENT_SCRIPT_BYTES`] of their scripts): what is past that was left out.
    pub truncated: bool,
}

/// What scripts changed about the template's objects, by SOM path.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Overrides {
    pub presence: BTreeMap<String, String>,
    pub access: BTreeMap<String, String>,
    /// Property changes (see [`crate::restyle`]): by SOM path, then by property key
    /// ([`crate::restyle::prop_key`]), the value set. Bounded by
    /// [`crate::restyle::MAX_PROP_OVERRIDES`] changes and
    /// [`crate::restyle::MAX_PROP_OVERRIDE_BYTES`] bytes; changed only through
    /// [`Overrides::set_prop`], which keeps the count. Shared with the template's memo (see
    /// [`overrides_of`]) until changed.
    props: Arc<crate::memo::PropMap>,
    prop_count: usize,
    /// The copies of drawn edges and corners kept (`=` keys), at most
    /// [`crate::restyle::MAX_PROP_OVERRIDES`] too, so a file holds no more entries than reading
    /// it looks at (twice that limit).
    prop_copies: usize,
    prop_bytes: usize,
    /// Property changes the file held that were not kept when it was read (not a change a
    /// script could make, kept for an object the template may make more than once
    /// ([`crate::som_made_once`]), or past the limits): counted for a note, as they aren't
    /// applied. The file keeps them until the form's overrides are next written
    /// ([`set_overrides`]), which leaves them out: after a script changes a property, a
    /// presence or an access.
    pub dropped: usize,
}

/// Why a property change was not kept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PropRefused {
    /// The value or the path is not one the property takes ([`crate::restyle::check`]).
    Invalid(String),
    /// The object already keeps as many changes as one may
    /// ([`crate::restyle::MAX_PROP_CHANGES_PER_OBJECT`]); other objects still take theirs.
    ObjectFull,
    /// The form already holds as many changes, or bytes of them, as it may.
    Full,
}

impl Overrides {
    pub fn is_empty(&self) -> bool {
        self.presence.is_empty() && self.access.is_empty() && self.props.is_empty()
    }

    /// The property changes, by SOM path.
    pub fn props(&self) -> &BTreeMap<String, BTreeMap<String, String>> {
        &self.props
    }

    /// Set attribute `attr` (`None`: the text) of the property at `path` of the object at `som`
    /// to `value`. `Ok(true)` when that changed what is kept, `Ok(false)` when it was already so.
    /// Refused past [`crate::restyle::MAX_PROP_OVERRIDES`] changes or
    /// [`crate::restyle::MAX_PROP_OVERRIDE_BYTES`] bytes in the form ([`PropRefused::Full`]), or
    /// [`crate::restyle::MAX_PROP_CHANGES_PER_OBJECT`] on one object ([`PropRefused::ObjectFull`]).
    pub fn set_prop(&mut self, som: &str, path: &str, attr: Option<&str>, value: &str) -> Result<bool, PropRefused> {
        self.set(som, path, attr, value, false)
    }

    /// [`Overrides::set_prop`] for a value the scripting host copied from the edge or corner
    /// drawn on a side when a script reached one the border doesn't list (so that only that
    /// side changes): kept under its own key, applied before the writes to the same property,
    /// and not counted as a change against the form's limit (its bytes are, and the copies have
    /// a limit of their own, as many).
    pub fn set_prop_copy(&mut self, som: &str, path: &str, attr: Option<&str>, value: &str) -> Result<bool, PropRefused> {
        self.set(som, path, attr, value, true)
    }

    fn set(&mut self, som: &str, path: &str, attr: Option<&str>, value: &str, copy: bool) -> Result<bool, PropRefused> {
        crate::restyle::check(path, attr, value).map_err(PropRefused::Invalid)?;
        let key = if copy { format!("={}", crate::restyle::prop_key(path, attr)) } else { crate::restyle::prop_key(path, attr) };
        let on_object = self.props.get(som).map_or(0, BTreeMap::len);
        if on_object >= crate::restyle::MAX_PROP_CHANGES_PER_OBJECT && self.props.get(som).is_none_or(|m| !m.contains_key(&key)) {
            return Err(PropRefused::ObjectFull);
        }
        let old = self.props.get(som).and_then(|m| m.get(&key));
        if old.is_some_and(|v| v == value) {
            return Ok(false);
        }
        let added = match old {
            Some(v) => value.len().saturating_sub(v.len()),
            None => som.len().saturating_add(key.len()).saturating_add(value.len()),
        };
        let removed = old.map_or(0, |v| v.len().saturating_sub(value.len()));
        let count = if old.is_some() || copy { self.prop_count } else { self.prop_count.saturating_add(1) };
        let copies = if old.is_none() && copy { self.prop_copies.saturating_add(1) } else { self.prop_copies };
        let bytes = self.prop_bytes.saturating_add(added).saturating_sub(removed);
        if count > crate::restyle::MAX_PROP_OVERRIDES
            || copies > crate::restyle::MAX_PROP_OVERRIDES
            || bytes > crate::restyle::MAX_PROP_OVERRIDE_BYTES
        {
            return Err(PropRefused::Full);
        }
        Arc::make_mut(&mut self.props).entry(som.to_string()).or_default().insert(key, value.to_string());
        self.prop_count = count;
        self.prop_copies = copies;
        self.prop_bytes = bytes;
        Ok(true)
    }
}

fn presence_name(p: Presence) -> &'static str {
    match p {
        Presence::Visible => "visible",
        Presence::Invisible => "invisible",
        Presence::Hidden => "hidden",
        Presence::Inactive => "inactive",
    }
}

fn access_name(a: Access) -> &'static str {
    match a {
        Access::Open => "open",
        Access::ReadOnly => "readOnly",
        Access::Protected => "protected",
        Access::NonInteractive => "nonInteractive",
    }
}

fn acroform(doc: &Document) -> Option<(Option<pdfcraft_cos::ObjRef>, Dict)> {
    let root = doc.root()?;
    let catalog = doc.get(root);
    let o = catalog.as_dict()?.get(b"AcroForm")?.clone();
    let d = doc.resolve(&o).as_dict().cloned()?;
    Some((o.as_ref(), d))
}

/// The overrides the document carries.
pub fn overrides(doc: &Document) -> Overrides {
    read_overrides(doc, None)
}

/// [`overrides`] for a document of the form `tpl`: when its property changes are the object this
/// template's memo last read them from, they are taken from the memo rather than checked one by
/// one again, so an event costs the same however many changes the form keeps.
pub fn overrides_of(doc: &Document, tpl: &Template) -> Overrides {
    read_overrides(doc, Some(tpl))
}

#[cfg(test)]
thread_local! {
    /// Property changes read from documents (rather than taken from a memo), on this thread.
    static KEPT_READ: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// Entries of documents' property changes looked at, kept or not, on this thread.
    static ENTRIES_LOOKED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How many entries of property changes reading documents looked at on this thread, since the
/// last call.
#[cfg(test)]
pub(crate) fn take_entries_looked() -> usize {
    ENTRIES_LOOKED.with(|c| c.replace(0))
}

/// How many property changes have been read from documents on this thread, since the last call.
#[cfg(test)]
pub(crate) fn take_kept_read() -> usize {
    KEPT_READ.with(|c| c.replace(0))
}

fn read_overrides(doc: &Document, tpl: Option<&Template>) -> Overrides {
    let memo = tpl.map(|t| &t.memo);
    let mut out = Overrides::default();
    let Some((_, acro)) = acroform(doc) else { return out };
    let Some(o) = acro.get(OVERRIDES_KEY).map(|o| doc.resolve(o)) else { return out };
    let Some(d) = o.as_dict() else { return out };
    for (which, map) in [(&b"Presence"[..], &mut out.presence), (&b"Access"[..], &mut out.access)] {
        if let Some(m) = d.get(which).map(|m| doc.resolve(m))
            && let Some(m) = m.as_dict()
        {
            for (k, v) in m.iter().take(MAX_OVERRIDES) {
                // Measured as the file holds it, before it is decoded.
                let Some(v) = v.as_string().filter(|v| v.bytes.len() <= MAX_STATE_BYTES) else { continue };
                map.insert(String::from_utf8_lossy(k).into_owned(), v.to_text());
            }
        }
    }
    // Property changes: each checked and counted as a script's would be, so a file can hold
    // no more than a script could have made, and nothing a script couldn't have set. They are
    // read as [`set_overrides`] writes them, a dictionary for each path holding its strings: a
    // reference in their place is followed nowhere, so what is read is what the object itself
    // holds, and the object can stand for it in the memo.
    if let Some(object) = d.get(b"Props").map(|m| doc.resolve(m))
        && let Some(m) = object.as_dict()
    {
        if let Some(kept) = memo.and_then(|memo| memo.kept(&object)) {
            out.props = kept.props;
            out.prop_count = kept.count;
            out.prop_copies = kept.copies;
            out.prop_bytes = kept.bytes;
            out.dropped = kept.dropped;
            return out;
        }
        // An entry refused (invalid, or past a limit) is counted in `dropped` for a note. No
        // more entries than the limits allow (changes and copies, each up to the limit) are
        // looked at, and no more bytes decoded than those could take; the rest is counted
        // unread, a path's dictionary by its size.
        let most = crate::restyle::MAX_PROP_OVERRIDES.saturating_mul(2);
        let mut read = 0usize;
        let mut raw_left = MAX_PROP_RAW_BYTES;
        for (som, sets) in m.iter() {
            let Some(sets) = sets.as_dict() else {
                read = read.saturating_add(1);
                out.dropped = out.dropped.saturating_add(1);
                continue;
            };
            if read >= most {
                out.dropped = out.dropped.saturating_add(sets.len());
                continue;
            }
            let som = String::from_utf8_lossy(som).into_owned();
            // Changes are kept only for an object the template makes no more than once
            // ([`crate::data::som_made_once`]): a script's write elsewhere is refused, and so is
            // one the file holds. Charged as read (a path holding nothing too, without asking
            // whose it is), so a file naming many paths costs no more than one holding as many
            // changes.
            if sets.is_empty() || tpl.is_some_and(|t| !crate::data::som_made_once(t, &som)) {
                read = read.saturating_add(sets.len().max(1));
                out.dropped = out.dropped.saturating_add(sets.len());
                continue;
            }
            for (seen, (key, v)) in sets.iter().enumerate() {
                #[cfg(test)]
                ENTRIES_LOOKED.with(|c| c.set(c.get().saturating_add(1)));
                read = read.saturating_add(1);
                if read > most {
                    out.dropped = out.dropped.saturating_add(sets.len().saturating_sub(seen));
                    break;
                }
                // Measured as the file holds it, before it is decoded: longer than any value
                // that may be kept, or past what all of them could take, it is not decoded.
                let Some(v) = v.as_string().filter(|v| v.bytes.len() <= MAX_PROP_RAW_VALUE && v.bytes.len() <= raw_left) else {
                    out.dropped = out.dropped.saturating_add(1);
                    continue;
                };
                raw_left = raw_left.saturating_sub(v.bytes.len());
                let v = v.to_text();
                #[cfg(test)]
                KEPT_READ.with(|c| c.set(c.get().saturating_add(1)));
                let key = String::from_utf8_lossy(key);
                let (path, attr) = crate::restyle::split_key(&key);
                let kept = if key.starts_with('=') { out.set_prop_copy(&som, path, attr, &v) } else { out.set_prop(&som, path, attr, &v) };
                if kept.is_err() {
                    out.dropped = out.dropped.saturating_add(1);
                }
            }
        }
        if let Some(memo) = memo {
            let kept = crate::memo::KeptProps {
                props: Arc::clone(&out.props),
                count: out.prop_count,
                copies: out.prop_copies,
                bytes: out.prop_bytes,
                dropped: out.dropped,
            };
            memo.read(&object, kept);
        }
    }
    out
}

/// The longest a kept presence or access is in a file, in bytes as the file
/// holds it: the longest a script can set, `nonInteractive`, is 30 bytes as UTF-16 with its
/// byte-order mark. A longer string is not decoded.
const MAX_STATE_BYTES: usize = 64;

/// The longest a kept property value is in a file, in bytes as the file holds it: a value of
/// [`crate::restyle::MAX_PROP_VALUE`] bytes, the longest that may be kept, is at most twice
/// that as UTF-16, and a byte-order mark. A longer string is refused before it is decoded.
const MAX_PROP_RAW_VALUE: usize = 2 + 2 * crate::restyle::MAX_PROP_VALUE;

/// The most bytes of kept property values decoded from a file, as the file holds them: what
/// [`crate::restyle::MAX_PROP_OVERRIDE_BYTES`] of values, the most that may be kept, take as
/// UTF-16, with a byte-order mark for each of the entries looked at.
const MAX_PROP_RAW_BYTES: usize = 2 * crate::restyle::MAX_PROP_OVERRIDE_BYTES + 4 * crate::restyle::MAX_PROP_OVERRIDES;

/// Whether `o` is an object of property changes as [`set_overrides`] writes them: a dictionary,
/// not empty, of dictionaries of strings.
fn holds_prop_changes(o: &Object) -> bool {
    o.as_dict()
        .is_some_and(|d| !d.is_empty() && d.iter().all(|(_, sets)| sets.as_dict().is_some_and(|m| m.iter().all(|(_, v)| v.as_string().is_some()))))
}

/// Store `ov` in the document (none: the key is removed).
pub fn set_overrides(doc: &mut Document, ov: &Overrides) -> Result<(), XfaError> {
    let Some(root) = doc.root() else { return Ok(()) };
    let Some((acro_ref, mut acro)) = acroform(doc) else { return Ok(()) };
    // The property changes go in an object of their own: they can run to a megabyte, and the
    // AcroForm dictionary that would otherwise hold them is read and copied on every edit. The
    // one already there is written over only when this document added it since it was read and
    // it still holds property changes: a reference in the file can name any object.
    let props_ref = acro
        .get(OVERRIDES_KEY)
        .map(|o| doc.resolve(o))
        .and_then(|o| o.as_dict().and_then(|d| d.get(b"Props")).and_then(Object::as_ref))
        .filter(|r| doc.xref_entry(r.num).is_none() && doc.is_edited(r.num) && holds_prop_changes(&doc.get(*r)));
    // The object this document added for the changes, when none are left: nothing refers to
    // it any more, and an incremental save would still write it.
    if let Some(r) = props_ref.filter(|_| ov.props.is_empty()) {
        doc.free(r);
    }
    if ov.is_empty() {
        acro.remove(OVERRIDES_KEY);
    } else {
        let mut d = Dict::new();
        for (which, map) in [("Presence", &ov.presence), ("Access", &ov.access)] {
            if map.is_empty() {
                continue;
            }
            let mut m = Dict::new();
            for (k, v) in map.iter().take(MAX_OVERRIDES) {
                m.set(k.as_bytes().to_vec(), Object::String(PdfString::text(v)));
            }
            d.set(which.as_bytes().to_vec(), Object::Dict(m));
        }
        if !ov.props.is_empty() {
            let mut objects = Dict::new();
            for (som, sets) in ov.props.iter() {
                let mut m = Dict::new();
                for (k, v) in sets {
                    m.set(k.as_bytes().to_vec(), Object::String(PdfString::text(v)));
                }
                objects.set(som.as_bytes().to_vec(), Object::Dict(m));
            }
            let r = match props_ref {
                Some(r) => {
                    doc.set(r, Object::Dict(objects));
                    r
                }
                None => doc.add(Object::Dict(objects)),
            };
            d.set(b"Props".to_vec(), Object::Ref(r));
        }
        acro.set(OVERRIDES_KEY.to_vec(), Object::Dict(d));
    }
    match acro_ref {
        Some(r) => doc.set(r, Object::Dict(acro)),
        None => doc.update_dict(root, |c| c.set(b"AcroForm".to_vec(), Object::Dict(acro)))?,
    }
    Ok(())
}

fn child_som(parent: &str, name: Option<&str>, index: usize) -> String {
    match name {
        Some(n) if !parent.is_empty() => format!("{parent}.{n}[{index}]"),
        Some(n) => format!("{n}[{index}]"),
        None => parent.to_string(),
    }
}

struct Walker<'a> {
    /// For data paths (areas are not data scopes).
    tpl: &'a Template,
    data: Option<&'a DataNode>,
    ov: &'a Overrides,
    events: Vec<ScriptEvent>,
    /// Bytes of script the events hold.
    script_bytes: usize,
    /// Objects built so far.
    nodes: usize,
    truncated: bool,
}

impl Walker<'_> {
    /// Count one more object; false once the budget is spent.
    fn take(&mut self) -> bool {
        if self.nodes >= MAX_FORM_NODES {
            self.truncated = true;
            return false;
        }
        self.nodes += 1;
        true
    }

    /// Record the scripts of the object at `som` as events: each non-empty event script, then its
    /// calculate and validate scripts (the validate one with `message`). Once [`MAX_EVENTS`] events
    /// or [`MAX_EVENT_SCRIPT_BYTES`] of their scripts are recorded, nothing more is and the form is
    /// marked truncated.
    fn push_events(&mut self, som: &str, scripts: &[Script], calculate: Option<&Script>, validate: Option<&Script>, message: Option<&str>) {
        if self.events.len() >= MAX_EVENTS {
            self.truncated = true;
            return;
        }
        let all = scripts
            .iter()
            .map(|s| (s, s.activity.as_str(), None))
            .chain(calculate.map(|c| (c, "calculate", None)))
            .chain(validate.map(|v| (v, "validate", message)));
        for (s, activity, message) in all {
            if s.text.trim().is_empty() {
                continue;
            }
            match self.script_bytes.checked_add(s.text.len()).filter(|n| *n <= MAX_EVENT_SCRIPT_BYTES) {
                Some(n) => self.script_bytes = n,
                None => {
                    self.truncated = true;
                    return;
                }
            }
            self.events.push(ScriptEvent {
                som: som.to_string(),
                activity: activity.to_string(),
                script: s.text.clone(),
                formcalc: s.formcalc,
                message: message.map(str::to_string),
            });
        }
    }

    /// The presence of the object named `name` at `som`: a script's, or else the template's. An
    /// unnamed object shares its parent's SOM path, so a presence set there is not its own.
    fn presence(&self, name: Option<&str>, som: &str, p: Presence) -> String {
        name.and_then(|_| self.ov.presence.get(som)).cloned().unwrap_or_else(|| presence_name(p).to_string())
    }

    /// The access of the object at `som`: a script's, or else the template's `a`.
    fn access(&self, som: &str, a: Access) -> String {
        self.ov.access.get(som).cloned().unwrap_or_else(|| access_name(a).to_string())
    }

    /// The live node of field `f`, child `index` of `parent_som`, with its scripts recorded. In an
    /// exclusion group (`radio` is the group's selected value) its value is its on value when picked,
    /// else empty; elsewhere it is its data value (as saved, as scripts read it) or its template default.
    fn field(&mut self, f: &Field, parent_som: &str, index: usize, radio: Option<&str>) -> FormNode {
        let som = child_som(parent_som, f.common.name.as_deref().or(Some("field")), index);
        // The field as scripts changed its properties (its validation, pictures, its tree):
        // read again only for changes the live form takes from it; for others its tree is
        // enough.
        let template_props = Arc::clone(&f.common.props);
        let changed;
        let mut props = None;
        let f = match self.ov.props.get(&som) {
            Some(sets) if crate::restyle::live_field_changes(sets) => {
                changed = crate::memo::restyled_field(&self.tpl.memo, f, sets);
                &*changed
            }
            Some(sets) => {
                props = Some(crate::memo::applied(&self.tpl.memo, &f.common.props, sets));
                f
            }
            None => f,
        };
        let data_value = self.data.and_then(|d| d.value_at(&data_path(self.tpl, &som))).map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
        let value = match (radio, data_value) {
            // A radio button's value is its on value when the group picked it.
            (Some(selected), _) => {
                let on = f.items.first().cloned().unwrap_or_default();
                if selected == on { on } else { String::new() }
            }
            (None, Some(v)) => f.saved_value(&Walker::as_scripts_read(f, &v)),
            (None, None) => f.value.plain().map(|v| f.saved_value(v.trim())).unwrap_or_default(),
        };
        self.push_events(&som, &f.scripts, f.calculate.as_ref(), f.validate.as_ref(), f.validate_message.as_deref());
        FormNode {
            name: f.common.name.clone().unwrap_or_else(|| "field".into()),
            som: som.clone(),
            kind: Some(NodeKind::Field),
            value,
            numeric: f.numeric,
            presence: self.presence(Some("field"), &som, f.common.presence),
            access: self.access(&som, f.access),
            repeatable: false,
            occur_min: 1,
            occur_max: Some(1),
            index,
            children: Vec::new(),
            display_picture: f.display_picture.clone(),
            props: props.unwrap_or_else(|| Arc::clone(&f.common.props)),
            template_props,
        }
    }

    /// A value the data holds as scripts read it: a date the field's data picture writes in
    /// another form (`20261018`) is the ISO date (`2026-10-18`) another viewer gives scripts as
    /// `rawValue`. Other values, and dates that don't match the picture, are as they are.
    fn as_scripts_read(f: &Field, v: &str) -> String {
        crate::data::data_to_iso(v, crate::data::data_date_letters(f).as_deref())
    }

    /// What [`Self::field`] gives a field outside an exclusion group: its data value (as saved,
    /// as scripts read it) or its template default.
    fn plain_field_value(tpl: &Template, data: Option<&DataNode>, f: &Field, som: &str) -> String {
        let data_value = data.and_then(|d| d.value_at(&data_path(tpl, som))).map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
        match data_value {
            Some(v) => f.saved_value(&Walker::as_scripts_read(f, &v)),
            None => f.value.plain().map(|v| f.saved_value(v.trim())).unwrap_or_default(),
        }
    }

    /// The property changes scripts made to the object at `som`, when it is named (an unnamed
    /// draw, area or subform shares its parent's SOM path, so the changes there are not its own).
    fn sets(&self, named: bool, som: &str) -> Option<&BTreeMap<String, String>> {
        if named { self.ov.props.get(som) } else { None }
    }

    /// The tree of properties of the object at `som` as scripts changed it, else `props`.
    fn props_at(&self, named: bool, som: &str, node: &Node) -> Arc<Props> {
        match self.sets(named, som) {
            // The tree is what restyling would give it (the changes applied to it), without
            // reading the object again.
            Some(sets) => crate::memo::applied(&self.tpl.memo, &node.common().props, sets),
            None => Arc::clone(&node.common().props),
        }
    }

    /// The live nodes of `nodes`, the children of `parent_som`, with their scripts recorded: one per
    /// instance of a repeating subform (what the data holds, else its initial count). Nothing past
    /// [`MAX_DEPTH`] is built, and building stops once the object budget is spent.
    fn children(&mut self, nodes: &[Node], parent_som: &str, depth: usize) -> Vec<FormNode> {
        let mut out = Vec::new();
        if depth > MAX_DEPTH {
            return out;
        }
        let sibs = SomIndices::of(nodes);
        for (i, n) in nodes.iter().enumerate() {
            if !self.take() {
                break;
            }
            let sib = sibs.at(i);
            match n {
                Node::Subform(sf) => {
                    let named = sf.common.name.is_some();
                    let occur = sf.occur;
                    let instances = crate::layout::subform_instances(self.tpl, self.data, sf, parent_som);
                    for k in 0..instances {
                        // The first instance was counted above.
                        if k > 0 && !self.take() {
                            break;
                        }
                        let index = sib + k;
                        let som = child_som(parent_som, sf.common.name.as_deref(), index);
                        self.push_events(&som, &sf.scripts, None, None, None);
                        let children = self.children(&sf.children, &som, depth + 1);
                        out.push(FormNode {
                            name: sf.common.name.clone().unwrap_or_else(|| "#subform".into()),
                            som: som.clone(),
                            kind: Some(NodeKind::Subform),
                            value: String::new(),
                            numeric: false,
                            presence: self.presence(sf.common.name.as_deref(), &som, sf.common.presence),
                            access: "open".into(),
                            repeatable: occur.max != Some(1),
                            occur_min: occur.min,
                            occur_max: occur.max,
                            index,
                            children,
                            display_picture: None,
                            props: self.props_at(named, &som, n),
                            template_props: Arc::clone(&n.common().props),
                        });
                    }
                }
                Node::Area(a) => {
                    let som = child_som(parent_som, a.common.name.as_deref(), sib);
                    let props = self.props_at(a.common.name.is_some(), &som, n);
                    let children = self.children(&a.children, &som, depth + 1);
                    out.push(FormNode {
                        name: a.common.name.clone().unwrap_or_else(|| "#area".into()),
                        som: som.clone(),
                        kind: Some(NodeKind::Area),
                        presence: self.presence(a.common.name.as_deref(), &som, a.common.presence),
                        access: "open".into(),
                        occur_min: 1,
                        occur_max: Some(1),
                        index: sib,
                        children,
                        props,
                        template_props: Arc::clone(&n.common().props),
                        ..Default::default()
                    });
                }
                Node::Field(f) => out.push(self.field(f, parent_som, sib, None)),
                Node::Draw(d) => {
                    let som = child_som(parent_som, d.common.name.as_deref(), sib);
                    out.push(FormNode {
                        name: d.common.name.clone().unwrap_or_else(|| "draw".into()),
                        som: som.clone(),
                        kind: Some(NodeKind::Draw),
                        value: d.value.plain().unwrap_or_default(),
                        props: self.props_at(d.common.name.is_some(), &som, n),
                        template_props: Arc::clone(&n.common().props),
                        presence: self.presence(d.common.name.as_deref(), &som, d.common.presence),
                        access: "open".into(),
                        occur_min: 1,
                        occur_max: Some(1),
                        index: sib,
                        ..Default::default()
                    });
                }
                Node::ExclGroup(g) => {
                    let som = child_som(parent_som, g.common.name.as_deref().or(Some("group")), sib);
                    let selected =
                        self.data.and_then(|d| d.text_at(&data_path(self.tpl, &som))).map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
                    self.push_events(&som, &g.scripts, None, None, None);
                    let fsibs = SomIndices::of_fields(&g.fields);
                    let mut children = Vec::new();
                    for (fi, f) in g.fields.iter().enumerate() {
                        if !self.take() {
                            break;
                        }
                        children.push(self.field(f, &som, fsibs.at(fi), Some(selected.as_deref().unwrap_or(""))));
                    }
                    out.push(FormNode {
                        name: g.common.name.clone().unwrap_or_else(|| "group".into()),
                        som: som.clone(),
                        kind: Some(NodeKind::ExclGroup),
                        value: selected.unwrap_or_default(),
                        presence: self.presence(Some("group"), &som, g.common.presence),
                        access: "open".into(),
                        occur_min: 1,
                        occur_max: Some(1),
                        index: sib,
                        children,
                        props: self.props_at(true, &som, n),
                        template_props: Arc::clone(&n.common().props),
                        ..Default::default()
                    });
                }
            }
        }
        out
    }
}

/// The live form: the root subform's instance tree (values from `data`, presence and access
/// from the template and `ov`) and every scripted event, in document order, within
/// [`MAX_FORM_NODES`] objects.
pub fn form_tree(tpl: &Template, data: Option<&DataNode>, ov: &Overrides) -> LiveForm {
    form_tree_paged(tpl, data, ov, &HashMap::new())
}

/// [`form_tree`] for a laid-out form whose page areas made `uses` pages each, by name
/// ([`page_area_uses`]): the master content of each of those pages is a node of its own
/// (`master[0]`, `master[1]`, …), as the layout names its fields, so a button repeated on every
/// page runs its click script as itself. A page area not in `uses` gets one node.
pub fn form_tree_paged(tpl: &Template, data: Option<&DataNode>, ov: &Overrides, uses: &HashMap<String, usize>) -> LiveForm {
    let mut w = Walker { tpl, data, ov, events: Vec::new(), script_bytes: 0, nodes: 0, truncated: false };
    let root = Node::Subform(Box::new(tpl.root.clone()));
    let mut nodes = w.children(std::slice::from_ref(&root), "", 0);
    let mut root_node = nodes.pop().unwrap_or_default();
    // Page masters' fields (Print and Save buttons, page numbers) belong to the layout model
    // in XFA; they hang off the root here, under their page area's name, one node per page made
    // from it, as the layouter names them, so their click scripts can run.
    if let Some(ps) = &tpl.root.page_set {
        for (i, area) in ps.areas.iter().enumerate() {
            // A name two areas share stays one node: its pages can't be told apart by it.
            let unique = area.name.as_ref().is_some_and(|n| ps.areas.iter().filter(|a| a.name.as_ref() == Some(n)).count() == 1);
            let pages = match &area.name {
                Some(n) if unique => uses.get(n).copied().unwrap_or(1).clamp(1, crate::layout::MAX_PAGES),
                _ => 1,
            };
            for page in 0..pages {
                let som = area.name.clone().map(|n| format!("{n}[{page}]")).unwrap_or_else(|| format!("#pageArea[{i}]"));
                let children = w.children(&area.items, &som, 1);
                if children.is_empty() {
                    break;
                }
                root_node.children.push(FormNode {
                    name: area.name.clone().unwrap_or_else(|| "#pageArea".into()),
                    som,
                    kind: Some(NodeKind::Area),
                    presence: "visible".into(),
                    access: "open".into(),
                    occur_min: 1,
                    occur_max: Some(1),
                    index: page,
                    children,
                    ..Default::default()
                });
            }
        }
    }
    LiveForm { root: root_node, events: w.events, truncated: w.truncated }
}

/// One object (or one set of objects) the live form gives a SOM segment to inside a container,
/// as [`SomOwners`] records it.
#[derive(Clone, Copy, Debug)]
struct Owner {
    /// Its number among the same-named objects of its SOM scope ([`Common::som_index`]).
    index: usize,
    /// How many live objects it stands for, up to 2: two when it is inside an unnamed subform
    /// that occurs more than once (whose instances give their children one path).
    copies: u8,
    /// The container its children are named in, when it has any.
    space: Option<usize>,
}

/// The owners of one name in a container, each list in the order of their numbers, so the
/// owners of one number are found without looking at the others.
#[derive(Debug, Default)]
struct Named {
    /// The objects that occur once: each owns its own number.
    once: Vec<Owner>,
    /// The named subforms that may repeat (`occur max` other than 1): each owns its number and,
    /// for its later instances, every number after it.
    many: Vec<Owner>,
}

impl Named {
    /// The owners of the number `index`: the objects numbered so (those with a container
    /// first), and each subform that may repeat numbered up to it (whose later instance it
    /// names).
    fn parts(&self, index: usize) -> (&[Owner], &[Owner]) {
        let from = self.once.partition_point(|o| o.index < index);
        let to = self.once.partition_point(|o| o.index <= index);
        let upto = self.many.partition_point(|o| o.index <= index);
        (self.once.get(from..to).unwrap_or_default(), self.many.get(..upto).unwrap_or_default())
    }

    /// The owners of the number `index` ([`Named::parts`]).
    fn at(&self, index: usize) -> impl Iterator<Item = &Owner> {
        let (exact, many) = self.parts(index);
        exact.iter().chain(many)
    }

    /// The containers of the owners of the number `index` that have one, each with how many
    /// live objects its owner stands for. The objects numbered so without one (fields, draws)
    /// aren't looked through, however many share the number: looking stops at the first. Each
    /// owner looked at is counted.
    fn within(&self, index: usize) -> impl Iterator<Item = (usize, u8)> {
        let looked = |_: &&Owner| {
            #[cfg(test)]
            OWNERS_LOOKED.with(|c| c.set(c.get().saturating_add(1)));
        };
        let (exact, many) = self.parts(index);
        let exact = exact.iter().inspect(looked).map_while(|o| Some((o.space?, o.copies)));
        exact.chain(many.iter().inspect(looked).filter_map(|o| Some((o.space?, o.copies))))
    }
}

/// The most objects holding others one segment of a path may name for the path to be followed
/// into them when asking who owns it ([`SomOwners::reach`]): a path through a segment naming
/// more is counted as shared without looking further. Without the bound a file's kept paths
/// could each be looked up in every same-named object of a hostile template.
///
/// Objects holding others share a segment as same-named subforms of one container when one of
/// them may repeat (its later instances take the numbers after it), or as unnamed exclusion
/// groups (each is `group[0]`). Of 491 form templates looked at, the most one segment named was
/// 9 (459 templates: 1; two: 2; one: 3; one: 9; the rest have no path that long), so 64 is
/// seven times the widest seen; with or without the bound, every one of the 1,011,788 paths
/// asked about in them (each live object's, and three paths beside each that name nothing)
/// got the same answer.
pub(crate) const MAX_REACHED: usize = 64;

/// What [`SomOwners::reach`] finds for a SOM path: the containers it reaches before its last
/// segment (each with how many live objects it stands for, up to 2), and that segment's name
/// and number.
type Reached = (Vec<(usize, u8)>, String, usize);

/// Who owns each SOM path the live form ([`form_tree_paged`]) names, made once per template
/// and kept in its memo, so [`crate::som_made_once`] costs one look-up a segment of the path
/// and [`crate::som_is_shared`] one in each container a segment reaches (no more than
/// [`MAX_REACHED`] a segment), however large the form is and however many paths are asked
/// about.
///
/// It follows the live naming: an unnamed subform or area adds no segment, so its children are
/// named in its parent's container and numbered with that container's other children (XFA 3.3
/// ch. 3, "Transparent Nodes", pp. 94-95, Example 3.12: `Receipt.Description[0]` and
/// `Receipt.Description[1]`; [`Common::som_index`]). Named objects so have a path each, but
/// for two cases: the instances of an unnamed subform the template makes more than once each
/// name their content as the first does ([`Owner::copies`]), and a later instance of a
/// subform that may repeat takes the number of a same-named object after it
/// ([`Named::many`]). Unnamed fields and exclusion groups have no path of their own: each
/// is `field[0]` or `group[0]` (unnamed objects are "invisible to the normal SOM syntax",
/// p. 94).
#[derive(Debug, Default)]
pub(crate) struct SomOwners {
    /// The names each container gives its children's segments, and who owns each.
    spaces: Vec<HashMap<String, Named>>,
    /// The first segment: the root subform's name and named page areas', each name with the
    /// container of everything it names and whether several objects have it. The number after
    /// the name isn't the template's (a page area's counts the pages laid out from it), so the
    /// content of all of one name is looked up as one container's.
    top: HashMap<String, (usize, bool)>,
    /// Unnamed page areas' containers, by their place in the page set (`#pageArea[i]`).
    unnamed_areas: HashMap<usize, usize>,
    /// Objects recorded, against [`MAX_FORM_NODES`].
    nodes: usize,
    /// Whether an object's name holds `[` or `]`: such a name's segment reads as another
    /// path's, so no path of the template is taken for one object's ([`SomOwners::made_once`]).
    bracketed: bool,
}

#[cfg(test)]
thread_local! {
    /// [`SomOwners`] made (rather than taken from a template's memo), on this thread.
    static OWNERS_MADE: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// Owners [`SomOwners::shared`] and [`SomOwners::made_once`] looked at, on this thread.
    static OWNERS_LOOKED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How many [`SomOwners`] have been made on this thread, since the last call.
#[cfg(test)]
pub(crate) fn take_owners_made() -> usize {
    OWNERS_MADE.with(|c| c.replace(0))
}

/// How many owners telling whether paths are shared, or name an object made once, looked at
/// on this thread, since the last call.
#[cfg(test)]
pub(crate) fn take_owners_looked() -> usize {
    OWNERS_LOOKED.with(|c| c.replace(0))
}

impl SomOwners {
    /// The owners of every path `tpl`'s live form names, down to [`MAX_DEPTH`] and within
    /// [`MAX_FORM_NODES`] objects, as the live form is built.
    pub(crate) fn of(tpl: &Template) -> SomOwners {
        #[cfg(test)]
        OWNERS_MADE.with(|c| c.set(c.get().saturating_add(1)));
        let mut o = SomOwners::default();
        let root = o.top_space(tpl.root.common.name.as_deref());
        o.fill(root, &tpl.root.children, 1, 1);
        if let Some(ps) = &tpl.root.page_set {
            for (i, area) in ps.areas.iter().enumerate() {
                let id = o.top_space(area.name.as_deref());
                if area.name.is_none() {
                    o.unnamed_areas.insert(i, id);
                }
                o.fill(id, &area.items, 1, 1);
            }
        }
        // In the order of the numbers: an unnamed field is `field[0]` wherever it stands, after
        // a `field[1]` too, and [`Named::at`] finds a number by halving. Of one number, the
        // owners with a container come first ([`Named::within`] stops at the first without).
        for named in o.spaces.iter_mut().flat_map(|space| space.values_mut()) {
            named.once.sort_by_key(|o| (o.index, o.space.is_none()));
            named.many.sort_by_key(|o| o.index);
        }
        o
    }

    /// The container of what the first segment `name` names: the one an earlier object of the
    /// name has, or a new one (an unnamed object has its own).
    fn top_space(&mut self, name: Option<&str>) -> usize {
        if let Some((id, several)) = name.and_then(|name| self.top.get_mut(name)) {
            *several = true;
            return *id;
        }
        let id = self.spaces.len();
        self.spaces.push(HashMap::new());
        if let Some(name) = name {
            self.bracketed |= name.contains(['[', ']']);
            self.top.insert(name.to_string(), (id, false));
        }
        id
    }

    /// Put `owner` among the owners of `name` in container `id`.
    fn own(&mut self, id: usize, name: &str, repeats: bool, owner: Owner) {
        self.bracketed |= name.contains(['[', ']']);
        let Some(container) = self.spaces.get_mut(id) else { return };
        let named = match container.get_mut(name) {
            Some(named) => named,
            None => container.entry(name.to_string()).or_default(),
        };
        if repeats { named.many.push(owner) } else { named.once.push(owner) }
    }

    /// A new container holding `nodes`; its id.
    fn space(&mut self, nodes: &[Node], depth: usize) -> usize {
        let id = self.spaces.len();
        self.spaces.push(HashMap::new());
        self.fill(id, nodes, 1, depth);
        id
    }

    /// Record `nodes` in container `id`, each standing for `copies` live objects; unnamed
    /// subforms and areas are looked through.
    fn fill(&mut self, id: usize, nodes: &[Node], copies: u8, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        let sibs = SomIndices::of(nodes);
        for (i, n) in nodes.iter().enumerate() {
            if self.nodes >= MAX_FORM_NODES {
                return;
            }
            self.nodes += 1;
            let (name, repeats, space) = match n {
                Node::Subform(sf) => match &sf.common.name {
                    Some(name) => (name.as_str(), sf.occur.max != Some(1), Some(self.space(&sf.children, depth + 1))),
                    None => {
                        // Its instances (as many as the template makes: data makes none of an
                        // unnamed one) each name their children here.
                        let more = crate::layout::instance_count(&sf.occur, 0) > 1;
                        self.fill(id, &sf.children, if more { 2 } else { copies }, depth + 1);
                        continue;
                    }
                },
                Node::Area(a) => match &a.common.name {
                    Some(name) => (name.as_str(), false, Some(self.space(&a.children, depth + 1))),
                    None => {
                        self.fill(id, &a.children, copies, depth + 1);
                        continue;
                    }
                },
                // An unnamed draw takes its parent's path: it owns none.
                Node::Draw(d) => match &d.common.name {
                    Some(name) => (name.as_str(), false, None),
                    None => continue,
                },
                Node::Field(f) => (f.common.name.as_deref().unwrap_or("field"), false, None),
                Node::ExclGroup(g) => {
                    // Its buttons, numbered among themselves.
                    let mut buttons: HashMap<String, Named> = HashMap::new();
                    for b in &g.fields {
                        if self.nodes >= MAX_FORM_NODES {
                            break;
                        }
                        self.nodes += 1;
                        let owner = Owner { index: b.common.som_index, copies: 1, space: None };
                        buttons.entry(b.common.name.clone().unwrap_or_else(|| "field".into())).or_default().once.push(owner);
                    }
                    let space = self.spaces.len();
                    self.spaces.push(buttons);
                    (g.common.name.as_deref().unwrap_or("group"), false, Some(space))
                }
            };
            let owner = Owner { index: sibs.at(i), copies, space };
            self.own(id, name, repeats, owner);
        }
    }

    /// The owners of `name[index]` in container `id`, each counted as looked at.
    fn owners<'a>(&'a self, id: usize, name: &str, index: usize) -> impl Iterator<Item = &'a Owner> {
        self.spaces.get(id).and_then(|space| space.get(name)).into_iter().flat_map(move |named| named.at(index)).inspect(|_| {
            #[cfg(test)]
            OWNERS_LOOKED.with(|c| c.set(c.get().saturating_add(1)));
        })
    }

    /// The containers the SOM path `som` reaches before its last segment (each with how many
    /// live objects it stands for, up to 2), with that segment's name and number: each segment
    /// is looked up in the containers the path so far reached, and each object found there owns
    /// it, so where two objects get one path, their children's paths are looked up in both.
    /// `Err`: nothing is there (`false`); or `som` is the root or a page area itself, with
    /// whether several are of its name; or a segment before the last names more than
    /// [`MAX_REACHED`] objects that hold others (`true`: counted as shared).
    fn reach(&self, som: &str) -> Result<Reached, bool> {
        let path = crate::data::som_to_path(som);
        let unnamed_area = som.strip_prefix("#pageArea[").and_then(|r| r.split_once(']')).and_then(|(i, _)| i.parse::<usize>().ok());
        let (top, several, rest) = match unnamed_area {
            Some(i) => (self.unnamed_areas.get(&i).copied(), false, path.as_slice()),
            None => {
                let Some(((first, _), rest)) = path.split_first() else { return Err(false) };
                let top = self.top.get(first).copied();
                (top.map(|(id, _)| id), top.is_some_and(|(_, several)| several), rest)
            }
        };
        // The root or a page area itself: one path for all of the name.
        let Some(((last, last_index), within)) = rest.split_last() else { return Err(several) };
        let mut reached: Vec<(usize, u8)> = top.map(|id| (id, 1)).into_iter().collect();
        for (name, index) in within {
            let mut next = Vec::new();
            for &(id, copies) in &reached {
                let Some(named) = self.spaces.get(id).and_then(|space| space.get(name)) else { continue };
                for (space, its) in named.within(*index) {
                    if next.len() >= MAX_REACHED {
                        return Err(true);
                    }
                    next.push((space, copies.saturating_mul(its).min(2)));
                }
            }
            if next.is_empty() {
                return Err(false);
            }
            reached = next;
        }
        Ok((reached, last.clone(), *last_index))
    }

    /// Whether more than one live object may have the SOM path `som` (see
    /// [`crate::som_is_shared`]): its last segment has two owners, or one standing for two live
    /// objects, in the containers the path reaches ([`SomOwners::reach`]). It stops at the
    /// second, so a call looks at no more than [`MAX_REACHED`] owners a segment, whatever the
    /// template holds.
    pub(crate) fn shared(&self, som: &str) -> bool {
        let (reached, last, index) = match self.reach(som) {
            Ok(found) => found,
            Err(several) => return several,
        };
        let mut live = 0u8;
        for &(id, copies) in &reached {
            for o in self.owners(id, &last, index) {
                live = live.saturating_add(copies.saturating_mul(o.copies).min(2));
                if live > 1 {
                    return true;
                }
            }
        }
        false
    }

    /// Whether `som` names an object the template makes no more than once, so that the path is
    /// that object's whatever the data holds (see [`crate::som_made_once`]): it starts at the
    /// root subform, and each segment after it names one object of its container that occurs
    /// once, with no subform that may repeat numbered up to it (whose later instance would take
    /// the number) and no second object of the number. `som` must be written as the live form
    /// writes paths (`name[n]` for each segment, a `.` between two): a path in another form
    /// names nothing here. A name may hold a `.` (`CheckList20.10[0]` is one segment, as each
    /// ends at its number); no name of the template may hold `[` or `]`, with which one path
    /// could be read two ways. One look-up a segment.
    pub(crate) fn made_once(&self, som: &str) -> bool {
        if self.bracketed {
            return false;
        }
        let mut segments = written_segments(som);
        // The root subform's container is the first made ([`SomOwners::of`]); a page area's
        // content is on each page laid out from it, and a root without a name has no path.
        let Some(Some((first, 0))) = segments.next() else { return false };
        let Some(&(root, false)) = self.top.get(first) else { return false };
        if root != 0 {
            return false;
        }
        let mut space = Some(root);
        for segment in segments {
            let Some((name, index)) = segment else { return false };
            let Some(named) = space.and_then(|id| self.spaces.get(id)).and_then(|s| s.get(name)) else { return false };
            #[cfg(test)]
            OWNERS_LOOKED.with(|c| c.set(c.get().saturating_add(1)));
            let ([owner], []) = named.parts(index) else { return false };
            if owner.copies != 1 {
                return false;
            }
            space = owner.space;
        }
        true
    }
}

/// The segments of a SOM path written as the live form writes them, `name[n]` each with a `.`
/// between two: each one's name and number, or `None` for (and after) a part not written so
/// (no number, a number not written as the form writes it, an empty name). A segment ends at
/// its `]`, so a name may hold a `.`.
fn written_segments(som: &str) -> impl Iterator<Item = Option<(&str, usize)>> {
    let mut rest = Some(som);
    let mut first = true;
    std::iter::from_fn(move || {
        let mut left = rest.take()?;
        if left.is_empty() && !first {
            return None;
        }
        if !first {
            left = match left.strip_prefix('.') {
                Some(after) => after,
                None => return Some(None),
            };
        }
        first = false;
        let segment = left.split_once(']').and_then(|(segment, after)| {
            let (name, number) = segment.split_once('[')?;
            let index = number.parse::<usize>().ok().filter(|i| i.to_string() == number)?;
            (!name.is_empty()).then_some((name, index, after))
        });
        match segment {
            Some((name, index, after)) => {
                rest = Some(after);
                Some(Some((name, index)))
            }
            None => Some(None),
        }
    })
}

/// Does the template declare any script (an event, calculation or validation)? Forms without
/// one need no live form and run nothing.
pub fn has_scripts(tpl: &Template) -> bool {
    /// Is any of `s` a non-blank script?
    fn any(s: &[Script]) -> bool {
        s.iter().any(|s| !s.text.trim().is_empty())
    }
    /// Has field `f` an event, calculate or validate script?
    fn field(f: &Field) -> bool {
        any(&f.scripts) || [&f.calculate, &f.validate].into_iter().flatten().any(|s| !s.text.trim().is_empty())
    }
    /// Has any object among `nodes` (and their containers, `depth` levels down) a script?
    fn walk(nodes: &[Node], depth: usize) -> bool {
        depth <= MAX_DEPTH
            && nodes.iter().any(|n| match n {
                Node::Subform(sf) => any(&sf.scripts) || walk(&sf.children, depth + 1),
                Node::Area(a) => walk(&a.children, depth + 1),
                Node::Field(f) => field(f),
                Node::ExclGroup(g) => any(&g.scripts) || g.fields.iter().any(field),
                Node::Draw(_) => false,
            })
    }
    any(&tpl.root.scripts) || walk(&tpl.root.children, 1) || tpl.root.page_set.as_ref().is_some_and(|ps| ps.areas.iter().any(|a| walk(&a.items, 1)))
}

/// Does any field of the template (in the body, an exclusion group or a master page) have an
/// `enter` or `exit` script? Forms without one have nothing to run when the focus moves.
pub fn has_focus_scripts(tpl: &Template) -> bool {
    /// Has field `f` a non-blank enter or exit script?
    fn field(f: &Field) -> bool {
        f.scripts.iter().any(|s| matches!(s.activity.as_str(), "enter" | "exit") && !s.text.trim().is_empty())
    }
    /// Has any field among `nodes` (and their containers, `depth` levels down) one?
    fn walk(nodes: &[Node], depth: usize) -> bool {
        depth <= MAX_DEPTH
            && nodes.iter().any(|n| match n {
                Node::Subform(sf) => walk(&sf.children, depth + 1),
                Node::Area(a) => walk(&a.children, depth + 1),
                Node::Field(f) => field(f),
                Node::ExclGroup(g) => g.fields.iter().any(field),
                Node::Draw(_) => false,
            })
    }
    walk(&tpl.root.children, 1) || tpl.root.page_set.as_ref().is_some_and(|ps| ps.areas.iter().any(|a| walk(&a.items, 1)))
}

/// Set the presence `ov` gives each of `nodes` (children of `parent_som`) and their descendants,
/// down to [`MAX_DEPTH`], and the access it gives a field outside an exclusion group. An unnamed
/// draw, area or subform shares its parent's SOM path, so it takes no override of its own.
pub(crate) fn apply_overrides_to(nodes: &mut [Node], parent_som: &str, ov: &Overrides, depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    let sibs = SomIndices::of(nodes);
    for (i, n) in nodes.iter_mut().enumerate() {
        let sib = sibs.at(i);
        let name_default = match n {
            Node::Field(_) => Some("field"),
            Node::ExclGroup(_) => Some("group"),
            _ => None,
        };
        let name = n.common().name.as_deref().or(name_default);
        let som = child_som(parent_som, name, sib);
        // An unnamed draw, area or subform has no SOM name: `som` is its parent's, and so are
        // the overrides under it.
        let presence = name.and_then(|_| ov.presence.get(&som)).map(|p| Presence::parse(Some(p)));
        let access = name.and_then(|_| ov.access.get(&som)).cloned();
        match n {
            Node::Subform(s) => {
                if let Some(p) = presence {
                    s.common.presence = p;
                }
                apply_overrides_to(&mut s.children, &som, ov, depth + 1);
            }
            Node::Area(a) => {
                if let Some(p) = presence {
                    a.common.presence = p;
                }
                apply_overrides_to(&mut a.children, &som, ov, depth + 1);
            }
            Node::Field(f) => {
                if let Some(p) = presence {
                    f.common.presence = p;
                }
                if let Some(a) = access {
                    f.access = match a.as_str() {
                        "readOnly" => Access::ReadOnly,
                        "protected" => Access::Protected,
                        "nonInteractive" => Access::NonInteractive,
                        _ => Access::Open,
                    };
                }
            }
            Node::Draw(d) => {
                if let Some(p) = presence {
                    d.common.presence = p;
                }
            }
            Node::ExclGroup(g) => {
                if let Some(p) = presence {
                    g.common.presence = p;
                }
                let fsibs = SomIndices::of_fields(&g.fields);
                for (fi, f) in g.fields.iter_mut().enumerate() {
                    let fsom = child_som(&som, f.common.name.as_deref().or(Some("field")), fsibs.at(fi));
                    if let Some(p) = ov.presence.get(&fsom) {
                        f.common.presence = Presence::parse(Some(p));
                    }
                }
            }
        }
    }
}

/// The template with `ov` applied to its objects (instance 0 of each; scripts that hide one
/// instance of a repeating subform hide them all). See [`apply_overrides_split`] for the
/// property changes.
pub fn apply_overrides(tpl: &Template, ov: &Overrides) -> Template {
    apply_overrides_split(tpl, ov).template
}

/// What [`apply_overrides_split`] gives.
#[derive(Clone, Debug, Default)]
pub struct Applied {
    /// The template with the changes applied.
    pub template: Template,
    /// Objects with kept property changes that aren't ones the template makes once at their
    /// paths (kept for another template, inside a subform that may repeat, or deeper than is
    /// walked): their changes are not applied.
    pub unplaced: usize,
    /// Objects some of whose property changes don't reach the page (see
    /// [`crate::restyle::restyle`]).
    pub cut: usize,
}

/// The template with `ov` applied: presence and access as [`apply_overrides`] does, and the
/// property changes, which are kept only for objects the template makes once
/// ([`crate::som_made_once`]: outside any subform that may repeat), so each is the template
/// object's. One whose object isn't found there is counted ([`Applied::unplaced`]) and left
/// out, not applied to another. Page-master content is left as it is: a page area makes many
/// pages, each with its own SOM path, so its presence and access are applied page by page as
/// the pages are laid out ([`crate::layout::layout_with_overrides`]), and no property change is
/// kept for it.
pub fn apply_overrides_split(tpl: &Template, ov: &Overrides) -> Applied {
    struct W<'a> {
        ov: &'a Overrides,
        /// The paths whose property changes were applied.
        placed: std::collections::BTreeSet<String>,
        cut: usize,
    }
    impl W<'_> {
        /// Apply the property changes of the object at `som` to `n`.
        fn props(&mut self, n: &mut Node, som: &str) {
            let Some(sets) = self.ov.props.get(som) else { return };
            if crate::restyle::restyle(n, sets) {
                self.cut = self.cut.saturating_add(1);
            }
            self.placed.insert(som.to_string());
        }

        /// Set the presence `ov` gives each of `nodes` (children of `parent_som`) and their
        /// descendants, down to [`MAX_DEPTH`], the access it gives a field outside an exclusion
        /// group, and (`props`) the property changes. An unnamed draw, area or subform shares
        /// its parent's SOM path, so it takes no change of its own. `in_repeat`: the nodes are
        /// inside a subform the template may make more than once, where no property change is
        /// kept (told as [`SomOwners`] tells it: a named subform by its `occur` maximum, an
        /// unnamed one, which data makes no instance of, by how many its `occur` starts with).
        fn walk(&mut self, nodes: &mut [Node], parent_som: &str, props: bool, presence_on: bool, in_repeat: bool, depth: usize) {
            if depth > MAX_DEPTH {
                return;
            }
            let ov = self.ov;
            let sibs = SomIndices::of(nodes);
            for (i, n) in nodes.iter_mut().enumerate() {
                let sib = sibs.at(i);
                let name_default = match n {
                    Node::Field(_) => Some("field"),
                    Node::ExclGroup(_) => Some("group"),
                    _ => None,
                };
                let name: Option<String> = n.common().name.as_deref().or(name_default).map(str::to_string);
                let som = child_som(parent_som, name.as_deref(), sib);
                let presence = name.as_ref().filter(|_| presence_on).and_then(|_| ov.presence.get(&som)).map(|p| Presence::parse(Some(p)));
                let access = name.as_ref().filter(|_| presence_on).and_then(|_| ov.access.get(&som)).cloned();
                let repeats = match n {
                    Node::Subform(s) => match s.common.name {
                        Some(_) => s.occur.max != Some(1),
                        None => crate::layout::instance_count(&s.occur, 0) > 1,
                    },
                    _ => false,
                };
                if props && name.is_some() && !in_repeat && !repeats {
                    self.props(n, &som);
                }
                let inner = in_repeat || repeats;
                match n {
                    Node::Subform(s) => {
                        if let Some(p) = presence {
                            s.common.presence = p;
                        }
                        self.walk(&mut s.children, &som, props, presence_on, inner, depth + 1);
                    }
                    Node::Area(a) => {
                        if let Some(p) = presence {
                            a.common.presence = p;
                        }
                        self.walk(&mut a.children, &som, props, presence_on, inner, depth + 1);
                    }
                    Node::Field(f) => {
                        if let Some(p) = presence {
                            f.common.presence = p;
                        }
                        if let Some(a) = access {
                            f.access = match a.as_str() {
                                "readOnly" => Access::ReadOnly,
                                "protected" => Access::Protected,
                                "nonInteractive" => Access::NonInteractive,
                                _ => Access::Open,
                            };
                        }
                    }
                    Node::Draw(d) => {
                        if let Some(p) = presence {
                            d.common.presence = p;
                        }
                    }
                    Node::ExclGroup(g) => {
                        if let Some(p) = presence {
                            g.common.presence = p;
                        }
                        let fsibs = SomIndices::of_fields(&g.fields);
                        for (fi, f) in g.fields.iter_mut().enumerate() {
                            let fsom = child_som(&som, f.common.name.as_deref().or(Some("field")), fsibs.at(fi));
                            if presence_on && let Some(p) = ov.presence.get(&fsom) {
                                f.common.presence = Presence::parse(Some(p));
                            }
                            if props
                                && !inner
                                && let Some(sets) = ov.props.get(&fsom)
                            {
                                let mut node = Node::Field(Box::new(std::mem::take(f)));
                                if crate::restyle::restyle(&mut node, sets) {
                                    self.cut = self.cut.saturating_add(1);
                                }
                                if let Node::Field(changed) = node {
                                    *f = *changed;
                                }
                                self.placed.insert(fsom);
                            }
                        }
                    }
                }
            }
        }
    }
    let mut w = W { ov, placed: Default::default(), cut: 0 };
    let mut t = tpl.clone();
    if !ov.is_empty() {
        let props = !ov.props.is_empty();
        let mut root = vec![Node::Subform(Box::new(t.root.clone()))];
        w.walk(&mut root, "", props, true, false, 0);
        if let Some(Node::Subform(r)) = root.pop() {
            t.root = *r;
        }
    }
    let unplaced = ov.props.keys().filter(|som| !w.placed.contains(*som)).count();
    Applied { template: t, unplaced, cut: w.cut }
}

/// The field at `som` as the template and the property changes in `ov` make it now (its
/// validation's format test, its calculation's override), for checks after the user changes
/// its value; `None` when no template field is there. Only the field's own changes, kept by its
/// SOM path, apply to it (what [`apply_overrides_split`] gives it in the template), so this
/// restyles one field, not the form, and only once for the same changes:
/// the field is taken from the template's memo ([`crate::memo::restyled_field`]), as the live
/// form takes it, so the checks of later events don't read it again.
pub fn field_now(tpl: &Template, ov: &Overrides, som: &str) -> Option<Field> {
    let f = crate::data::template_field(tpl, som)?;
    Some(match ov.props().get(som) {
        Some(sets) => Field::clone(&crate::memo::restyled_field(&tpl.memo, f, sets)),
        None => f.clone(),
    })
}

/// Whether the calculation of the field at `som` is overridden (its value says so,
/// `value@override`, XFA 3.3 p. 883, as the template or a script set it), so that calculations
/// leave it alone. Only a change to the value's `override` can change what the template says,
/// so the field is restyled ([`field_now`]) only when one is kept.
pub fn value_overridden(tpl: &Template, ov: &Overrides, som: &str) -> bool {
    let key = crate::restyle::prop_key("value#0", Some("override"));
    if ov.props().get(som).is_some_and(|sets| sets.contains_key(&key)) {
        return field_now(tpl, ov, som).is_some_and(|f| f.value_override);
    }
    crate::data::template_field(tpl, som).is_some_and(|f| f.value_override)
}

/// Notes for the layout's warnings: property changes the file held that weren't kept
/// ([`Overrides::dropped`], not applied), objects some of whose changes don't reach the page
/// ([`Applied::cut`]) and kept changes whose object the template doesn't make once
/// ([`Applied::unplaced`]).
pub fn property_notes(ov: &Overrides, applied: &Applied) -> Vec<String> {
    let mut out = Vec::new();
    if ov.dropped > 0 {
        out.push(format!(
            "{} property change(s) kept in the file are not ones a script could make or are past the form's limits; they are left out and not applied",
            ov.dropped
        ));
    }
    if applied.cut > 0 {
        out.push(format!(
            "{} object(s) have more properties than are kept for scripts; some of the changes scripts made to them are not drawn",
            applied.cut
        ));
    }
    if applied.unplaced > 0 {
        out.push(format!(
            "{} object(s) with property changes kept in the file are not ones the template makes once at their paths; those changes are not drawn",
            applied.unplaced
        ));
    }
    out
}

/// The generated fields of the AcroForm by SOM path: `som → (field name, is a radio group)`.
pub fn fields_by_som(doc: &Document) -> HashMap<String, String> {
    field_objects_by_som(doc).into_iter().map(|(som, (name, _))| (som, name)).collect()
}

/// Resolve each generated SOM path to its AcroForm name and exact field dictionary.
pub fn field_objects_by_som(doc: &Document) -> HashMap<String, (String, pdfcraft_cos::ObjRef)> {
    // Each field is read once: a field listed twice in `/Kids` (or among its own kids) would
    // otherwise be walked again for every path to it, doubling the work at each level. Fields
    // are told apart by object number, as `Document::get` finds them, so `5 0 R` and `5 1 R`
    // are one. A field is read again only when it is reached shallower than before: first reached
    // at `MAX_DEPTH`, its kids would otherwise never be read. `steps` counts every entry looked at, references or not, so fields sharing one
    // long `/Kids` array stay bounded too; `walk_all` returns false once it is spent, and every
    // loop then stops.
    struct Walk {
        out: HashMap<String, (String, pdfcraft_cos::ObjRef)>,
        /// The shallowest depth each object number was read at.
        seen: HashMap<u32, usize>,
        steps: usize,
    }
    fn walk_all(doc: &Document, kids: &[Object], prefix: &str, w: &mut Walk, depth: usize) -> bool {
        kids.iter().all(|k| {
            w.steps += 1;
            if w.steps > MAX_FIELD_STEPS || w.out.len() > 100_000 {
                return false;
            }
            k.as_ref().is_none_or(|r| walk(doc, r, prefix, w, depth))
        })
    }
    fn walk(doc: &Document, r: pdfcraft_cos::ObjRef, prefix: &str, w: &mut Walk, depth: usize) -> bool {
        if depth > MAX_DEPTH || w.seen.get(&r.num).is_some_and(|&read| read <= depth) {
            return true;
        }
        w.seen.insert(r.num, depth);
        let o = doc.get(r);
        let Some(d) = o.as_dict() else { return true };
        let t = d.get(b"T").map(|t| doc.resolve(t)).and_then(|t| t.as_string().map(|s| s.to_text()));
        let name = match (&t, prefix.is_empty()) {
            (Some(t), true) => t.clone(),
            (Some(t), false) => format!("{prefix}.{t}"),
            (None, _) => prefix.to_string(),
        };
        if let Some(som) = d.get(crate::pdf::SOM_KEY).map(|s| doc.resolve(s)).and_then(|s| s.as_string().map(|s| s.to_text()))
            && t.is_some()
        {
            w.out.insert(som, (name.clone(), r));
        }
        // Read in place: a copy of the array at every level would cost its length each time.
        let kids = d.get(b"Kids").map(|k| doc.resolve(k));
        walk_all(doc, kids.as_deref().and_then(Object::as_array).map(Vec::as_slice).unwrap_or_default(), &name, w, depth + 1)
    }
    let Some((_, acro)) = acroform(doc) else { return HashMap::new() };
    let fields = acro.get(b"Fields").map(|f| doc.resolve(f));
    let mut w = Walk { out: HashMap::new(), seen: HashMap::new(), steps: 0 };
    walk_all(doc, fields.as_deref().and_then(Object::as_array).map(Vec::as_slice).unwrap_or_default(), "", &mut w, 0);
    w.out
}

/// What a reset gives the field or exclusion group at `som` in the data: its template default
/// as saved (a choice list's item value, not its shown text, as [`Field::saved_value`] gives
/// it), or nothing when it has none. A group's default is the on value of the first of its
/// buttons whose own default is that on value. `None` for an object a reset restores no value
/// of: not a field or group of the template, or a button, signature or picture field.
pub fn reset_default(tpl: &Template, som: &str) -> Option<String> {
    match crate::data::template_object(tpl, som)? {
        Node::Field(f) => {
            if matches!(f.ui, crate::model::Ui::Button | crate::model::Ui::Signature | crate::model::Ui::ImageEdit) {
                return None;
            }
            Some(f.value.plain().map(|v| f.saved_value(v.trim())).unwrap_or_default())
        }
        Node::ExclGroup(g) => Some(
            g.fields
                .iter()
                .find_map(|f| {
                    let on = f.items.first().map(|s| s.trim()).filter(|s| !s.is_empty())?;
                    (f.value.plain()?.trim() == on).then(|| on.to_string())
                })
                .unwrap_or_default(),
        ),
        _ => None,
    }
}

/// How many pages the layout made from each named page area of `tpl`: the master content of the
/// n-th page from area `master` is `master[n-1]`. Each page the layout wrote names its area
/// ([`crate::AREA_KEY`]), so a page whose master fields are all hidden still counts; a file laid
/// out before pages were named shows it by its generated fields. Areas neither shows are left out
/// (one page's worth, to [`form_tree_paged`]).
pub fn page_area_uses(doc: &Document, tpl: &Template) -> HashMap<String, usize> {
    let mut out: HashMap<String, usize> = HashMap::new();
    let Some(ps) = &tpl.root.page_set else { return out };
    let is_area = |name: &str| tpl.root.common.name.as_deref() != Some(name) && ps.areas.iter().any(|a| a.name.as_deref() == Some(name));
    // The flat page list laying the form out writes.
    let pages = doc.root().and_then(|r| doc.get(r).as_dict().and_then(|c| c.get(b"Pages")).map(|p| doc.resolve(p)));
    let kids =
        pages.and_then(|p| p.as_dict().and_then(|d| d.get(b"Kids")).map(|k| doc.resolve(k))).and_then(|k| k.as_array().cloned()).unwrap_or_default();
    for page in kids.iter().filter_map(Object::as_ref).take(crate::layout::MAX_PAGES) {
        let page = doc.get(page);
        let Some(name) = page.as_dict().and_then(|d| d.get(crate::AREA_KEY)).and_then(Object::as_string).map(PdfString::to_text) else { continue };
        if is_area(&name) {
            let pages = out.entry(name).or_insert(0);
            *pages = pages.saturating_add(1).min(crate::layout::MAX_PAGES);
        }
    }
    for som in fields_by_som(doc).keys() {
        let first = som.split('.').next().unwrap_or_default();
        let Some((name, rest)) = first.split_once('[') else { continue };
        let Some(index) = rest.strip_suffix(']').and_then(|i| i.parse::<usize>().ok()) else { continue };
        if !is_area(name) {
            continue;
        }
        let pages = out.entry(name.to_string()).or_insert(1);
        *pages = (*pages).max(index.saturating_add(1)).min(crate::layout::MAX_PAGES);
    }
    out
}

/// Lay the form out again from its template, data and overrides, replacing the pages and the
/// generated fields (a script added a row or hid a subform).
pub fn rerender(doc: &mut Document, tpl: &Template) -> Result<Report, XfaError> {
    let packets = crate::read_packets(doc)?.ok_or(XfaError::NotXfa)?;
    let data = parse_datasets(&packets.xdp);
    let ov = overrides_of(doc, tpl);
    let applied = apply_overrides_split(tpl, &ov);
    let mut form = crate::layout::layout_with_overrides(&applied.template, data.as_ref(), &ov)?;
    form.warnings.extend(property_notes(&ov, &applied));
    if form.pages.is_empty() {
        return Err(XfaError::Malformed("the template laid out to no pages".into()));
    }
    // The fields an earlier pass generated give way to the new layout's, after it is written: it
    // keeps a signature one of them holds. The AcroForm keeps every field PdfCraft did not generate.
    let Some(root) = doc.root() else { return Err(XfaError::NotXfa) };
    let top = |doc: &Document| -> Vec<Object> {
        acroform(doc).and_then(|(_, acro)| acro.get(b"Fields").map(|f| doc.resolve(f)).and_then(|f| f.as_array().cloned())).unwrap_or_default()
    };
    let generated: std::collections::HashSet<pdfcraft_cos::ObjRef> = top(doc)
        .iter()
        .filter_map(Object::as_ref)
        .filter(|r| doc.get(*r).as_dict().is_some_and(|d| d.contains(crate::pdf::SOM_KEY) || d.contains(crate::pdf::NODE_KEY)))
        .collect();
    let written = crate::pdf::write_form(doc, &form)?;
    if !generated.is_empty()
        && let Some((acro_ref, mut acro)) = acroform(doc)
    {
        let kept: Vec<Object> = top(doc).into_iter().filter(|f| f.as_ref().is_none_or(|r| !generated.contains(&r))).collect();
        acro.set(b"Fields".to_vec(), Object::Array(kept));
        match acro_ref {
            Some(r) => doc.set(r, Object::Dict(acro)),
            None => doc.update_dict(root, |c| c.set(b"AcroForm".to_vec(), Object::Dict(acro)))?,
        }
        // A generated field edited in this overlay is freed with its appearance and widget kids,
        // so a replaced password widget's value or masked appearance cannot stay in the overlay.
        // Signature fields, and the fields that name them by their SOM path, stay: the new
        // layout's signature field took their value and appearance.
        let free_ap = |doc: &mut Document, d: &Dict| {
            if let Some(ap) = d.get(b"AP").map(|a| doc.resolve(a)).and_then(|a| a.as_dict().cloned())
                && let Some(n) = ap.get(b"N").and_then(Object::as_ref)
                && doc.is_edited(n.num)
            {
                doc.free(n);
            }
        };
        let signature = |d: &Dict| d.name(b"FT") == Some(b"Sig".as_slice()) || d.contains(crate::pdf::NODE_KEY);
        for r in generated {
            let Some(d) = doc.get(r).as_dict().cloned().filter(|d| !signature(d)) else { continue };
            free_ap(doc, &d);
            if let Some(kids) = d.get(b"Kids").map(|k| doc.resolve(k)).and_then(|k| k.as_array().cloned()) {
                for k in kids.iter().filter_map(Object::as_ref) {
                    if let Some(kd) = doc.get(k).as_dict().cloned() {
                        if signature(&kd) {
                            continue;
                        }
                        free_ap(doc, &kd);
                    }
                    if doc.is_edited(k.num) {
                        doc.free(k);
                    }
                }
            }
            if doc.is_edited(r.num) {
                doc.free(r);
            }
        }
    }
    let mut warnings = form.warnings;
    warnings.extend(written.warnings);
    if written.usage_rights {
        warnings.push(crate::pdf::USAGE_RIGHTS_REMOVED.into());
    }
    warnings.dedup();
    Ok(Report { pages: written.pages, fields: written.fields, template_bytes: packets.xdp.len(), warnings })
}

/// The parsed template of a document's XFA packets.
pub fn template_of(doc: &Document) -> Result<Template, XfaError> {
    let packets = crate::read_packets(doc)?.ok_or(XfaError::NotXfa)?;
    if packets.xdp.trim().is_empty() {
        return Err(XfaError::Malformed("the XFA packets hold no template".into()));
    }
    let (mut tpl, _) = crate::parse(&packets.xdp)?;
    crate::linked_images::resolve(&mut tpl, doc);
    Ok(tpl)
}

/// Whether the template field at `som` is multiline text without a fixed height.
/// A change to its value can require a new layout, including new continuation pages.
pub fn field_grows_with_text(tpl: &Template, som: &str) -> bool {
    crate::data::template_field(tpl, som).is_some_and(Field::grows_with_text)
}

/// Is the field at `som` embedded in rich text (a floating field, XFA 3.3 p. 1222)? Its value
/// is shown in that text, so a new one needs the form laid out again. A page area's fields count
/// too, and so does any field bound to the same data as an embedded one.
pub fn field_is_embedded(tpl: &Template, som: &str) -> bool {
    !tpl.embedded_paths.is_empty() && path_is_embedded(tpl, &data_path(tpl, som))
}

/// The fields rich text embeds, by the SOM path the layout gives them (a repeating subform's
/// first instance), with their template defaults: what a reset gives a floating field that has
/// no widget of its own.
pub fn embedded_fields(tpl: &Template) -> Vec<(String, String)> {
    crate::data::fields_with_ids(tpl)
        .into_iter()
        .filter(|(id, _, _)| tpl.embedded.contains(*id))
        .map(|(_, som, f)| (som, f.value.plain().unwrap_or_default()))
        .collect()
}

/// Does `path` hold the data of a field embedded in rich text, in any instance of a repeating
/// subform? See [`field_is_embedded`].
pub fn path_is_embedded(tpl: &Template, path: &crate::DataPath) -> bool {
    !tpl.embedded_paths.is_empty() && tpl.embedded_paths.contains(&path.iter().map(|(name, _)| name.clone()).collect::<Vec<_>>())
}

/// Give the field at `som` in `live`, and every other field bound to the same data node (a
/// field inside an area shares its node with one outside, since areas are left out of data
/// paths), the value [`form_tree`] would give it from `data` now, so a value change needs no
/// rebuild of the tree. The walk follows the data path only (through areas and unnamed
/// containers, which have no data segment), so it costs the depth, not the form. `None`: a
/// node bound there is not a plain field the tree can refresh alone (a radio button, an
/// exclusion group, an object the template doesn't have): rebuild the tree.
pub fn refresh_value(live: &mut FormNode, tpl: &Template, data: Option<&DataNode>, som: &str) -> Option<bool> {
    crate::data::template_field(tpl, som)?;
    let path = data_path(tpl, som);
    if path.is_empty() {
        return None;
    }
    /// Down the one branch of `n` that `path` leads to from segment `pos`, giving each field
    /// bound there its value from `data`; `None` when something there needs a rebuild.
    fn walk(
        n: &mut FormNode,
        tpl: &Template,
        data: Option<&DataNode>,
        path: &[(String, usize)],
        pos: usize,
        changed: &mut bool,
        depth: usize,
    ) -> Option<()> {
        if depth > MAX_DEPTH {
            return Some(());
        }
        for c in n.children.iter_mut() {
            // No data segment of their own: looked through.
            let transparent = matches!(c.kind, Some(NodeKind::Area)) || (matches!(c.kind, Some(NodeKind::Subform)) && c.name.starts_with('#'));
            if transparent {
                walk(c, tpl, data, path, pos, changed, depth + 1)?;
                continue;
            }
            let Some((name, index)) = path.get(pos) else { continue };
            if c.name != *name || c.index != *index {
                continue;
            }
            if pos + 1 < path.len() {
                walk(c, tpl, data, path, pos + 1, changed, depth + 1)?;
                continue;
            }
            // Bound to the node.
            if c.kind != Some(NodeKind::Field) {
                return None;
            }
            let f = crate::data::template_field(tpl, &c.som)?;
            let value = Walker::plain_field_value(tpl, data, f, &c.som);
            if c.value != value {
                c.value = value;
                *changed = true;
            }
        }
        Some(())
    }
    let mut changed = false;
    // The root subform is the path's first segment.
    let (first, rest) = path.split_first()?;
    if live.name != first.0 || live.index != first.1 {
        return Some(false);
    }
    if rest.is_empty() {
        return None;
    }
    walk(live, tpl, data, &path, 1, &mut changed, 0)?;
    Some(changed)
}

/// Bring the live node at `som` up to date with the property changes `ov` holds for it, as
/// [`form_tree`] would build it, so a property change needs no rebuild of the tree. `false`:
/// there is no such node, or it is not one the tree can refresh alone: rebuild the tree. A
/// change to how often a subform occurs, or one turning a validation's script off, changes
/// more than the node (the instance manager, the events) and is the caller's to rebuild for.
pub fn refresh_props(live: &mut FormNode, ov: &Overrides, som: &str) -> bool {
    /// The node at `som` under `n`, followed down the path: only children on the way to it (whose
    /// SOM path starts it) or sharing their parent's (unnamed ones) are looked into, so finding
    /// one costs the children of the nodes on the way to it, not the form.
    fn find<'a>(n: &'a mut FormNode, som: &str, depth: usize) -> Option<&'a mut FormNode> {
        if n.som == som {
            return Some(n);
        }
        if depth > MAX_DEPTH {
            return None;
        }
        let own = n.som.clone();
        n.children
            .iter_mut()
            .filter(|c| c.som == own || c.som == som || som.strip_prefix(c.som.as_str()).is_some_and(|r| r.starts_with('.')))
            .find_map(|c| find(c, som, depth + 1))
    }
    let Some(sets) = ov.props.get(som) else { return false };
    let Some(n) = find(live, som, 0) else { return false };
    // The changes applied to the template's tree, as building the form again applies them:
    // over the node's tree, an edge or corner an earlier change made would stay the copy it was
    // then, not the copy of the side as it is drawn now. The node keeps the template's tree, so
    // it needs no template look-up (which a name holding a `.` defeats).
    let props = crate::restyle::apply_props(&n.template_props, sets).0;
    if n.kind == Some(NodeKind::Field) && !props.cut {
        // A field's display picture, as the template reader reads it (`format.picture`); a tree
        // cut short keeps the template's, as the reader does.
        n.display_picture = display_picture_of(&props);
    }
    n.props = Arc::new(props);
    true
}

/// The display picture a field's property tree gives (`format` > `picture`, trimmed; none when
/// empty), as the template reader reads it.
fn display_picture_of(props: &Props) -> Option<String> {
    let format = props.elements.iter().find(|e| e.tag == "format")?;
    let picture = format.children.iter().find(|e| e.tag == "picture")?;
    picture.text.as_deref().map(str::trim).filter(|t| !t.is_empty()).map(str::to_string)
}

/// The data tree of a document's datasets packet.
pub fn data_of(doc: &Document) -> Option<DataNode> {
    crate::read_packets(doc).ok().flatten().and_then(|p| parse_datasets(&p.xdp))
}
