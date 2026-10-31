//! A visual editor for properties of reachable indirect structure tags.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use egui::Ui;
use pdfcraft_cos::ObjRef;
use pdfcraft_engine::a11y::{TagContentItem, TagInfo, TagPlacement, TagProperty};
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

/// The chosen page and an edit to apply after the panel releases its document borrow.
pub(crate) fn panel(
    ui: &mut Ui,
    t: &Tokens,
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
        ui.label(tl!("This document has no editable structure tags."));
        return (None, None);
    }
    if state.selected.is_none_or(|r| !tags.iter().any(|t| t.object == r)) {
        state.selected = Some(tags[0].object);
        state.draft = Draft::from_tag(&tags[0]);
    }
    let search_label = ui.label(tl!("Find a tag"));
    ui.add(egui::TextEdit::singleline(&mut state.query).hint_text(tl!("Find a tag"))).labelled_by(search_label.id);
    ui.add_space(4.0);
    let query = state.query.to_lowercase();
    let matching = visible_tags(tags, &query, &state.collapsed);
    let parents: HashSet<ObjRef> = tags.iter().filter_map(|tag| tag.parent).collect();
    let dirty = state.selected.and_then(|r| tags.iter().find(|t| t.object == r)).is_some_and(|tag| state.draft.edit(tag).is_some());
    ui.label(egui::RichText::new(crate::i18n::fmt(tl!("{n} tags"), &[("n", &matching.len().to_string())])).small().color(t.text_muted));
    let mut page = None;
    egui::ScrollArea::vertical().max_height(100.0).show_rows(ui, 26.0, matching.len(), |ui, range| {
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
                if ui
                    .add_enabled(!dirty || state.selected == Some(tag.object), egui::Button::selectable(state.selected == Some(tag.object), label))
                    .clicked()
                {
                    state.selected = Some(tag.object);
                    state.draft = Draft::from_tag(tag);
                    page = tag.page;
                }
            });
        }
    });
    ui.separator();
    let Some(tag) = state.selected.and_then(|r| tags.iter().find(|t| t.object == r)) else { return (page, None) };
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
    let siblings: Vec<&TagInfo> = tag.parent.map(|parent| tags.iter().filter(|other| other.parent == Some(parent)).collect()).unwrap_or_default();
    let position = siblings.iter().position(|sibling| sibling.object == tag.object);
    let before = position.and_then(|position| position.checked_sub(1)).map(|position| siblings[position]);
    let after = position.and_then(|position| siblings.get(position + 1)).copied();
    ui.horizontal(|ui| {
        save = ui.add_enabled(can_edit && !state.draft.kind.is_empty() && edit.is_some(), egui::Button::new(tl!("Save tag properties"))).clicked();
        if ui.add_enabled(dirty, egui::Button::new(tl!("Revert changes"))).clicked() {
            state.draft = Draft::from_tag(tag);
        }
    });
    let (mut earlier, mut later, mut artifact) = (false, false, false);
    ui.horizontal(|ui| {
        earlier = ui.add_enabled(can_edit && !dirty && before.is_some(), egui::Button::new(tl!("Move tag earlier"))).clicked();
        later = ui.add_enabled(can_edit && !dirty && after.is_some(), egui::Button::new(tl!("Move tag later"))).clicked();
        artifact = ui
            .add_enabled(can_edit && !dirty, egui::Button::new(tl!("Mark as artifact")))
            .on_hover_text(tl!("Remove this leaf tag from reading order and mark its direct content as an artifact"))
            .clicked();
    });
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

fn field(ui: &mut Ui, label: &str, value: &mut String, multiline: bool) {
    let l = ui.label(label);
    if multiline {
        ui.add(widgets::block(value).desired_rows(2).desired_width(f32::INFINITY)).labelled_by(l.id);
    } else {
        ui.add(egui::TextEdit::singleline(value).desired_width(f32::INFINITY)).labelled_by(l.id);
    }
}
