use craft::behavior::{IconId, Mixed};
use craft::ui::buttons::{IconButton, ToggleButton};
use craft::ui::choice::Swatch;
use craft::ui::contract::{Bind, Props as _, Show as _, ShowContainer as _};
use craft::ui::display::Divider;
use craft::ui::form::SliderField;

use craft::review::ui::annotate::AnnotateWords;
use craft::ui::menu::{Menu, MenuItem};
use craft::ui::popover::Popover;
use craft::ui::runtime::env::Env;
use craft::ui::runtime::requests::ColorRequest;

// PLAN: `mod colours;` is declared in `tools/comments.rs` (pdf-comments-w7).
use super::colours::{self, RecentColours, fill_choice, palette, to_srgba};
use super::state::{Composer, ComposerKind};
use super::{Annotation, CommentPrefs, CommentTool, DocInfo, DocView, Edit, ReviewState, Rgb, defaultable};

/// A row of the attachment actions: its label and whether the file opens in a new tab rather than being saved.
pub(crate) struct AttachmentRow {
    pub(crate) label: &'static str,
    pub(crate) open_in_tab: bool,
}

/// The attachment actions of a file attachment comment, written once for the menu and the Comments panel card:
/// Save attachment… and, for a PDF, Open in a new tab. Reading the file changes nothing, so a read-only document
/// offers them too.
pub(crate) fn attachment_rows(info: &DocInfo, a: &Annotation) -> Vec<AttachmentRow> {
    let source = pdfcraft_render::AttachmentSource::Annotation { page: a.page, index: a.index };
    let Some(file) = info.attachments.iter().find(|f| f.source == source) else { return Vec::new() };
    let mut rows = vec![AttachmentRow { label: tl!("Save attachment…"), open_in_tab: false }];
    if file.name.to_lowercase().ends_with(".pdf") {
        rows.push(AttachmentRow { label: tl!("Open in a new tab"), open_in_tab: true });
    }
    rows
}

/// The attachment rows first in a file attachment comment's menu, with a rule after them.
pub(crate) fn attachment_items(ui: &mut egui::Ui, view: &mut DocView, info: &DocInfo, a: &Annotation) {
    let rows = attachment_rows(info, a);
    for row in &rows {
        if MenuItem::new(row.label).show(ui).ran {
            view.session.comments.attachment_request = Some((a.page, a.index, row.open_in_tab));
        }
    }
    if !rows.is_empty() {
        Divider::horizontal().show(ui);
    }
}

/// What the canvas context menu may offer.
#[derive(Clone, Copy, Debug)]
pub(crate) struct MenuRights {
    /// Comments may be added and changed.
    pub(crate) allowed: bool,
    /// The document's pages may change (Flatten comments draws into them).
    pub(crate) can_modify: bool,
    /// Comments are on the comment clipboard, to paste.
    pub(crate) can_paste: bool,
}

/// What the canvas asks the app to do.
#[derive(Clone, Debug, PartialEq)]
pub enum CanvasAction {
    Edit(Box<Edit>),
    OpenComments,
    /// Run this command of the command board (Copy and Paste of the comments).
    Command(&'static str),
    /// Comment Properties for (page, index).
    Properties(usize, usize),
    /// Put this text on the clipboard, through the app's clipboard service.
    CopyText(String),
    /// Give every selected comment this colour.
    Colour(Rgb),
    /// Open the host's colour picker for the Colour submenu's Other… cell; its answer colours the selected comments.
    ColourRequest(ColorRequest),
    /// Copy Image: image (page, index in `page_images`) to the clipboard.
    CopyImage(usize, usize),
    /// Copy Image in Original Format: the same image as a file (JPEG as stored, else PNG).
    CopyImageFile(usize, usize),
}

/// What the comment rows of the canvas context menu read.
pub(crate) struct CommentMenuCx<'a> {
    pub(crate) view: &'a mut DocView,
    pub(crate) info: &'a DocInfo,
    /// Whether Delete would delete the comment at (page, index) (the engine's `Document::annotation_deletable`:
    /// unlocked, and no locked reply to it).
    pub(crate) deletable: &'a dyn Fn(usize, usize) -> bool,
    pub(crate) prefs: &'a CommentPrefs,
    pub(crate) rights: MenuRights,
    pub(crate) recent: &'a RecentColours,
    /// The active comment tool, for the rows of a shape being drawn.
    pub(crate) tool: Option<CommentTool>,
}

