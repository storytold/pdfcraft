//! Compare files: choose the older version, then the Compare panel lists the differences
//! (replaced blue, inserted green, deleted red), shades them on the newer document's pages, and
//! offers a report or the differences as comments.

use craft::behavior::{IconId, Selection, keyed};
use craft::pages::ui::{Lead, PageRangeField, SyncedViews, SyncedViewsState};
use craft::pages::{format_pages, parse_pages};
use craft::theme::variant::Tone;
use craft::ui::buttons::Button;
use craft::ui::choice::{Checkbox, Segmented};
use craft::ui::contract::{Bind, Entry, Pad, Props as _, SelectMode, Show as _, ShowCollection as _, ShowContainer as _};
use craft::ui::dialog::{DialogResult, dialog_id};
use craft::ui::display::Text;
use craft::ui::layout_components::{Row, Stack};
use craft::ui::list::{HueTarget, List, ListRow};
use craft::ui::notices::Notice;
use craft::ui::patterns::form_dialogs::{DialogFields, HostedParamDialog, Param, ParamDialog, ParamKind};
use craft::ui::primitives::IconSpec;
use craft::ui::style::DataColor;
use egui::Color32;
use pdfcraft_engine::DocId;
use pdfcraft_engine::compare::{Comparison, Kind};

use crate::{PdfCraftApp, RightPanel};

/// The key the Compare Files dialog files its answer under.
const DIALOG_KEY: &str = "compare";

/// The last comparison.
pub struct CompareState {
    pub old: DocId,
    pub new: DocId,
    pub result: Comparison,
    /// The pages of the new file this comparison covers (0-based; empty: every page).
    pub pages: Vec<usize>,
    /// Regions that look different (page, user-space box), page n against page n.
    pub visual: Vec<(usize, [f64; 4])>,
    pub selected: Option<usize>,
    /// Side by side: the old file on the left, the new on the right, scrolled and zoomed
    /// together.
    pub side_by_side: bool,
    /// The pair's divider and which half leads; the new file leads at first.
    pub views: SyncedViewsState,
    /// Show: whether the text changes are in the list and the shading, and whether the visual
    /// differences (counted, never listed) are shaded (Acrobat's Filter).
    pub show_text: bool,
    pub show_visual: bool,
}

/// Visual differences are shaded orange.
pub const VISUAL: Color32 = Color32::from_rgb(0xF0, 0x8C, 0x1A);

/// What the panel asks for.
pub enum PanelAction {
    Select(usize),
    Mark,
    Report,
    Clear,
    SideBySide(bool),
    Show { text: bool, visual: bool },
}

