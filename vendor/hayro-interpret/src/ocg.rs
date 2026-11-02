use hayro_syntax::object::dict::keys::{
    AS, BASE_STATE, D, OCGS, OCMD, OCPROPERTIES, OFF, ON, P, TYPE, USAGE, VE, VIEW, VIEW_STATE,
};
use hayro_syntax::object::{Array, Dict, MaybeRef, Name, Object, ObjectIdentifier};
use hayro_syntax::xref::XRef;
use std::collections::{HashMap, HashSet};

pub(crate) struct OcgState {
    inactive_ocgs: HashSet<ObjectIdentifier>,
    /// PdfCraft patch (40): every group the document lists in `/OCProperties /OCGs`, so a
    /// visibility expression naming anything else is known to be malformed.
    known_ocgs: HashSet<ObjectIdentifier>,
    visibility_stack: Vec<bool>,
}

impl OcgState {
    /// PdfCraft patch (67): the marked-content visibility now, to put back after content that
    /// may leave it unbalanced is interpreted once more (`draw_knockout_passes`).
    pub(crate) fn visibility(&self) -> Vec<bool> {
        self.visibility_stack.clone()
    }

    /// PdfCraft patch (67): see `visibility`.
    pub(crate) fn set_visibility(&mut self, visibility: Vec<bool>) {
        self.visibility_stack = visibility;
    }

    fn dummy() -> Self {
        Self {
            inactive_ocgs: HashSet::default(),
            known_ocgs: HashSet::default(),
            visibility_stack: vec![],
        }
    }

    /// The groups' states as the document opens: the default configuration (`/BaseState`, then
    /// `/ON` and `/OFF`), then, when `view_auto_state` is set (PdfCraft patch (67)), the auto
    /// state its `/AS` array gives for the View event ([`apply_view_auto_state`]).
    pub(crate) fn from_catalog(catalog: &Dict<'_>, xref: &XRef, view_auto_state: bool) -> Self {
        let Some(oc_properties) = catalog.get::<Dict<'_>>(OCPROPERTIES) else {
            return Self::dummy();
        };