fn row(ui: &mut egui::Ui, label: &str, enabled: bool) -> bool {
    MenuItem::new(label.to_string()).enabled(enabled).show(ui).ran
}

/// Rows of the canvas context menu for the selected comment (or the page), inside `Menu::context`.
pub(crate) fn context_rows(ui: &mut egui::Ui, cx: CommentMenuCx<'_>) -> Option<CanvasAction> {
    let CommentMenuCx { view, info, deletable, prefs, rights, recent, tool } = cx;
    let MenuRights { allowed, can_modify, can_paste } = rights;
    let mut action = None;
    // Connected lines, a polygon or a cloud being drawn: finish it from the menu, as other editors do (Enter
    // and a double-click finish it too), take the last point back, or stop.
    if let Some(tool) = tool.filter(|t| t.clicks_points())
        && !view.session.comments.tool.drawing().is_empty()
    {
        let enough = view.session.comments.tool.drawing().len() >= if tool == CommentTool::PolyLine { 2 } else { 3 };
        if row(ui, tl!("Complete"), enough) {
            let words = Env::of(ui).words::<AnnotateWords>();
            view.session.comments.tool.finish_drawing(&words);
        }
        if row(ui, tl!("Undo last point"), true) {
            view.session.comments.tool.undo_point();
        }
        if row(ui, tl!("Cancel drawing"), true) {
            view.session.comments.tool.cancel_drawing();
        }
        return None;
    }
    // PLAN: assumed `CommentView::primary() -> Option<(usize, usize)>` and `selection()` (pdf-comments-w7, A-2)
    let selected = view.session.comments.primary().and_then(|(p, i)| info.annotations.iter().find(|a| a.page == p && a.index == i && !a.is_reply()));
    let several = view.session.comments.selection().len() > 1;
    match selected {
        Some(a) => {
            let (page, index) = (a.page, a.index);
            attachment_items(ui, view, info, a);
            if row(ui, tl!("Edit text…"), allowed) {
                view.session.comments.composer = Some(Composer {
                    page,
                    at: super::bridge::edit_anchor(info, a),
                    kind: ComposerKind::Edit(index),
                    text: a.contents.clone().unwrap_or_default(),
                    focus: true,
                });
            }
            if row(ui, tl!("Reply"), allowed) {
                view.session.comments.reveal = true;
                action = Some(CanvasAction::OpenComments);
            }
            Menu::submenu_row_enabled(ui, tl!("Set status"), allowed, |ui, _| {
                for s in [ReviewState::None, ReviewState::Accepted, ReviewState::Cancelled, ReviewState::Completed, ReviewState::Rejected] {
                    if row(ui, tl!(s.name()), true) {
                        action =
                            Some(CanvasAction::Edit(Box::new(Edit::SetAnnotationStatus { page, index, state: s, author: prefs.author.clone() })));
                    }
                }
            });
            Menu::submenu_row_enabled(ui, tl!("Colour"), allowed, |ui, _| {
                let pick = palette(ui, tl!("Colour"), a.color.map(|c| c.map(f64::from)), "comment.menu.colour", recent);
                if let Some(c) = pick.picked {
                    // Several selected: the app gives each of them the colour.
                    action = Some(if several {
                        CanvasAction::Colour(c)
                    } else {
                        CanvasAction::Edit(Box::new(Edit::StyleAnnotation { page, index, color: Some(c), opacity: None, width: None, endings: None }))
                    });
                }
                if let Some(request) = pick.request {
                    action = Some(CanvasAction::ColourRequest(request));
                }
            });
            // The checkmark of the reviewer using this copy: checkmarks are each reviewer's own
            // (ISO 32000-2 Table 174: "The annotation has been marked by the user").
            let marked = pdfcraft_render::ReviewStates::new(&info.annotations).marked_by(a, &prefs.author);
            if row(ui, tl!(if marked { "Remove checkmark" } else { "Mark with checkmark" }), allowed) {
                action = Some(CanvasAction::Edit(Box::new(Edit::MarkAnnotation { page, index, marked: !marked, author: prefs.author.clone() })));
            }
            if row(ui, tl!("Copy text"), true) {
                action = Some(CanvasAction::CopyText(a.contents.clone().unwrap_or_default()));
            }
            Divider::horizontal().show(ui);
            // The comments themselves, to paste into this or another document; the rows take their chords
            // from the command board.
            // The command board owns both gates: the Paste handler (pdf-canvas-w16) reads `allowed`, the
            // clipboard and the paste place (`view.comments.context_at`).
            // Worded and gated as the comment clipboard works: Paste comments waits for copied comments.
            let chord = |ui: &egui::Ui, id: &str| crate::commands::chord_label(ui.ctx(), id);
            let mut copy = MenuItem::new(tl!(if several { "Copy comments" } else { "Copy comment" }));
            if let Some(text) = chord(ui, "edit.copy") {
                copy = copy.shortcut(text);
            }
            if copy.show(ui).ran {
                action = Some(CanvasAction::Command("edit.copy"));
            }
            let mut paste = MenuItem::new(tl!("Paste comments")).enabled(allowed && can_paste);
            if let Some(text) = chord(ui, "edit.paste") {
                paste = paste.shortcut(text);
            }
            if paste.show(ui).ran {
                action = Some(CanvasAction::Command("edit.paste"));
            }
            Divider::horizontal().show(ui);
            // The engine's own check, for each selected comment: a locked comment, or one with a locked
            // reply, stays. On a group subordinate, Delete takes just it, as Acrobat's does.
            let (dpage, dindex) = view.session.comments.target().unwrap_or((page, index));
            let deletes = deletable(dpage, dindex) && view.session.comments.selection().iter().all(|&(p, i)| deletable(p, i));
            if row(ui, tl!(if several { "Delete comments" } else { "Delete" }), allowed && deletes) {
                action = super::bridge::delete_selected(&mut view.session.comments, info).map(|e| CanvasAction::Edit(Box::new(e)));
            }
            if row(ui, tl!("Properties…"), allowed) {
                action = Some(CanvasAction::Properties(page, index));
            }
            if row(ui, tl!("Make Current Properties Default"), defaultable(a)) {
                view.session.comments.default_request = Some((page, index));
            }
        }
        None => {
            if let Some((page, at)) = view.session.comments.context_at
                && row(ui, tl!("Add a sticky note here"), allowed)
            {
                view.session.comments.composer = Some(Composer { page, at, kind: ComposerKind::Note, text: String::new(), focus: true });
            }
            if row(ui, tl!("Comments panel"), true) {
                action = Some(CanvasAction::OpenComments);
            }
        }
    }
    // Every comment of the document into its pages, as Comment ▸ Flatten comments does: signed or reviewed
    // pages whose comments should no longer move.
    if !info.annotations.is_empty() {
        Divider::horizontal().show(ui);
        if row(ui, tl!("Flatten comments"), can_modify) {
            view.session.comments.select(None);
            action = Some(CanvasAction::Edit(Box::new(Edit::Flatten { comments: true, fields: false })));
        }
    }
    action
}

