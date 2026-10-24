//! The form as scripts see it (XFA 3.3 part 6): the instance tree with values and presence, the
//! events a template declares, the presence overrides scripts make (kept in the PDF so they
//! survive undo and reopening), and laying the form out again after scripts changed it.

use std::collections::{BTreeMap, HashMap};

use pdfcraft_cos::{Dict, Document, Object, PdfString};

use crate::data::{DataNode, data_path, parse_datasets};
use crate::model::{Access, Field, Node, Presence, Script, SomIndices, Template};
use crate::{Report, XfaError};

/// Private key on the AcroForm: presence scripts set, by SOM path.
pub const OVERRIDES_KEY: &[u8] = b"PCXfaOverrides";

const MAX_DEPTH: usize = 64;
const MAX_EVENTS: usize = 20_000;
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
}

impl Overrides {
    pub fn is_empty(&self) -> bool {
        self.presence.is_empty() && self.access.is_empty()
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
    let mut out = Overrides::default();
    let Some((_, acro)) = acroform(doc) else { return out };
    let Some(o) = acro.get(OVERRIDES_KEY).map(|o| doc.resolve(o)) else { return out };
    let Some(d) = o.as_dict() else { return out };
    for (which, map) in [(&b"Presence"[..], &mut out.presence), (&b"Access"[..], &mut out.access)] {
        if let Some(m) = d.get(which).map(|m| doc.resolve(m))
            && let Some(m) = m.as_dict()
        {
            for (k, v) in m.iter().take(MAX_OVERRIDES) {
                if let Some(v) = v.as_string() {
                    map.insert(String::from_utf8_lossy(k).into_owned(), v.to_text());
                }
            }
        }
    }
    out
}

/// Store `ov` in the document (none: the key is removed).
pub fn set_overrides(doc: &mut Document, ov: &Overrides) -> Result<(), XfaError> {
    let Some(root) = doc.root() else { return Ok(()) };
    let Some((acro_ref, mut acro)) = acroform(doc) else { return Ok(()) };
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
                    let from_data = match (&sf.common.name, self.data) {
                        (Some(name), Some(d)) if sf.occur.max != Some(1) => d.count(&data_path(self.tpl, parent_som), name),
                        _ => 0,
                    };
                    let instances = crate::layout::instance_count(&sf.occur, from_data);
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
                            repeatable: sf.occur.max != Some(1),
                            occur_min: sf.occur.min,
                            occur_max: sf.occur.max,
                            index,
                            children,
                            display_picture: None,
                        });
                    }
                }
                Node::Area(a) => {
                    let som = child_som(parent_som, a.common.name.as_deref(), sib);
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
                    let fields: Vec<Node> = g.fields.iter().map(|f| Node::Field(Box::new(f.clone()))).collect();
                    let fsibs = SomIndices::of(&fields);
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
    let mut w = Walker { tpl, data, ov, events: Vec::new(), script_bytes: 0, nodes: 0, truncated: false };
    let root = Node::Subform(Box::new(tpl.root.clone()));
    let mut nodes = w.children(std::slice::from_ref(&root), "", 0);
    let mut root_node = nodes.pop().unwrap_or_default();
    // Page masters' fields (Print and Save buttons, page numbers) belong to the layout model
    // in XFA; they hang off the root here, under their page area's name, as the layouter
    // names them, so their click scripts can run.
    if let Some(ps) = &tpl.root.page_set {
        for (i, area) in ps.areas.iter().enumerate() {
            let som = area.name.clone().map(|n| format!("{n}[0]")).unwrap_or_else(|| format!("#pageArea[{i}]"));
            let children = w.children(&area.items, &som, 1);
            if children.is_empty() {
                continue;
            }
            root_node.children.push(FormNode {
                name: area.name.clone().unwrap_or_else(|| "#pageArea".into()),
                som,
                kind: Some(NodeKind::Area),
                presence: "visible".into(),
                access: "open".into(),
                occur_min: 1,
                occur_max: Some(1),
                index: 0,
                children,
                ..Default::default()
            });
        }
    }
    LiveForm { root: root_node, events: w.events, truncated: w.truncated }
}