        let Some(config) = oc_properties.get::<Dict<'_>>(D) else {
            return Self::dummy();
        };

        let mut inactive = HashSet::new();
        let known: HashSet<ObjectIdentifier> = oc_properties
            .get::<Array<'_>>(OCGS)
            .map(|ocgs| {
                ocgs.raw_iter()
                    .filter_map(|item| item.as_obj_ref())
                    .map(Into::into)
                    .collect()
            })
            .unwrap_or_default();

        let base_state = config
            .get::<Name<'_>>(BASE_STATE)
            .and_then(|b| BaseState::from_name(b.as_ref()));

        if base_state.unwrap_or(BaseState::On) == BaseState::Off
            && let Some(ocgs) = oc_properties.get::<Array<'_>>(OCGS)
        {
            for item in ocgs.raw_iter() {
                if let Some(ref_) = item.as_obj_ref() {
                    let id: ObjectIdentifier = ref_.into();
                    inactive.insert(id);
                }
            }
        }

        let mut read_ocg_array = |key, insert_active: bool| {
            if let Some(arr) = config.get::<Array<'_>>(key) {
                for item in arr.raw_iter() {
                    if let Some(ref_) = item.as_obj_ref() {
                        let id: ObjectIdentifier = ref_.into();
                        if insert_active {
                            inactive.remove(&id);
                        } else {
                            inactive.insert(id);
                        }
                    }
                }
            }
        };

        read_ocg_array(ON, true);
        read_ocg_array(OFF, false);

        if view_auto_state {
            apply_view_auto_state(&config, xref, &mut inactive);
        }

        Self {
            inactive_ocgs: inactive,
            known_ocgs: known,
            visibility_stack: Vec::new(),
        }
    }

    pub(crate) fn begin_single_oc(&mut self, ocg_id: ObjectIdentifier) {
        let is_active = !self.inactive_ocgs.contains(&ocg_id);
        let visible = self.is_visible() && is_active;
        self.visibility_stack.push(visible);
    }

    pub(crate) fn begin_ocmd(&mut self, ocmd: &Dict<'_>) {
        // PdfCraft patch (40): a visibility expression takes precedence over /OCGs and /P
        // (ISO 32000-2 §8.11.2.2); a malformed one is ignored and they decide instead.
        if let Some(visible) = ocmd.get::<Array<'_>>(VE).and_then(|ve| self.visibility_expression(&ve, 0)) {
            let visible = self.is_visible() && visible;
            self.visibility_stack.push(visible);
            return;
        }

        let policy = ocmd
            .get::<Name<'_>>(P)
            .and_then(|n| OcmdPolicy::from_name(n.as_ref()))
            .unwrap_or(OcmdPolicy::AnyOn);

        let mut ocg_ids: Vec<ObjectIdentifier> = Vec::new();

        if let Some(arr) = ocmd.get::<Array<'_>>(OCGS) {
            for item in arr.raw_iter() {
                if let Some(ref_) = item.as_obj_ref() {
                    ocg_ids.push(ref_.into());
                }
            }
        } else if let Some(ref_) = ocmd.get_ref(OCGS) {
            ocg_ids.push(ref_.into());
        }

        let is_active = if ocg_ids.is_empty() {
            true
        } else {
            match policy {
                OcmdPolicy::AllOn => ocg_ids.iter().all(|id| !self.inactive_ocgs.contains(id)),
                OcmdPolicy::AnyOn => ocg_ids.iter().any(|id| !self.inactive_ocgs.contains(id)),
                OcmdPolicy::AnyOff => ocg_ids.iter().any(|id| self.inactive_ocgs.contains(id)),
                OcmdPolicy::AllOff => ocg_ids.iter().all(|id| self.inactive_ocgs.contains(id)),
            }
        };

        let visible = self.is_visible() && is_active;
        self.visibility_stack.push(visible);
    }

    /// PdfCraft patch (40): evaluate a visibility expression (`/VE`, PDF 1.6): `[/And e…]`,
    /// `[/Or e…]` or `[/Not e]`, each `e` an optional content group (by reference) or a nested
    /// expression. `None` for a malformed expression: an unknown operator, a `/Not` without
    /// exactly one operand, an `/And` or `/Or` without any, an operand that is neither (a
    /// reference to an object the document doesn't list as a group is neither), or nesting
    /// deeper than `MAX_VISIBILITY_EXPRESSION_DEPTH`.
    fn visibility_expression(&self, expr: &Array<'_>, depth: usize) -> Option<bool> {
        if depth > MAX_VISIBILITY_EXPRESSION_DEPTH {
            return None;
        }
        let mut items = expr.raw_iter();
        let operator = match items.next()? {
            MaybeRef::NotRef(Object::Name(name)) => name,
            _ => return None,
        };
        let (mut count, mut all, mut any) = (0usize, true, false);
        for item in items {
            let value = match item {
                MaybeRef::Ref(r) => {
                    let id: ObjectIdentifier = r.into();
                    if !self.known_ocgs.contains(&id) {
                        return None;
                    }
                    !self.inactive_ocgs.contains(&id)
                }
                MaybeRef::NotRef(Object::Array(nested)) => self.visibility_expression(&nested, depth + 1)?,
                _ => return None,
            };
            count = count.saturating_add(1);
            all &= value;
            any |= value;
        }
        match (operator.as_ref(), count) {
            (b"And", 1..) => Some(all),
            (b"Or", 1..) => Some(any),
            (b"Not", 1) => Some(!all),
            _ => None,
        }
    }

    pub(crate) fn begin_ocg(&mut self, props: &Dict<'_>, ref_id: ObjectIdentifier) {
        match props.get::<Name<'_>>(TYPE).as_deref() {
            Some(OCMD) => self.begin_ocmd(props),
            _ => self.begin_single_oc(ref_id),
        }
    }

    pub(crate) fn begin_marked_content(&mut self) {
        let visible = self.is_visible();
        self.visibility_stack.push(visible);
    }

    pub(crate) fn end_marked_content(&mut self) {
        self.visibility_stack.pop();
    }

    /// PdfCraft patch: override one group's state (viewer layer toggles).
    pub(crate) fn set_active(&mut self, id: ObjectIdentifier, active: bool) {
        if active {
            self.inactive_ocgs.remove(&id);
        } else {
            self.inactive_ocgs.insert(id);
        }
    }

    pub(crate) fn is_visible(&self) -> bool {
        self.visibility_stack.last().copied().unwrap_or(true)
    }
}

impl Default for OcgState {
    fn default() -> Self {
        Self::dummy()
    }
}

