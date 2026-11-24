//! A visual editor for properties of reachable indirect structure tags.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use craft::behavior::{DisplayOrder, ItemSource, Selection, SelectionChange};
use craft::theme::variant::Tone;
use craft::ui::buttons::Button;
use craft::ui::contract::{Bind, CollectionEvent, Outcome as _, Props as _, SelectMode, Show as _, ShowCollection as _, ShowContainer as _};
use craft::ui::display::{Shade, Text};
use craft::ui::form::{Field, FieldGroup};
use craft::ui::list::{List, ListRow};
use craft::ui::notices::Notice;
use craft::ui::text::TextField;
use egui::Ui;
use pdfcraft_cos::ObjRef;
use pdfcraft_engine::a11y::{TagContentItem, TagInfo, TagPlacement, TagProperty};
use pdfcraft_engine::{DocId, Edit};

#[derive(Clone, Debug, Default)]
pub(crate) struct State {
    doc: Option<DocId>,
    selected: Option<ObjRef>,
    collapsed: BTreeSet<ObjRef>,
    query: String,
    draft: Draft,
    /// The tag list per (document, edit generation): walking the structure tree is too slow
    /// to repeat every frame on a large tagged PDF.
    cache: Option<(DocId, u64, Arc<Vec<TagInfo>>)>,
    /// The selected tag's marked content per (document, edit generation, tag): reading it parses
    /// page content, so it isn't repeated every frame either.
    content_cache: Option<(DocId, u64, ObjRef, Arc<Vec<TagContentItem>>)>,
}

#[derive(Clone, Debug, Default)]
struct Draft {
    kind: String,
    title: String,
    actual_text: String,
    alt: String,
    language: String,
}

impl Draft {
    fn from_tag(tag: &TagInfo) -> Self {
        Self {
            kind: tag.kind.clone(),
            title: tag.title.clone().unwrap_or_default(),
            actual_text: tag.actual_text.clone().unwrap_or_default(),
            alt: tag.alt.clone().unwrap_or_default(),
            language: tag.language.clone().unwrap_or_default(),
        }
    }

    fn edit(&self, tag: &TagInfo) -> Option<Edit> {
        let mut edits = Vec::new();
        let mut add = |property, prior: Option<&str>, next: &str| {
            let value = (!next.is_empty()).then(|| next.to_owned());
            if prior != value.as_deref() {
                edits.push(Edit::SetTagProperty { object: tag.object, property, value });
            }
        };
        add(TagProperty::Type, Some(&tag.kind), &self.kind);
        add(TagProperty::Title, tag.title.as_deref(), &self.title);
        add(TagProperty::ActualText, tag.actual_text.as_deref(), &self.actual_text);
        add(TagProperty::AlternateText, tag.alt.as_deref(), &self.alt);
        add(TagProperty::Language, tag.language.as_deref(), &self.language);
        match edits.len() {
            0 => None,
            1 => edits.pop(),
            _ => Some(Edit::Batch { label: "Change tag properties".into(), edits }),
        }
    }
}

impl State {
    /// The selected tag's marked content, read again only after an edit or a new selection.
    pub(crate) fn content(&mut self, doc: &pdfcraft_engine::Document) -> Arc<Vec<TagContentItem>> {
        let Some(object) = self.selected else { return Arc::default() };
        let generation = doc.edit_generation();
        if let Some((id, g, o, content)) = &self.content_cache
            && *id == doc.id
            && *g == generation
            && *o == object
        {
            return content.clone();
        }
        let content = Arc::new(doc.tag_content(object).unwrap_or_default());
        self.content_cache = Some((doc.id, generation, object, content.clone()));
        content
    }
    /// The document's tags, walked again only after an edit.
    pub(crate) fn tags(&mut self, doc: &pdfcraft_engine::Document) -> Arc<Vec<TagInfo>> {
        let generation = doc.edit_generation();
        if let Some((id, g, tags)) = &self.cache
            && *id == doc.id
            && *g == generation
        {
            return tags.clone();
        }
        let tags = Arc::new(doc.tags());
        self.cache = Some((doc.id, generation, tags.clone()));
        tags
    }