/// Does the template declare any script (an event, calculation or validation)? Forms without
/// one need no live form and run nothing.
pub fn has_scripts(tpl: &Template) -> bool {
    fn any(s: &[Script]) -> bool {
        s.iter().any(|s| !s.text.trim().is_empty())
    }
    fn field(f: &Field) -> bool {
        any(&f.scripts) || [&f.calculate, &f.validate].into_iter().flatten().any(|s| !s.text.trim().is_empty())
    }
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

/// The template with `ov` applied to its objects (instance 0 of each; scripts that hide one
/// instance of a repeating subform hide them all).
pub fn apply_overrides(tpl: &Template, ov: &Overrides) -> Template {
    /// Set the presence `ov` gives each of `nodes` (children of `parent_som`) and their descendants,
    /// down to [`MAX_DEPTH`], and the access it gives a field outside an exclusion group. An unnamed
    /// draw, area or subform shares its parent's SOM path, so it takes no override of its own.
    fn walk(nodes: &mut [Node], parent_som: &str, ov: &Overrides, depth: usize) {
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
                    walk(&mut s.children, &som, ov, depth + 1);
                }
                Node::Area(a) => {
                    if let Some(p) = presence {
                        a.common.presence = p;
                    }
                    walk(&mut a.children, &som, ov, depth + 1);
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
                    let fields: Vec<Node> = g.fields.iter().map(|f| Node::Field(Box::new(f.clone()))).collect();
                    let fsibs = SomIndices::of(&fields);
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
    let mut t = tpl.clone();
    if !ov.is_empty() {
        let mut root = vec![Node::Subform(Box::new(t.root.clone()))];
        walk(&mut root, "", ov, 0);
        if let Some(Node::Subform(r)) = root.pop() {
            t.root = *r;
        }
    }
    t
}

/// The generated fields of the AcroForm by SOM path: `som → (field name, is a radio group)`.
pub fn fields_by_som(doc: &Document) -> HashMap<String, String> {
    fn walk(doc: &Document, r: pdfcraft_cos::ObjRef, prefix: &str, out: &mut HashMap<String, String>, depth: usize) {
        if depth > MAX_DEPTH || out.len() > 100_000 {
            return;
        }
        let o = doc.get(r);
        let Some(d) = o.as_dict() else { return };
        let t = d.get(b"T").map(|t| doc.resolve(t)).and_then(|t| t.as_string().map(|s| s.to_text()));
        let name = match (&t, prefix.is_empty()) {
            (Some(t), true) => t.clone(),
            (Some(t), false) => format!("{prefix}.{t}"),
            (None, _) => prefix.to_string(),
        };
        if let Some(som) = d.get(crate::pdf::SOM_KEY).map(|s| doc.resolve(s)).and_then(|s| s.as_string().map(|s| s.to_text()))
            && t.is_some()
        {
            out.insert(som, name.clone());
        }
        if let Some(kids) = d.get(b"Kids").map(|k| doc.resolve(k)).and_then(|k| k.as_array().cloned()) {
            for k in kids.iter().filter_map(Object::as_ref) {
                walk(doc, k, &name, out, depth + 1);
            }
        }
    }
    let mut out = HashMap::new();
    let Some((_, acro)) = acroform(doc) else { return out };
    let fields = acro.get(b"Fields").map(|f| doc.resolve(f)).and_then(|f| f.as_array().cloned()).unwrap_or_default();
    for f in fields.iter().filter_map(Object::as_ref) {
        walk(doc, f, "", &mut out, 0);
    }
    out
}

/// Lay the form out again from its template, data and overrides, replacing the pages and the
/// generated fields (a script added a row or hid a subform).
pub fn rerender(doc: &mut Document, tpl: &Template) -> Result<Report, XfaError> {
    let packets = crate::read_packets(doc)?.ok_or(XfaError::NotXfa)?;
    let data = parse_datasets(&packets.xdp);
    let ov = overrides(doc);
    let laid = apply_overrides(tpl, &ov);
    let form = crate::layout(&laid, data.as_ref())?;
    if form.pages.is_empty() {
        return Err(XfaError::Malformed("the template laid out to no pages".into()));
    }
    // The AcroForm keeps only the fields PdfCraft did not generate.
    let Some(root) = doc.root() else { return Err(XfaError::NotXfa) };
    if let Some((acro_ref, mut acro)) = acroform(doc) {
        let fields = acro.get(b"Fields").map(|f| doc.resolve(f)).and_then(|f| f.as_array().cloned()).unwrap_or_default();
        let kept: Vec<Object> =
            fields.into_iter().filter(|f| f.as_ref().is_none_or(|r| doc.get(r).as_dict().is_none_or(|d| !d.contains(crate::pdf::SOM_KEY)))).collect();
        acro.set(b"Fields".to_vec(), Object::Array(kept));
        match acro_ref {
            Some(r) => doc.set(r, Object::Dict(acro)),
            None => doc.update_dict(root, |c| c.set(b"AcroForm".to_vec(), Object::Dict(acro)))?,
        }
    }
    let written = crate::pdf::write_form(doc, &form)?;
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
    crate::parse(&packets.xdp).map(|(t, _)| t)
}

/// Whether the template field at `som` is multiline text without a fixed height.
/// A change to its value can require a new layout, including new continuation pages.
pub fn field_grows_with_text(tpl: &Template, som: &str) -> bool {
    crate::data::template_field(tpl, som).is_some_and(Field::grows_with_text)
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

/// The data tree of a document's datasets packet.
pub fn data_of(doc: &Document) -> Option<DataNode> {
    crate::read_packets(doc).ok().flatten().and_then(|p| parse_datasets(&p.xdp))
}