/// What the quick bar's controls reported this pass.
#[derive(Default)]
pub(crate) struct QuickBarPick {
    /// The colour picked from a swatch, for the selected comments this tool made.
    pub(crate) colour: Option<Rgb>,
    /// The Other… cell of the colour or fill palette was activated: the caller opens the picker for it.
    pub(crate) request: Option<ColorRequest>,
}

/// A control that opens a popover under it: `opener` draws the control, a click on it opens and closes the
/// popover, and `body` fills it; a body that reports it is done (a choice that ends the popover's use) closes it. The
/// popover's open flag is the opener's own, kept in the UI's memory. The framework runs `body` in each of its passes
/// (sizing, then the one that takes input), so a body keeps what a pass picked rather than overwrite it with a later
/// pass's nothing.
pub(crate) fn popover_from(
    ui: &mut egui::Ui,
    label: &str,
    opener: impl FnOnce(&mut egui::Ui) -> egui::Response,
    mut body: impl FnMut(&mut egui::Ui) -> bool,
) {
    let resp = opener(ui);
    let key = resp.id.with("popover-open");
    let mut open = ui.memory(|m| m.data.get_temp::<bool>(key)).unwrap_or(false);
    // A click by the pointer or by keyboard activation.
    if craft::ui::contract::clicked(&resp) {
        open = !open;
    }
    let mut done = false;
    // The popover runs its body in a sizing pass and again in the pass that shows it.
    Popover::new(label.to_string(), Bind::bound(&mut open)).opened_by(&resp).show(ui, |ui, _| {
        done |= body(ui);
    });
    open &= !done;
    ui.memory_mut(|m| m.data.insert_temp(key, open));
}

