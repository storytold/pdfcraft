use super::model::{
    MAX_ALERTS, MAX_CONSOLE, MAX_DEPTH, MAX_EFFECTS, MAX_NODES, MAX_PAYLOAD, MAX_VALUE, XfaDoc, XfaEffect, XfaKind, XfaNode, clip, clip_value,
    effect_key, payload,
};
use super::*;

/// One node of the flattened form, shared by the JavaScript and FormCalc engines.
pub(crate) struct HNode {
    pub(crate) name: String,
    pub(crate) som: String,
    pub(crate) kind: XfaKind,
    pub(crate) value: String,
    pub(crate) numeric: bool,
    pub(crate) presence: String,
    pub(crate) access: String,
    pub(crate) repeatable: bool,
    pub(crate) occur_min: usize,
    pub(crate) occur_max: Option<usize>,
    pub(crate) index: usize,
    pub(crate) parent: Option<usize>,
    pub(crate) children: Vec<usize>,
    /// Removed by `removeInstance`: skipped everywhere.
    pub(crate) gone: bool,
    /// See [`XfaNode::formatted`]; dropped when a script sets the value, which it no longer shows.
    pub(crate) formatted: Option<String>,
}

/// The form as a script sees and changes it, with the effects it records.
pub(crate) struct XHost {
    pub(crate) nodes: Vec<HNode>,
    pub(crate) effects: Vec<XfaEffect>,
    pub(crate) console: Vec<String>,
    pub(crate) doc: XfaDoc,
    pub(super) added: usize,
    /// Index of `this`.
    pub(crate) current: usize,
    /// Where the effect setting a property of an object is in `effects`, by (property, SOM):
    /// setting it again replaces that effect (the last value wins) unless something that
    /// depends on order (rows added or removed, a reset) came after it.
    set_at: HashMap<(u8, String), usize>,
    /// Effects before this index are behind an order-dependent effect.
    barrier: usize,
    alerts: usize,
    dropped_effects: usize,
    clipped_values: usize,
    dropped_alerts: usize,
    dropped_console: usize,
    /// Bytes of text kept in effects, console lines and values written, against [`MAX_PAYLOAD`].
    kept: usize,
    dropped_bytes: usize,
}

impl XHost {
    /// The host for a script on `root` (the template's root subform), with `this` at the node
    /// named by `target`. `xfa.form` is a synthetic root whose child is the root subform.
    pub(crate) fn new(root: XfaNode, target: &str, doc: &XfaDoc) -> XHost {
        let form = XfaNode { name: "form".into(), kind: Some(XfaKind::Form), children: vec![root], ..Default::default() };
        let mut nodes = Vec::new();
        flatten(&form, None, &mut nodes, 0);
        let current = nodes.iter().position(|n| n.som == target && n.kind != XfaKind::Form).unwrap_or(1.min(nodes.len().saturating_sub(1)));
        XHost {
            nodes,
            effects: Vec::new(),
            console: Vec::new(),
            doc: doc.clone(),
            added: 0,
            current,
            set_at: HashMap::new(),
            barrier: 0,
            alerts: 0,
            dropped_effects: 0,
            clipped_values: 0,
            dropped_alerts: 0,
            dropped_console: 0,
            kept: 0,
            dropped_bytes: 0,
        }
    }

    /// Room for `bytes` more bytes of kept text: charged when there is, a drop counted when not.
    fn room(&mut self, bytes: usize) -> bool {
        match self.kept.checked_add(bytes).filter(|k| *k <= MAX_PAYLOAD) {
            Some(k) => {
                self.kept = k;
                true
            }
            None => {
                self.dropped_bytes = self.dropped_bytes.saturating_add(1);
                false
            }
        }
    }

    /// Record an effect: values, presence and access set again on the same object replace the
    /// earlier one; past [`MAX_EFFECTS`] (or [`MAX_ALERTS`] message boxes, or [`MAX_PAYLOAD`]
    /// bytes of text) effects are counted and dropped.
    pub(crate) fn push(&mut self, e: XfaEffect) {
        let e = self.clip_effect(e);
        if self.room(payload(&e)) {
            self.record(e);
        }
    }

    /// Room for `e`, which holds `bytes` of text, charged now: a change to the snapshot is made
    /// only when the effect that applies it will be kept, so a script never sees a change its
    /// effects leave out. An effect that replaces an earlier one needs no new slot. A drop is
    /// counted when there is no room.
    pub(super) fn reserve(&mut self, e: &XfaEffect, bytes: usize) -> bool {
        let replaces = effect_key(e).and_then(|k| self.set_at.get(&k)).is_some_and(|&at| at >= self.barrier && at < self.effects.len());
        if !replaces && self.effects.len() >= MAX_EFFECTS {
            self.dropped_effects += 1;
            return false;
        }
        self.room(bytes)
    }

    /// Set the presence or access of node `i` to what `e` (a [`XfaEffect::SetPresence`] or
    /// [`XfaEffect::SetAccess`]) says, in the snapshot only when the effect is kept.
    pub(crate) fn set_state(&mut self, i: usize, e: XfaEffect) {
        if !self.reserve(&e, payload(&e)) {
            return;
        }
        if let Some(n) = self.nodes.get_mut(i) {
            match &e {
                XfaEffect::SetPresence { presence, .. } => n.presence = presence.clone(),
                XfaEffect::SetAccess { access, .. } => n.access = access.clone(),
                _ => {}
            }
        }
        self.record(e);
    }