    /// Select a checker finding's tag without discarding an unfinished edit.
    pub(crate) fn select(&mut self, doc: DocId, tags: &[TagInfo], object: ObjRef) -> bool {
        let Some(tag) = tags.iter().find(|tag| tag.object == object) else { return false };
        if self.doc == Some(doc)
            && self
                .selected
                .and_then(|selected| tags.iter().find(|tag| tag.object == selected))
                .is_some_and(|current| self.draft.edit(current).is_some())
        {
            return false;
        }
        if self.doc != Some(doc) {
            self.collapsed.clear();
        }
        self.doc = Some(doc);
        self.selected = Some(object);
        self.draft = Draft::from_tag(tag);
        self.query.clear();
        self.collapsed.clear();
        true
    }
}

/// The tags in display order, as the list shows them.
struct TagRows<'a> {
    rows: &'a [&'a TagInfo],
    parents: &'a HashSet<ObjRef>,
    collapsed: &'a BTreeSet<ObjRef>,
    /// A search lists matches with their ancestors and ignores collapsed groups, so no row is a group then.
    searching: bool,
    selected: Option<ObjRef>,
    /// An unsaved draft keeps every other row from being selected.
    dirty: bool,
}

impl TagRows<'_> {
    fn is_group(&self, tag: &TagInfo) -> bool {
        self.parents.contains(&tag.object) && !self.searching
    }
}

impl DisplayOrder<ObjRef> for TagRows<'_> {
    fn len(&self) -> usize {
        self.rows.len()
    }
    fn key(&self, at: usize) -> ObjRef {
        // The list never asks past `len()`; object 0 is a key no tag has.
        self.rows.get(at).map_or_else(|| ObjRef::new(0, 0), |tag| tag.object)
    }
    fn depth(&self, at: usize) -> usize {
        self.rows.get(at).map_or(0, |tag| tag.depth)
    }
    fn has_children(&self, at: usize) -> bool {
        self.rows.get(at).is_some_and(|tag| self.is_group(tag))
    }
    fn open(&self, at: usize) -> bool {
        self.rows.get(at).is_some_and(|tag| self.is_group(tag) && !self.collapsed.contains(&tag.object))
    }
    fn enabled(&self, at: usize) -> bool {
        self.rows.get(at).is_some_and(|tag| !self.dirty || self.selected == Some(tag.object))
    }
}

impl ItemSource<ObjRef> for TagRows<'_> {}

/// A list of `count` rows keyed by position.
struct Positions(usize);

impl DisplayOrder<usize> for Positions {
    fn len(&self) -> usize {
        self.0
    }
    fn key(&self, at: usize) -> usize {
        at
    }
}

impl ItemSource<usize> for Positions {}

/// The label of a marked-content row.
fn content_label(item: &TagContentItem) -> String {
    let page_label = item.page.map(|p| crate::i18n::fmt(tl!("Go to page {p}"), &[("p", &(p + 1).to_string())]));
    match (&page_label, item.text.trim()) {
        (Some(page), text) if !text.is_empty() => format!("MCID {} · {page} · {text}", item.mcid),
        (Some(page), _) => format!("MCID {} · {page}", item.mcid),
        (_, text) if !text.is_empty() => format!("MCID {} · {text}", item.mcid),
        _ => format!("MCID {}", item.mcid),
    }
}