/// PdfCraft patch (67): at most this many usage application dictionaries of an `/AS` array are
/// read; the rest are ignored.
const MAX_AUTO_STATE_APPLICATIONS: usize = 256;
/// PdfCraft patch (67): at most this many `/OCGs` entries are read across all of them.
const MAX_AUTO_STATE_GROUPS: usize = 4096;
/// PdfCraft patch (67): at most this many `/Category` names are read per application.
const MAX_AUTO_STATE_CATEGORIES: usize = 16;

/// PdfCraft patch (67): apply the configuration's auto state for the View event (ISO 32000-2
/// §8.11.4.4, Tables 99 and 101). For each usage application dictionary in `/AS` whose `/Event`
/// is `/View`, each group in its `/OCGs` gets a recommended state from each category in its
/// `/Category` that the group's `/Usage` gives one for. A group is ON only if every
/// recommendation it receives, across all such dictionaries, is ON, and OFF otherwise; a group
/// that receives none keeps its configured state.
///
/// Only the `/View` category (`/ViewState`) is evaluated. `/Zoom`, `/User` and `/Language` need
/// the magnification, user and locale, which the interpreter doesn't know, so they give no
/// recommendation. Malformed entries (not a dictionary, no `/Category`, a `/ViewState` other
/// than ON or OFF) are skipped, and the walk is bounded by the caps above.
fn apply_view_auto_state(config: &Dict<'_>, xref: &XRef, inactive: &mut HashSet<ObjectIdentifier>) {
    let Some(applications) = config.get::<Array<'_>>(AS) else {
        return;
    };
    let mut recommended: HashMap<ObjectIdentifier, bool> = HashMap::new();
    let mut groups_left = MAX_AUTO_STATE_GROUPS;
    for application in applications.raw_iter().take(MAX_AUTO_STATE_APPLICATIONS) {
        let application = match application {
            MaybeRef::Ref(r) => xref.get::<Dict<'_>>(r.into()),
            MaybeRef::NotRef(Object::Dict(d)) => Some(d),
            MaybeRef::NotRef(_) => None,
        };
        let Some(application) = application else {
            continue;
        };
        if application.get::<Name<'_>>(b"Event").as_deref() != Some(VIEW) {
            continue;
        }
        let views = application.get::<Array<'_>>(b"Category").is_some_and(|categories| {
            categories
                .raw_iter()
                .take(MAX_AUTO_STATE_CATEGORIES)
                .any(|c| matches!(c, MaybeRef::NotRef(Object::Name(n)) if n.as_ref() == VIEW))
        });
        if !views {
            continue;
        }
        let Some(groups) = application.get::<Array<'_>>(OCGS) else {
            continue;
        };
        for group in groups.raw_iter() {
            if groups_left == 0 {
                break;
            }
            groups_left -= 1;
            let Some(r) = group.as_obj_ref() else {
                continue;
            };
            let id: ObjectIdentifier = r.into();
            let state = xref
                .get::<Dict<'_>>(id)
                .and_then(|g| g.get::<Dict<'_>>(USAGE))
                .and_then(|u| u.get::<Dict<'_>>(VIEW))
                .and_then(|v| v.get::<Name<'_>>(VIEW_STATE));
            let on = match state.as_deref() {
                Some(ON) => true,
                Some(OFF) => false,
                _ => continue,
            };
            let entry = recommended.entry(id).or_insert(true);
            *entry &= on;
        }
    }
    for (id, on) in recommended {
        if on {
            inactive.remove(&id);
        } else {
            inactive.insert(id);
        }
    }
}

/// PdfCraft patch (40): how deep visibility expressions may nest; deeper ones are malformed.
const MAX_VISIBILITY_EXPRESSION_DEPTH: usize = 32;

#[derive(Debug, PartialEq, Eq, Copy, Clone)]
enum BaseState {
    On,
    Off,
    Unchanged,
}

impl BaseState {
    fn from_name(name: &[u8]) -> Option<Self> {
        match name {
            b"ON" => Some(Self::On),
            b"OFF" => Some(Self::Off),
            b"Unchanged" => Some(Self::Unchanged),
            _ => None,
        }
    }
}

#[derive(Debug, PartialEq, Eq, Copy, Clone)]
enum OcmdPolicy {
    AllOn,
    AnyOn,
    AnyOff,
    AllOff,
}

impl OcmdPolicy {
    fn from_name(name: &[u8]) -> Option<Self> {
        match name {
            b"AllOn" => Some(Self::AllOn),
            b"AnyOn" => Some(Self::AnyOn),
            b"AnyOff" => Some(Self::AnyOff),
            b"AllOff" => Some(Self::AllOff),
            _ => None,
        }
    }
}