pub fn colour(kind: Kind) -> Color32 {
    let [r, g, b] = pdfcraft_engine::compare::colour(kind);
    Color32::from_rgb((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
}

/// The key a document is offered under in the dialog's choice.
fn option_key(id: DocId) -> String {
    id.0.to_string()
}

/// Opens the Compare Files dialog: the older version among the other open documents, the active file
/// as the new one, and the pages to compare.
pub(crate) fn open_compare_dialog(app: &mut PdfCraftApp, ctx: &egui::Context) {
    let Some((active_i, new)) = app.active_ids() else { return };
    let others: Vec<(String, String)> =
        app.views.iter().filter(|v| v.id != new).filter_map(|v| app.session.get(v.id).map(|d| (option_key(d.id), d.name.clone()))).collect();
    let old = app.compare_old.map(option_key).filter(|key| others.iter().any(|(k, _)| k == key)).or_else(|| others.first().map(|(k, _)| k.clone()));
    let new_name = app.session.get(new).map(|d| d.name.clone()).unwrap_or_default();
    let count = app.views.get(active_i).map_or(0, |v| v.page_count());
    let fields = DialogFields::from_pairs([("old", old.unwrap_or_default()), ("pages", app.compare_pages.clone())]);
    // Passed inline: a closure made apart from the call gets one lifetime for its argument and result.
    let dialog = HostedParamDialog::new(ctx, DIALOG_KEY, fields, move |fields: &mut DialogFields| {
        let chosen = fields.text("old").filter(|o| !o.is_empty()).map(str::to_owned);
        let pages_ok = parse_pages(fields.text("pages").unwrap_or_default(), count).is_ok();
        let options = others.iter().map(|(key, name)| (key.clone(), name.clone().into())).collect();
        let pages = Param::custom("pages", tl!("Pages").to_string(), move |ui, fields, _| {
            let mut shown = parse_pages(fields.text("pages").unwrap_or_default(), count).unwrap_or_default();
            if shown.is_empty() {
                // An empty entry compares every page, so the field shows them all (1-3).
                shown = (0..count).collect();
            }
            let out = PageRangeField::new(tl!("Pages").to_string(), &shown, count, 0).show(ui);
            if let Some(picked) = out.picked {
                fields.set("pages", format_pages(&picked));
            }
            out.response
        })
        .opens_with(String::new());
        let params = vec![
            Param::new("old", tl!("Old file:").to_string(), ParamKind::choice(options, chosen.clone())),
            Param::read_only_text("new", tl!("New file:").to_string(), new_name.clone()),
            pages,
        ];
        let reason = if chosen.is_none() { tl!("Open the older version first") } else { tl!("Enter valid page numbers") };
        ParamDialog::new(DIALOG_KEY, tl!("Compare Files").to_string(), params)
            .ok_label(tl!("Compare").into())
            .ok_enabled(chosen.is_some() && pages_ok)
            .ok_reason(reason.into())
            .footer_note(tl!("Text is compared word by word; the differences are shown on the new file.").to_string())
    });
    crate::dialogs::host::open(app, dialog);
}

/// Once per frame: when the Compare Files dialog answered OK, runs the comparison it asked for.
pub(crate) fn pass(app: &mut PdfCraftApp, ctx: &egui::Context) {
    let Some(DialogResult::Ok(result)) = app.dialogs.take_by_id::<HostedParamDialog>(dialog_id(DIALOG_KEY)) else { return };
    let chosen = result.fields.text("old").unwrap_or_default();
    let Some(old) = app.views.iter().map(|v| v.id).find(|id| option_key(*id) == chosen) else { return };
    app.compare_old = Some(old);
    app.compare_pages = result.fields.text("pages").unwrap_or_default().to_owned();
    app.run_compare(ctx);
}

/// The text a change shows in the list.
fn change_text(ch: &pdfcraft_engine::compare::Change) -> String {
    match ch.kind {
        Kind::Replaced => format!("\"{}\" → \"{}\"", ch.old.text, ch.new.text),
        Kind::Inserted => format!("+ \"{}\"", ch.new.text),
        Kind::Deleted => format!("− \"{}\"", ch.old.text),
    }
}

/// The kind's name, written out so the catalogue extraction sees each word.
fn kind_label(kind: Kind) -> String {
    match kind {
        Kind::Replaced => tl!("Replaced").to_string(),
        Kind::Inserted => tl!("Inserted").to_string(),
        Kind::Deleted => tl!("Deleted").to_string(),
    }
}

/// A count of one kind of difference after its coloured badge.
fn count_row(ui: &mut egui::Ui, colour: Color32, n: usize, kind: &str) {
    let text = crate::i18n::fmt(tl!("{n} {kind}"), &[("n", &n.to_string()), ("kind", kind)]);
    ListRow::new(text).icon(IconSpec::svg(IconId::new("square"))).hue(DataColor::new(colour), HueTarget::Icon).show(ui);
}

/// The Compare panel, a dock body.
pub(crate) fn panel(ui: &mut egui::Ui, state: &Option<CompareState>, id: DocId) -> Option<PanelAction> {
    let Some(s) = state.as_ref().filter(|s| s.new == id) else {
        Notice::new(Tone::Info, tl!("Compare this file with an older version: Compare files… in All tools.").to_string()).show(ui);
        return None;
    };
    let mut action = None;
    let c = &s.result;
    Stack::new().pad(Pad::Control).show(ui, |ui, _| {
        if c.identical() {
            Notice::new(Tone::Info, tl!("No differences in the text.").to_string()).show(ui);
        }
        for k in [Kind::Replaced, Kind::Inserted, Kind::Deleted] {
            count_row(ui, colour(k), c.count(k), &kind_label(k));
        }
        count_row(ui, VISUAL, s.visual.len(), tl!("Visual"));
        Row::new().show(ui, |ui, _| {
            Text::new(tl!("Show:").to_string()).show(ui);
            let (mut text, mut visual) = (s.show_text, s.show_visual);
            Checkbox::new(Bind::bound(&mut text)).label(tl!("Text").to_string()).show(ui);
            Checkbox::new(Bind::bound(&mut visual)).label(tl!("Visual").to_string()).show(ui);
            if (text, visual) != (s.show_text, s.show_visual) {
                action = Some(PanelAction::Show { text, visual });
            }
        });
        let entries = [Entry::new(true, tl!("Side by side").to_string()), Entry::new(false, tl!("New file only").to_string())];
        let mut side = s.side_by_side;
        Segmented::new(tl!("Layout").to_string(), Bind::bound(&mut side), &entries).show(ui);
        if side != s.side_by_side {
            action = Some(PanelAction::SideBySide(side));
        }
        Row::new().show(ui, |ui, _| {
            if Button::new(tl!("Mark as comments").to_string()).show(ui).clicked() {
                action = Some(PanelAction::Mark);
            }
            if Button::new(tl!("Report…").to_string()).show(ui).clicked() {
                action = Some(PanelAction::Report);
            }
            if Button::new(tl!("Clear").to_string()).show(ui).clicked() {
                action = Some(PanelAction::Clear);
            }
        });
        let order: Vec<usize> = (0..c.changes.len()).filter(|_| s.show_text).take(2000).collect();
        let mut selection = Selection::new(s.selected.into_iter().collect(), s.selected, s.selected);
        let rows = keyed(&order, |i| *i);
        List::new(tl!("Differences").to_string()).select(SelectMode::Single).selection(Bind::bound(&mut selection)).show_items(
            ui,
            &rows,
            &mut |key, item| {
                let Some(ch) = c.changes.get(*key) else { return ListRow::new(String::new()) };
                let detail =
                    crate::i18n::fmt(tl!("{kind} · page {p}"), &[("kind", kind_label(ch.kind).as_str()), ("p", &(ch.new.page + 1).to_string())]);
                ListRow::new(change_text(ch))
                    .detail(detail)
                    .hue(DataColor::new(colour(ch.kind)), HueTarget::Icon)
                    .icon(IconSpec::svg(IconId::new("square")))
                    .state(item.row.clone())
            },
        );
        if let Some(picked) = selection.items.first().copied().filter(|p| Some(*p) != s.selected) {
            action = Some(PanelAction::Select(picked));
        }
    });
    action
}

impl PdfCraftApp {
    /// Set gaps for the active document and both visible comparison panes, leaving other tabs alone.
    pub(crate) fn set_active_page_gaps(&mut self, show: bool) {
        let Some((index, id)) = self.active_ids() else { return };
        let partner = side_by_side(self, index).and_then(|(old, _)| self.views.get(old).map(|v| v.id));
        for view in &mut self.views {
            if view.id == id || partner == Some(view.id) {
                view.view.page_gaps = show;
            }
        }
    }

    /// Compare the chosen older document with the active one and show the differences.
    pub fn run_compare(&mut self, ctx: &egui::Context) {
        let Some((i, new)) = self.active_ids() else { return };
        let Some(old) = self.compare_old else { return };
        let count = self.views.get(i).map_or(0, |v| v.page_count());
        let pages = match parse_pages(&self.compare_pages, count) {
            Ok(pages) => pages,
            Err(e) => return self.notify_error(e.to_string()),
        };
        match self.session.compare_pages(old, new, &pages) {
            Ok(result) => {
                let mut visual = self.session.compare_visual(old, new, 72.0).unwrap_or_default();
                if !pages.is_empty() {
                    visual.retain(|(page, _)| pages.contains(page));
                }
                let n = result.changes.len();
                // Side by side when the old version is open in a tab and the window has the room.
                let side_by_side = self.views.iter().any(|v| v.id == old) && ctx.content_rect().width() >= 1000.0;
                self.compare = Some(crate::compare_ui::CompareState {
                    old,
                    new,
                    result,
                    pages,
                    visual,
                    selected: None,
                    side_by_side,
                    views: Default::default(),
                    show_text: true,
                    show_visual: true,
                });
                self.shade_compare(i);
                self.set_right(Some(RightPanel::Compare));
                self.notify(if n == 0 {
                    tl!("The text is the same").to_string()
                } else if n == 1 {
                    tl!("1 difference found").to_string()
                } else {
                    crate::i18n::fmt(tl!("{n} differences found"), &[("n", &n.to_string())])
                });
            }
            Err(e) => self.notify_error(e),
        }
    }

    pub(crate) fn compare_action(&mut self, ctx: &egui::Context, index: usize, a: PanelAction) {
        let Some(s) = self.compare.as_mut() else { return };
        let (old, new, pages) = (s.old, s.new, s.pages.clone());
        match a {
            PanelAction::Select(i) => {
                s.selected = Some(i);
                let Some(ch) = s.result.changes.get(i) else { return };
                let rect = ch.new.rects.first().copied().or(ch.new.near).unwrap_or_default();
                let page = ch.new.page;
                let old_target = (ch.old.page, ch.old.rects.first().copied().or(ch.old.near).unwrap_or_default());
                let v = &mut self.views[index];
                v.go_to_page(page);
                v.session.flash = Some((page, rect.map(|x| x as f32), 0.0));
                // The old file's half jumps to its side of the change too.
                if let Some(oi) = self.views.iter().position(|v| v.id == old) {
                    let v = &mut self.views[oi];
                    v.go_to_page(old_target.0);
                    v.session.flash = Some((old_target.0, old_target.1.map(|x| x as f32), 0.0));
                }
            }
            PanelAction::SideBySide(on) => {
                s.side_by_side = on;
            }
            PanelAction::Show { text, visual } => {
                s.show_text = text;
                s.show_visual = visual;
                self.shade_compare(index);
            }
            PanelAction::Mark => match self.session.mark_differences_of(old, new, &pages) {
                Ok(n) => {
                    if let Some(info) = self.session.get(new).map(|d| d.info.clone()) {
                        self.views[index].document_changed(ctx, &info);
                    }
                    self.notify(if n == 1 {
                        tl!("Added 1 comment").to_string()
                    } else {
                        crate::i18n::fmt(tl!("Added {n} comments"), &[("n", &n.to_string())])
                    });
                }
                Err(e) => self.notify_error(e),
            },
            PanelAction::Report => match self.session.compare_report_of(old, new, &pages) {
                Ok(bytes) => {
                    if let Err(e) = self.open_bytes(ctx, "Compare Report.pdf", None, bytes.to_vec()) {
                        self.notify_error(e);
                    }
                }
                Err(e) => self.notify_error(e),
            },
            PanelAction::Clear => {
                self.compare = None;
                self.views[index].session.compare_marks.clear();
                if let Some(oi) = self.views.iter().position(|v| v.id == old) {
                    self.views[oi].session.compare_marks.clear();
                }
                self.set_right(None);
            }
        }
    }
}

impl PdfCraftApp {
    /// Shade the comparison on the new file's pages (view `index`) and, when it is open, the old
    /// file's, with the kinds of change the filter shows.
    fn shade_compare(&mut self, index: usize) {
        let Some(s) = self.compare.as_ref() else { return };
        let old = s.old;
        let (old_marks, new_marks) = (shading(s, true), shading(s, false));
        if let Some(v) = self.views.get_mut(index) {
            v.session.compare_marks = new_marks;
        }
        if let Some(v) = self.views.iter_mut().find(|v| v.id == old) {
            v.session.compare_marks = old_marks;
        }
    }
}

/// One half's shading: its side of the text changes (the old file has no side of an insertion)
/// and the visual differences no text change there covers, so both versions read with their own
/// shading. Hidden kinds are left out after the overlap test, so hiding the text does not uncover
/// the visual differences a text change explains.
fn shading(s: &CompareState, old: bool) -> Vec<(usize, [f32; 4], Color32)> {
    let overlaps = |p: usize, r: [f32; 4], marks: &[(usize, [f32; 4], Color32)]| {
        marks.iter().any(|(mp, m, _)| *mp == p && m[0] <= r[2] && r[0] <= m[2] && m[1] <= r[3] && r[1] <= m[3])
    };
    let mut text = Vec::new();
    for ch in s.result.changes.iter().filter(|ch| !(old && ch.kind == Kind::Inserted)) {
        let side = if old { &ch.old } else { &ch.new };
        text.extend(side.rects.iter().map(|r| (side.page, r.map(|v| v as f32), colour(ch.kind))));
    }
    let mut visual = Vec::new();
    for (p, r) in &s.visual {
        let r = r.map(|v| v as f32);
        // The old file's half also skips a region overlapping one it already shades.
        if !overlaps(*p, r, &text) && !(old && overlaps(*p, r, &visual)) {
            visual.push((*p, r, VISUAL));
        }
    }
    let mut out = if s.show_text { text } else { Vec::new() };
    if s.show_visual {
        out.extend(visual);
    }
    out
}

/// The two halves of a side-by-side comparison, as view indexes (old, new), when the active
/// document is the new file of a comparison shown side by side and the old file is open too.
pub(crate) fn side_by_side(app: &PdfCraftApp, index: usize) -> Option<(usize, usize)> {
    let c = app.compare.as_ref().filter(|c| c.side_by_side)?;
    let new = app.views.get(index).filter(|v| v.id == c.new)?;
    let old = app.views.iter().position(|v| v.id == c.old)?;
    (old != index).then_some((old, app.views.iter().position(|v| v.id == new.id)?))
}

/// Side by side: the old file on the left, the new on the right, each a page view of its own
/// with its shading. The pair keeps them scrolled and zoomed together, the half last pressed leading.
pub(crate) fn compare_area(app: &mut PdfCraftApp, old: usize, new: usize, ui: &mut egui::Ui) {
    let (Some(old_view), Some(new_view)) = (app.views.get(old), app.views.get(new)) else { return };
    let ids = [old_view.id, new_view.id];
    let mut states = (old_view.view.clone(), new_view.view.clone());
    let names = ids.map(|id| app.session.get(id).map(|d| d.name.clone()).unwrap_or_default());
    let Some(mut pair) = app.compare.as_mut().map(|c| std::mem::take(&mut c.views)) else { return };
    let left = crate::i18n::fmt(tl!("Old file: {name}"), &[("name", &names[0])]);
    let right = crate::i18n::fmt(tl!("New file: {name}"), &[("name", &names[1])]);
    SyncedViews::new(tl!("Comparison").to_string(), &mut pair).captions(left, right).halves(
        ui,
        (&mut states.0, &mut states.1),
        |ui, rect, state, half| {
            let index = if half == Lead::Left { old } else { new };
            // The app draws through its own view state: the pair's state for this half stands in for it during the draw.
            if let Some(v) = app.views.get_mut(index) {
                std::mem::swap(&mut v.view, state);
            }
            let output = crate::canvas::document_area_at(app, index, rect, ui);
            if let Some(v) = app.views.get_mut(index) {
                std::mem::swap(&mut v.view, state);
            }
            output
        },
    );
    // A half can close its own tab while it draws (#3915): each is found again by id.
    for (id, state) in ids.into_iter().zip([states.0, states.1]) {
        if let Some(v) = app.views.iter_mut().find(|v| v.id == id) {
            v.view = state;
        }
    }
    if let Some(c) = app.compare.as_mut() {
        c.views = pair;
    }
}