    /// `e` with its text cut to the lengths an effect keeps.
    fn clip_effect(&mut self, e: XfaEffect) -> XfaEffect {
        match e {
            XfaEffect::MessageBox(m) => XfaEffect::MessageBox(clip(m)),
            XfaEffect::LaunchUrl(u) => XfaEffect::LaunchUrl(clip(u)),
            XfaEffect::SetFocus(s) => XfaEffect::SetFocus(clip(s)),
            XfaEffect::ResetData(names) => XfaEffect::ResetData(names.into_iter().map(clip).collect()),
            XfaEffect::SetValue { som, value } => {
                let (value, cut) = clip_value(value);
                if cut {
                    self.clipped_values += 1;
                }
                XfaEffect::SetValue { som, value }
            }
            other => other,
        }
    }

    /// Keep `e`, whose bytes are already charged, against the count limits.
    pub(super) fn record(&mut self, e: XfaEffect) {
        if let Some(key) = effect_key(&e) {
            if let Some(&at) = self.set_at.get(&key)
                && at >= self.barrier
                && let Some(slot) = self.effects.get_mut(at)
            {
                *slot = e;
                return;
            }
            if self.effects.len() >= MAX_EFFECTS {
                self.dropped_effects += 1;
                return;
            }
            self.set_at.insert(key, self.effects.len());
            self.effects.push(e);
            return;
        }
        if matches!(e, XfaEffect::MessageBox(_)) {
            if self.alerts >= MAX_ALERTS {
                self.dropped_alerts += 1;
                return;
            }
            self.alerts += 1;
        }
        if self.effects.len() >= MAX_EFFECTS {
            self.dropped_effects += 1;
            return;
        }
        if matches!(e, XfaEffect::AddInstance { .. } | XfaEffect::RemoveInstance { .. } | XfaEffect::ResetData(_)) {
            self.barrier = self.effects.len() + 1;
        }
        self.effects.push(e);
    }

    pub(crate) fn print(&mut self, line: String) {
        if self.console.len() >= MAX_CONSOLE {
            self.dropped_console += 1;
        } else if self.room(line.len()) {
            self.console.push(line);
        }
    }

    /// What the limits left out.
    pub(crate) fn notes(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.dropped_effects > 0 {
            out.push(format!("the script made more than {MAX_EFFECTS} changes; {} more were left out", self.dropped_effects));
        }
        if self.dropped_alerts > 0 {
            out.push(format!("the script showed more than {MAX_ALERTS} messages; {} more were left out", self.dropped_alerts));
        }
        if self.dropped_console > 0 {
            out.push(format!("the script printed more than {MAX_CONSOLE} console lines; {} more were left out", self.dropped_console));
        }
        if self.dropped_bytes > 0 {
            out.push(format!("the script's changes and output passed {} MiB of text; {} more were left out", MAX_PAYLOAD >> 20, self.dropped_bytes));
        }
        if self.clipped_values > 0 {
            out.push(format!("the script set {} value(s) longer than {MAX_VALUE} characters; they were cut", self.clipped_values));
        }
        out
    }

    /// Give node `i` the value `text` when it is a field or exclusion group (nothing else
    /// holds a value; writing one into a subform would put text where another viewer expects
    /// a data group). Returns whether it was.
    pub(crate) fn set_value(&mut self, i: usize, text: String) -> bool {
        let Some(n) = self.nodes.get(i) else { return false };
        if n.gone || !matches!(n.kind, XfaKind::Field | XfaKind::ExclGroup) {
            return false;
        }
        let som = n.som.clone();
        // Cut here, not only in the effect: later reads in the script see what is saved, and
        // the snapshot stays bounded however often a long value is written.
        let (text, cut) = clip_value(text);
        if cut {
            self.clipped_values += 1;
        }
        // The snapshot's copy and the effect's are charged together, before either is kept:
        // past the byte budget or the effect cap the write is left out of both.
        let effect = XfaEffect::SetValue { som, value: text.clone() };
        if !self.reserve(&effect, text.len().saturating_add(payload(&effect))) {
            return true;
        }
        if let Some(n) = self.nodes.get_mut(i) {
            n.value = text;
            n.formatted = None;
        }
        self.record(effect);
        true
    }
}

pub(super) type Shared = RefCell<XHost>;

/// The form host of the script running in `ctx`.
pub(super) fn host(ctx: &Context) -> JsResult<&Shared> {
    ctx.get_data::<Shared>().ok_or_else(|| error("the XFA script host is not set up"))
}

/// The snapshot `n` and its descendants flattened into `out` under `parent` (to [`MAX_DEPTH`]
/// and [`MAX_NODES`]): the index of `n`'s node, `None` past the limits.
fn flatten(n: &XfaNode, parent: Option<usize>, out: &mut Vec<HNode>, depth: usize) -> Option<usize> {
    if depth > MAX_DEPTH || out.len() >= MAX_NODES {
        return None;
    }
    let idx = out.len();
    out.push(HNode {
        name: n.name.clone(),
        som: n.som.clone(),
        kind: n.kind.unwrap_or(XfaKind::Subform),
        value: n.value.clone(),
        numeric: n.numeric,
        presence: if n.presence.is_empty() { "visible".into() } else { n.presence.clone() },
        access: if n.access.is_empty() { "open".into() } else { n.access.clone() },
        repeatable: n.repeatable,
        occur_min: n.occur_min,
        occur_max: n.occur_max,
        index: n.index,
        parent,
        children: Vec::new(),
        gone: false,
        formatted: n.formatted.clone(),
    });
    let mut kids = Vec::new();
    for c in &n.children {
        if let Some(ci) = flatten(c, Some(idx), out, depth + 1) {
            kids.push(ci);
        }
    }
    if let Some(h) = out.get_mut(idx) {
        h.children = kids;
    }
    Some(idx)
}