/// The chosen page and an edit to apply after the panel releases its document borrow.
pub(crate) fn panel(
    ui: &mut Ui,
    doc: DocId,
    tags: &[TagInfo],
    content: &[TagContentItem],
    state: &mut State,
    can_edit: bool,
) -> (Option<usize>, Option<Edit>) {
    if state.doc != Some(doc) {
        state.doc = Some(doc);
        state.selected = None;
        state.draft = Draft::default();
        state.query.clear();
        state.collapsed.clear();
    }
    if tags.is_empty() {
        Notice::new(Tone::Neutral, tl!("This document has no editable structure tags.")).show(ui);
        return (None, None);
    }
    if state.selected.is_none_or(|r| !tags.iter().any(|t| t.object == r)) {
        state.selected = Some(tags[0].object);
        state.draft = Draft::from_tag(&tags[0]);
    }
    Field::new(tl!("Find a tag")).show(ui, |ui, _| {
        TextField::new(tl!("Find a tag"), Bind::bound(&mut state.query)).show(ui);
    });
    let query = state.query.to_lowercase();
    let matching = visible_tags(tags, &query, &state.collapsed);
    let parents: HashSet<ObjRef> = tags.iter().filter_map(|tag| tag.parent).collect();
    let dirty = state.selected.and_then(|r| tags.iter().find(|t| t.object == r)).is_some_and(|tag| state.draft.edit(tag).is_some());
    Text::new(crate::i18n::fmt(tl!("{n} tags"), &[("n", &matching.len().to_string())])).shade(Shade::Muted).show(ui);
    let mut page = None;
    let source =
        TagRows { rows: &matching, parents: &parents, collapsed: &state.collapsed, searching: !query.is_empty(), selected: state.selected, dirty };
    let mut picked = match state.selected {
        Some(object) => Selection::single(object),
        None => Selection::new(Vec::new(), None, None),
    };
    let out = List::<ObjRef>::new(tl!("Tags"))
        .tree(true)
        .natural_height(true)
        .select(SelectMode::Single)
        .selection(Bind::bound(&mut picked))
        .show_items(ui, &source, &mut |key, _| {
            let Some(tag) = matching.iter().find(|tag| tag.object == *key) else { return ListRow::new("") };
            let title = tag.title.as_deref().filter(|s| !s.is_empty()).unwrap_or(&tag.kind);
            let row = ListRow::new(format!("{title} · {}", tag.object.num)).depth(tag.depth);
            if source.is_group(tag) { row.group(!source.collapsed.contains(&tag.object)) } else { row }
        });
    for event in &out.events {
        match event {
            CollectionEvent::Open(change) => {
                if change.open {
                    state.collapsed.remove(&change.key);
                } else {
                    state.collapsed.insert(change.key);
                }
            }
            CollectionEvent::Selection(SelectionChange::Set(keys)) => {
                if let Some(tag) = keys.last().and_then(|key| tags.iter().find(|tag| tag.object == *key)) {
                    state.selected = Some(tag.object);
                    state.draft = Draft::from_tag(tag);
                    page = tag.page;
                }
            }
            _ => {}
        }
    }
    let Some(tag) = state.selected.and_then(|r| tags.iter().find(|t| t.object == r)) else { return (page, None) };
    Text::new(format!("{} {} R", tag.object.num, tag.object.generation)).shade(Shade::Muted).show(ui);
    if let Some(p) = tag.page
        && Button::new(crate::i18n::fmt(tl!("Go to page {p}"), &[("p", &(p + 1).to_string())])).show(ui).response().clicked()
    {
        page = Some(p);
    }
    let draft = &mut state.draft;
    FieldGroup::new(tl!("Tag properties")).show(ui, |ui, _| {
        Field::new(tl!("Tag type")).show(ui, |ui, _| {
            TextField::new(tl!("Tag type"), Bind::bound(&mut draft.kind)).enabled(can_edit).show(ui);
        });
        Field::new(tl!("Title")).show(ui, |ui, _| {
            TextField::new(tl!("Title"), Bind::bound(&mut draft.title)).enabled(can_edit).show(ui);
        });
        Field::new(tl!("Actual text")).show(ui, |ui, _| {
            TextField::new(tl!("Actual text"), Bind::bound(&mut draft.actual_text)).lines(2).enabled(can_edit).show(ui);
        });
        Field::new(tl!("Alternate text")).show(ui, |ui, _| {
            TextField::new(tl!("Alternate text"), Bind::bound(&mut draft.alt)).lines(2).enabled(can_edit).show(ui);
        });
        Field::new(tl!("Language")).show(ui, |ui, _| {
            TextField::new(tl!("Language"), Bind::bound(&mut draft.language)).enabled(can_edit).show(ui);
        });
    });
    let edit = state.draft.edit(tag);
    let siblings: Vec<&TagInfo> = tag.parent.map(|parent| tags.iter().filter(|other| other.parent == Some(parent)).collect()).unwrap_or_default();
    let position = siblings.iter().position(|sibling| sibling.object == tag.object);
    let before = position.and_then(|position| position.checked_sub(1)).and_then(|position| siblings.get(position).copied());
    let after = position.and_then(|position| siblings.get(position + 1)).copied();
    let save =
        Button::new(tl!("Save tag properties")).enabled(can_edit && !state.draft.kind.is_empty() && edit.is_some()).show(ui).response().clicked();
    if Button::new(tl!("Revert changes")).enabled(dirty).show(ui).response().clicked() {
        state.draft = Draft::from_tag(tag);
    }
    let earlier = Button::new(tl!("Move tag earlier")).enabled(can_edit && !dirty && before.is_some()).show(ui).response().clicked();
    let later = Button::new(tl!("Move tag later")).enabled(can_edit && !dirty && after.is_some()).show(ui).response().clicked();
    let artifact = Button::new(tl!("Mark as artifact")).enabled(can_edit && !dirty).show(ui).response().clicked();
    let mut chosen = Selection::new(Vec::new(), None, None);
    let marked = List::<usize>::new(tl!("Marked content"))
        .natural_height(true)
        .select(SelectMode::Single)
        .selection(Bind::bound(&mut chosen))
        .show_items(ui, &Positions(content.len()), &mut |key, _| ListRow::new(content.get(*key).map(content_label).unwrap_or_default()));
    for event in &marked.events {
        if let CollectionEvent::Selection(SelectionChange::Set(keys)) = event
            && let Some(item) = keys.last().and_then(|key| content.get(*key))
        {
            page = item.page.or(page);
        }
    }
    (
        page,
        if save {
            edit
        } else if earlier {
            before.map(|anchor| Edit::MoveTag { object: tag.object, anchor: anchor.object, placement: TagPlacement::Before })
        } else if later {
            after.map(|anchor| Edit::MoveTag { object: tag.object, anchor: anchor.object, placement: TagPlacement::After })
        } else if artifact {
            Some(Edit::MarkTagArtifact { object: tag.object })
        } else {
            None
        },
    )
}

fn visible_tags<'a>(tags: &'a [TagInfo], query: &str, collapsed: &BTreeSet<ObjRef>) -> Vec<&'a TagInfo> {
    if query.is_empty() {
        let mut hidden_after: Option<usize> = None;
        return tags
            .iter()
            .filter(|tag| {
                if hidden_after.is_some_and(|level| tag.depth > level) {
                    return false;
                }
                hidden_after = collapsed.contains(&tag.object).then_some(tag.depth);
                true
            })
            .collect();
    }
    let by_object: HashMap<ObjRef, &TagInfo> = tags.iter().map(|tag| (tag.object, tag)).collect();
    let mut show = HashSet::new();
    for tag in
        tags.iter().filter(|tag| tag.kind.to_lowercase().contains(query) || tag.title.as_ref().is_some_and(|v| v.to_lowercase().contains(query)))
    {
        let mut next = Some(tag.object);
        for _ in 0..=256 {
            let Some(object) = next.filter(|o| show.insert(*o)) else { break };
            next = by_object.get(&object).and_then(|t| t.parent);
        }
    }
    tags.iter().filter(|tag| show.contains(&tag.object)).collect()
}