/// The comment tools' extra quick-bar controls: pin, colour, fill, opacity and thickness.
pub(crate) fn quick_bar_controls(ui: &mut egui::Ui, tool: CommentTool, prefs: &mut CommentPrefs, recent: &RecentColours) -> QuickBarPick {
    let style = prefs.style(tool);
    let mut out = QuickBarPick::default();
    let mut pinned = prefs.pinned;
    ToggleButton::new(tl!("Keep tool selected"), Bind::bound(&mut pinned)).icon(IconId::new("pin")).icon_only().show(ui);
    prefs.pinned = pinned;
    popover_from(
        ui,
        tl!("Colour"),
        |ui| Swatch::new(tl!("Colour"), colours::to_srgba(style.color)).show(ui).response,
        |ui| {
            let pick = palette(ui, tl!("Colour"), Some(style.color), "comment.tool.colour", recent);
            if let Some(c) = pick.picked {
                prefs.set_color(tool, c);
                out.colour = Some(c);
            }
            if pick.request.is_some() {
                out.request = pick.request;
            }
            false
        },
    );
    if tool.has_fill() {
        popover_from(
            ui,
            tl!("Fill colour"),
            |ui| {
                let label = if style.fill.is_some() { tl!("Fill colour") } else { tl!("Fill colour: none") };
                Swatch::optional(label, Mixed::Same(style.fill.map(to_srgba))).show(ui).response
            },
            |ui| {
                let pick = fill_choice(ui, style.fill, "comment.tool.fill", recent);
                if let Some(f) = pick.choice {
                    prefs.set_fill(tool, f);
                }
                if pick.request.is_some() {
                    out.request = pick.request;
                }
                false
            },
        );
    }
    popover_from(
        ui,
        tl!("Opacity"),
        |ui| IconButton::new(IconId::new("blend")).label(tl!("Opacity")).show(ui).response,
        |ui| {
            let mut percent = (style.opacity * 100.0).round();
            let start = percent;
            SliderField::new(tl!("Opacity"), Bind::bound(&mut percent), 10.0..=100.0).step(5.0).unit("%").show(ui);
            if percent != start {
                // The whole comment's: its fill and text take it too.
                prefs.set_opacity(tool, percent / 100.0);
                prefs.set_part_opacity(tool, None, None);
            }
            false
        },
    );
    if tool.has_width() {
        popover_from(
            ui,
            tl!("Line thickness"),
            |ui| IconButton::new(IconId::new("sliders-horizontal")).label(tl!("Line thickness")).show(ui).response,
            |ui| {
                let mut w = style.width;
                SliderField::new(tl!("Line thickness"), Bind::bound(&mut w), tool.min_width()..=super::MAX_LINE_WIDTH).step(0.5).unit("pt").show(ui);
                if w != style.width {
                    prefs.set_width(tool, w);
                }
                false
            },
        );
    }
    out
}
