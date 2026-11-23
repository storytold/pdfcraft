//! A visual editor for properties of reachable indirect structure tags.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use egui::Ui;
use pdfcraft_cos::ObjRef;
use pdfcraft_engine::a11y::{TagBox, TagContentItem, TagInfo, TagPlacement, TagProperty};
use pdfcraft_engine::{DocId, Edit};

use crate::canvas::PageXform;
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
    /// Highlight content is off: the pages don't outline the selected tag.
    hide_highlight: bool,
    /// The selected tag's outlines, one per page, per (document, edit generation, tag).
    boxes_cache: Option<(DocId, u64, ObjRef, Arc<Vec<TagBox>>)>,
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
    /// What the pages outline for the selected tag: one box per page its content and its
    /// descendants' content is on, or none while Highlight content is off.
    pub(crate) fn highlighted(&mut self, doc: &pdfcraft_engine::Document) -> Arc<Vec<TagBox>> {
        let Some(object) = self.selected.filter(|_| !self.hide_highlight && self.doc == Some(doc.id)) else { return Arc::default() };
        let generation = doc.edit_generation();
        if let Some((id, g, o, boxes)) = &self.boxes_cache
            && *id == doc.id
            && *g == generation
            && *o == object
        {
            return boxes.clone();
        }
        let boxes = Arc::new(pdfcraft_engine::a11y::tag_page_bounds(&doc.tag_boxes(object).unwrap_or_default()));
        self.boxes_cache = Some((doc.id, generation, object, boxes.clone()));
        boxes
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
    let mut highlight = !state.hide_highlight;
    if ui.add(widgets::checkbox(&mut highlight, tl!("Highlight content"))).changed() {
        state.hide_highlight = !highlight;
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

/// Outlines the selected tag's content on page `page`.
pub(crate) fn paint_page(painter: &egui::Painter, t: &Tokens, xf: &PageXform, page: usize, info: &pdfcraft_render::DocInfo, boxes: &[TagBox]) {
    for b in boxes.iter().filter(|b| b.page == page) {
        let r = xf.user_rect_f64(info, page, b.rect).expand(1.5);
        // Zoom can carry a huge box past `f32`.
        if !r.is_finite() {
            continue;
        }
        painter.rect_filled(r, egui::CornerRadius::same(2), t.accent.gamma_multiply(0.16));
        painter.rect_stroke(r, egui::CornerRadius::same(2), egui::Stroke::new(1.5, t.accent), egui::StrokeKind::Outside);
    }
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

#[cfg(test)]
mod tests {
    use crate::PdfCraftApp;
    use egui_kittest::kittest::Queryable;
    use pdfcraft_engine::a11y::TagBox;

    /// A figure and a filled bar, each its own tag.
    const FIGURES: &[u8] = b"%PDF-1.7
1 0 obj << /Type /Catalog /Pages 2 0 R /MarkInfo << /Marked true >> /StructTreeRoot 5 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Resources << /XObject << /Im 10 0 R >> >> >> endobj
4 0 obj << /Length 109 >> stream
/Figure << /MCID 0 >> BDC q 40 0 0 20 10 150 cm /Im Do Q EMC /Figure << /MCID 1 >> BDC 100 100 50 30 re f EMC
endstream endobj
5 0 obj << /Type /StructTreeRoot /K 6 0 R >> endobj
6 0 obj << /S /Document /P 5 0 R /K [7 0 R 8 0 R] >> endobj
7 0 obj << /S /Figure /P 6 0 R /Pg 3 0 R /K 0 >> endobj
8 0 obj << /S /Figure /P 6 0 R /Pg 3 0 R /K 1 >> endobj
10 0 obj << /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Length 1 >> stream
\x00
endstream endobj
trailer << /Root 1 0 R >>
%%EOF";

    fn highlighted(h: &mut egui_kittest::Harness<'static, PdfCraftApp>) -> Vec<TagBox> {
        let app = h.state_mut();
        let Some(doc) = app.session.get(app.views[0].id) else { return Vec::new() };
        app.tags_state.highlighted(doc).to_vec()
    }

    /// The selected tag is outlined on its page (#7070), the document's tag for the whole tree, and
    /// Highlight content turns it off.
    #[test]
    fn the_selected_tag_is_outlined_until_highlight_content_is_off() {
        let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
            let mut app = PdfCraftApp::new();
            app.set_option("language", "en").unwrap();
            app.open_bytes("figures.pdf", None, FIGURES.to_vec()).unwrap();
            app
        });
        h.run_steps(4);
        assert!(h.state_mut().execute("a11y.tags"));
        h.run_steps(2);
        assert_eq!(highlighted(&mut h), [TagBox { page: 0, rect: [10.0, 100.0, 150.0, 170.0] }], "the Document tag covers both figures");
        h.get_by_label("Figure · 8").click();
        h.run_steps(2);
        assert_eq!(highlighted(&mut h), [TagBox { page: 0, rect: [100.0, 100.0, 150.0, 130.0] }]);
        h.get_by_label("Highlight content").click();
        h.run_steps(2);
        assert!(highlighted(&mut h).is_empty());
    }
}
