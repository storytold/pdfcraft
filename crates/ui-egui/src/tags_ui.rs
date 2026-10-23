//! The Tags panel: the structure tree with each tag's type, title, actual text, alternate text
//! and language; new, delete, move (drag a row, or the move buttons), change type and change to
//! artifact; the selected tag's content highlighted on the page; and Find untagged content.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use egui::{Stroke, Ui};
use pdfcraft_cos::ObjRef;
use pdfcraft_engine::a11y::{Destination, TagContentItem, TagInfo, TagPlacement, TagProperty};
use pdfcraft_engine::{DocId, Edit};

use crate::{icons, theme::Tokens, widgets};

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
    /// Highlight content is off.
    hide_highlight: bool,
    /// The type a new tag gets.
    new_kind: String,
    /// Where the selected tag is drawn, per (document, edit generation, tag, page).
    boxes_cache: Vec<BoxesEntry>,
    /// The tags before New Tag ran: the one that appears next is selected.
    before_new: Option<(DocId, HashSet<ObjRef>)>,
    /// Scroll the list to the selected tag (it was chosen outside the list).
    reveal: bool,
}

/// Where a tag is drawn on a page: (document, edit generation, tag, page, boxes in user space).
type BoxesEntry = (DocId, u64, ObjRef, usize, Arc<Vec<[f64; 4]>>);

/// A tag row being dragged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TagDrag(ObjRef);

/// Types offered for a new tag (ISO 32000-2 §14.8.4).
const NEW_KINDS: &[&str] = &[
    "P",
    "H1",
    "H2",
    "H3",
    "H4",
    "H5",
    "H6",
    "Sect",
    "Div",
    "Part",
    "Art",
    "Aside",
    "BlockQuote",
    "Caption",
    "Figure",
    "Formula",
    "Table",
    "THead",
    "TBody",
    "TFoot",
    "TR",
    "TH",
    "TD",
    "L",
    "LI",
    "Lbl",
    "LBody",
    "Link",
    "Annot",
    "Form",
    "Span",
    "Quote",
    "Note",
    "Reference",
    "Code",
    "TOC",
    "TOCI",
    "Index",
    "Title",
    "Document",
    "NonStruct",
];

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
    /// The tag whose content the page highlights: the selection, when Highlight content is on.
    pub(crate) fn highlighted(&self, doc: DocId) -> Option<ObjRef> {
        (!self.hide_highlight && self.doc == Some(doc)).then_some(self.selected).flatten()
    }

    /// The selected tag.
    pub(crate) fn selected_tag(&self) -> Option<ObjRef> {
        self.selected
    }

    /// Whether Highlight content is on: the page then shows the selected tag's content, and a
    /// click on content selects its tag.
    pub(crate) fn highlighting(&self) -> bool {
        !self.hide_highlight
    }

    /// Where a tag's content is drawn on a page (user space), read again only after an edit.
    pub(crate) fn boxes(&mut self, doc: &pdfcraft_engine::Document, tag: ObjRef, page: usize) -> Arc<Vec<[f64; 4]>> {
        let generation = doc.edit_generation();
        if let Some((.., b)) = self.boxes_cache.iter().find(|(d, g, t, p, _)| *d == doc.id && *g == generation && *t == tag && *p == page) {
            return b.clone();
        }
        let boxes = Arc::new(doc.tag_boxes(tag, page));
        self.boxes_cache.retain(|(d, g, t, ..)| *d == doc.id && *g == generation && *t == tag);
        if self.boxes_cache.len() >= 16 {
            self.boxes_cache.remove(0);
        }
        self.boxes_cache.push((doc.id, generation, tag, page, boxes.clone()));
        boxes
    }

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
        self.reveal = true;
        true
    }
}

/// The chosen page, an edit to apply after the panel releases its document borrow, and a
/// command to run (Find untagged content opens the Reading Order tool).
pub(crate) fn panel(
    ui: &mut Ui,
    t: &Tokens,
    doc: DocId,
    tags: &[TagInfo],
    content: &[TagContentItem],
    state: &mut State,
    can_edit: bool,
) -> (Option<usize>, Option<Edit>, Option<&'static str>) {
    if state.doc != Some(doc) {
        state.doc = Some(doc);
        state.selected = None;
        state.draft = Draft::default();
        state.query.clear();
        state.collapsed.clear();
    }
    let mut command = None;
    if tags.is_empty() {
        ui.label(tl!("This document has no editable structure tags."));
        if ui.button(tl!("Find untagged content")).clicked() {
            command = Some("a11y.reading_order");
        }
        return (None, None, command);
    }
    // New Tag ran: select the tag it made.
    if let Some((d, before)) = state.before_new.take()
        && d == doc
        && let Some(made) = tags.iter().find(|t| !before.contains(&t.object))
    {
        state.selected = Some(made.object);
        state.draft = Draft::from_tag(made);
        state.reveal = true;
    }
    if state.selected.is_none_or(|r| !tags.iter().any(|t| t.object == r)) {
        state.selected = Some(tags[0].object);
        state.draft = Draft::from_tag(&tags[0]);
    }
    let search_label = ui.label(tl!("Find a tag"));
    ui.add(egui::TextEdit::singleline(&mut state.query).hint_text(tl!("Find a tag"))).labelled_by(search_label.id);
    let mut highlight = !state.hide_highlight;
    if ui.add(widgets::checkbox(&mut highlight, tl!("Highlight content"))).changed() {
        state.hide_highlight = !highlight;
    }
    ui.add_space(4.0);
    let query = state.query.to_lowercase();
    let matching = visible_tags(tags, &query, &state.collapsed);
    let parents: HashSet<ObjRef> = tags.iter().filter_map(|tag| tag.parent).collect();
    let dirty = state.selected.and_then(|r| tags.iter().find(|t| t.object == r)).is_some_and(|tag| state.draft.edit(tag).is_some());
    ui.label(egui::RichText::new(crate::i18n::fmt(tl!("{n} tags"), &[("n", &matching.len().to_string())])).small().color(t.text_muted));
    let mut page = None;
    let mut dropped: Option<Edit> = None;
    let mut list = egui::ScrollArea::vertical().max_height(130.0);
    if std::mem::take(&mut state.reveal)
        && let Some(at) = state.selected.and_then(|r| matching.iter().position(|t| t.object == r))
    {
        // Rows are 26 points plus spacing; show the selection with a row above it.
        let row = 26.0 + ui.spacing().item_spacing.y;
        list = list.vertical_scroll_offset(((at as f32) - 1.0).max(0.0) * row);
    }
    list.show_rows(ui, 26.0, matching.len(), |ui, range| {
        for tag in &matching[range] {
            let title = tag.title.as_deref().filter(|s| !s.is_empty()).unwrap_or(&tag.kind);
            let label = format!("{title} · {}", tag.object.num);
            ui.horizontal(|ui| {
                ui.add_space((tag.depth.min(8) as f32) * 12.0);
                if parents.contains(&tag.object) && query.is_empty() {
                    let closed = state.collapsed.contains(&tag.object);
                    if icons::button(
                        ui,
                        if closed { "chevron-right" } else { "chevron-down" },
                        18.0,
                        false,
                        if closed { tl!("Expand tag") } else { tl!("Collapse tag") },
                    )
                    .clicked()
                    {
                        if closed {
                            state.collapsed.remove(&tag.object);
                        } else {
                            state.collapsed.insert(tag.object);
                        }
                    }
                } else {
                    ui.add_space(18.0);
                }
                let row = ui.add_enabled(
                    !dirty || state.selected == Some(tag.object),
                    egui::Button::selectable(state.selected == Some(tag.object), label).sense(egui::Sense::click_and_drag()),
                );
                if row.clicked() {
                    state.selected = Some(tag.object);
                    state.draft = Draft::from_tag(tag);
                    page = tag.page;
                }
                // Drag a row onto another: its top third puts the tag before it, the bottom
                // third after it, the middle inside it.
                if can_edit && !dirty && row.drag_started() {
                    egui::DragAndDrop::set_payload(ui.ctx(), TagDrag(tag.object));
                }
                let zone = |r: &egui::Response| {
                    let y = r.ctx.pointer_latest_pos().map_or(r.rect.center().y, |p| p.y);
                    let third = r.rect.height() / 3.0;
                    if y < r.rect.top() + third {
                        Destination::Before(tag.object)
                    } else if y > r.rect.bottom() - third {
                        Destination::After(tag.object)
                    } else {
                        Destination::Into(Some(tag.object))
                    }
                };
                if let Some(dragged) = row.dnd_hover_payload::<TagDrag>()
                    && dragged.0 != tag.object
                {
                    let stroke = Stroke::new(2.0, t.accent);
                    match zone(&row) {
                        Destination::Before(_) => {
                            ui.painter().hline(row.rect.x_range(), row.rect.top(), stroke);
                        }
                        Destination::After(_) => {
                            ui.painter().hline(row.rect.x_range(), row.rect.bottom(), stroke);
                        }
                        _ => {
                            ui.painter().rect_stroke(row.rect, egui::CornerRadius::same(3), stroke, egui::StrokeKind::Inside);
                        }
                    }
                }
                if let Some(dragged) = row.dnd_release_payload::<TagDrag>()
                    && dragged.0 != tag.object
                {
                    dropped = Some(Edit::MoveTagTo { object: dragged.0, destination: zone(&row) });
                }
                if can_edit && !dirty {
                    row.on_hover_text(tl!("Drag to reorder"));
                }
            });
        }
    });
    ui.separator();
    if dropped.is_some() {
        return (page, dropped, None);
    }
    let Some(tag) = state.selected.and_then(|r| tags.iter().find(|t| t.object == r)) else { return (page, None, None) };
    let siblings: Vec<&TagInfo> = tag.parent.map(|parent| tags.iter().filter(|other| other.parent == Some(parent)).collect()).unwrap_or_default();
    let position = siblings.iter().position(|sibling| sibling.object == tag.object);
    let before = position.and_then(|position| position.checked_sub(1)).and_then(|position| siblings.get(position)).copied();
    let after = position.and_then(|position| siblings.get(position + 1)).copied();
    ui.label(egui::RichText::new(format!("{} {} R", tag.object.num, tag.object.generation)).small().color(t.text_faint));
    if let Some(p) = tag.page
        && ui.button(crate::i18n::fmt(tl!("Go to page {p}"), &[("p", &(p + 1).to_string())])).clicked()
    {
        page = Some(p);
    }
    ui.add_enabled_ui(can_edit, |ui| {
        field(ui, tl!("Tag type"), &mut state.draft.kind, false);
        field(ui, tl!("Title"), &mut state.draft.title, false);
        field(ui, tl!("Actual text"), &mut state.draft.actual_text, true);
        field(ui, tl!("Alternate text"), &mut state.draft.alt, true);
        field(ui, tl!("Language"), &mut state.draft.language, false);
    });
    let edit = state.draft.edit(tag);
    let mut save = false;
    ui.horizontal(|ui| {
        save = ui.add_enabled(can_edit && !state.draft.kind.is_empty() && edit.is_some(), egui::Button::new(tl!("Save tag properties"))).clicked();
        if ui.add_enabled(dirty, egui::Button::new(tl!("Revert changes"))).clicked() {
            state.draft = Draft::from_tag(tag);
        }
    });
    let (mut earlier, mut later, mut artifact) = (false, false, false);
    ui.horizontal_wrapped(|ui| {
        earlier = ui.add_enabled(can_edit && !dirty && before.is_some(), egui::Button::new(tl!("Move tag earlier"))).clicked();
        later = ui.add_enabled(can_edit && !dirty && after.is_some(), egui::Button::new(tl!("Move tag later"))).clicked();
        artifact = ui
            .add_enabled(can_edit && !dirty, egui::Button::new(tl!("Mark as artifact")))
            .on_hover_text(tl!("Mark everything in this tag as an artifact and remove the tag from reading order"))
            .clicked();
    });
    let (mut into, mut out) = (false, false);
    ui.horizontal_wrapped(|ui| {
        into = ui.add_enabled(can_edit && !dirty && before.is_some(), egui::Button::new(tl!("Move into previous tag"))).clicked();
        out = ui.add_enabled(can_edit && !dirty && tag.parent.is_some(), egui::Button::new(tl!("Move out of parent tag"))).clicked();
    });
    let (mut create, mut delete, mut unwrap) = (false, false, false);
    if state.new_kind.is_empty() {
        state.new_kind = "P".into();
    }
    ui.horizontal_wrapped(|ui| {
        egui::ComboBox::from_id_salt("tags-new-kind").selected_text(state.new_kind.as_str()).width(90.0).show_ui(ui, |ui| {
            for kind in NEW_KINDS {
                ui.selectable_value(&mut state.new_kind, (*kind).to_owned(), *kind);
            }
        });
        create = ui.add_enabled(can_edit && !dirty, egui::Button::new(tl!("New tag"))).clicked();
        delete = ui.add_enabled(can_edit && !dirty, egui::Button::new(tl!("Delete tag"))).clicked();
        unwrap = ui.add_enabled(can_edit && !dirty, egui::Button::new(tl!("Remove tag, keep its contents"))).clicked();
    });
    if ui.button(tl!("Find untagged content")).clicked() {
        command = Some("a11y.reading_order");
    }
    ui.separator();
    for item in content {
        let page_label = item.page.map(|p| crate::i18n::fmt(tl!("Go to page {p}"), &[("p", &(p + 1).to_string())]));
        let label = match (&page_label, item.text.trim()) {
            (Some(page), text) if !text.is_empty() => format!("MCID {} · {page} · {text}", item.mcid),
            (Some(page), _) => format!("MCID {} · {page}", item.mcid),
            (_, text) if !text.is_empty() => format!("MCID {} · {text}", item.mcid),
            _ => format!("MCID {}", item.mcid),
        };
        if ui.button(label).clicked() {
            page = item.page;
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
            Some(Edit::TagToArtifact { object: tag.object })
        } else if into {
            before.map(|previous| Edit::MoveTagTo { object: tag.object, destination: Destination::Into(Some(previous.object)) })
        } else if out {
            tag.parent.map(|parent| Edit::MoveTagTo { object: tag.object, destination: Destination::After(parent) })
        } else if create {
            state.before_new = Some((doc, tags.iter().map(|t| t.object).collect()));
            Some(Edit::NewTag { kind: state.new_kind.clone(), destination: Destination::Into(Some(tag.object)) })
        } else if delete || unwrap {
            Some(Edit::DeleteTag { object: tag.object, keep_contents: unwrap })
        } else {
            None
        },
        command,
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

fn field(ui: &mut Ui, label: &str, value: &mut String, multiline: bool) {
    let l = ui.label(label);
    if multiline {
        ui.add(widgets::block(value).desired_rows(2).desired_width(f32::INFINITY)).labelled_by(l.id);
    } else {
        ui.add(egui::TextEdit::singleline(value).desired_width(f32::INFINITY)).labelled_by(l.id);
    }
}
