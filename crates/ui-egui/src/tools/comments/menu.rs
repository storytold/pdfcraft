use super::paint::edit_anchor;
use super::state::{Composer, ComposerKind};
use super::{
    Annotation, Color32, CommentPrefs, CommentTool, DocInfo, DocView, Edit, Rect, ReviewState, Rgb, SELECT_BLUE, SWATCHES, Sense, Stroke, color32,
    icons, theme, tool_for, vec2,
};

/// Save attachment… and, for a PDF, Open in a new tab, first in a file attachment comment's
/// menus as in the Attachments panel: the comment is where the reader finds its file. Reading
/// the file changes nothing, so a read-only document offers them too.
pub(crate) fn attachment_items(ui: &mut egui::Ui, view: &mut DocView, info: &DocInfo, a: &Annotation) {
    let source = pdfcraft_render::AttachmentSource::Annotation { page: a.page, index: a.index };
    let Some(file) = info.attachments.iter().find(|f| f.source == source) else { return };
    if ui.button(tl!("Save attachment…")).clicked() {
        view.comments.attachment_request = Some((a.page, a.index, false));
        ui.close();
    }
    if file.name.to_lowercase().ends_with(".pdf") && ui.button(tl!("Open in a new tab")).clicked() {
        view.comments.attachment_request = Some((a.page, a.index, true));
        ui.close();
    }
    ui.separator();
}

/// Items of the canvas context menu for the selected comment (or the page).
/// `can_modify`: the document's pages may change (Flatten comments draws into them).
/// `deletable`: whether Delete would delete the comment at (page, index) (the engine's
/// `Document::annotation_deletable`: unlocked, and no locked reply to it).
pub(crate) fn context_menu(
    ui: &mut egui::Ui,
    view: &mut DocView,
    info: &DocInfo,
    deletable: &dyn Fn(usize, usize) -> bool,
    prefs: &CommentPrefs,
    allowed: bool,
    can_modify: bool,
) -> Option<CanvasAction> {
    let mut action = None;
    let selected = view.comments.selected.and_then(|(p, i)| info.annotations.iter().find(|a| a.page == p && a.index == i && !a.is_reply()));
    match selected {
        Some(a) => {
            let (page, index) = (a.page, a.index);
            attachment_items(ui, view, info, a);
            if ui.add_enabled(allowed, egui::Button::new(tl!("Edit text…"))).clicked() {
                view.comments.composer = Some(Composer {
                    page,
                    at: edit_anchor(info, a),
                    kind: ComposerKind::Edit(index),
                    text: a.contents.clone().unwrap_or_default(),
                    focus: true,
                });
                ui.close();
            }
            if ui.add_enabled(allowed, egui::Button::new(tl!("Reply"))).clicked() {
                view.comments.reveal = true;
                action = Some(CanvasAction::OpenComments);
                ui.close();
            }
            ui.add_enabled_ui(allowed, |ui| {
                ui.menu_button(tl!("Set status"), |ui| {
                    for s in [ReviewState::None, ReviewState::Accepted, ReviewState::Cancelled, ReviewState::Completed, ReviewState::Rejected] {
                        if ui.button(tl!(s.name())).clicked() {
                            action =
                                Some(CanvasAction::Edit(Box::new(Edit::SetAnnotationStatus { page, index, state: s, author: prefs.author.clone() })));
                            ui.close();
                        }
                    }
                });
                let keep_open = egui::containers::menu::MenuConfig::new().close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
                let _ = egui::containers::menu::SubMenuButton::new(tl!("Colour")).config(keep_open).ui(ui, |ui| {
                    if let Some(c) = swatch_grid(ui, a.color.map(|c| c.map(f64::from))) {
                        action = Some(CanvasAction::Edit(Box::new(Edit::StyleAnnotation {
                            page,
                            index,
                            color: Some(c),
                            opacity: None,
                            width: None,
                            endings: None,
                        })));
                        ui.close();
                    }
                });
            });
            let thread: Vec<&pdfcraft_render::Annotation> = info.annotations.iter().filter(|r| r.replies_to(a)).collect();
            let marked = crate::comments_panel::is_marked(&thread);
            if ui.add_enabled(allowed, egui::Button::new(tl!(if marked { "Remove checkmark" } else { "Mark with checkmark" }))).clicked() {
                action = Some(CanvasAction::Edit(Box::new(Edit::MarkAnnotation { page, index, marked: !marked, author: prefs.author.clone() })));
                ui.close();
            }
            if ui.button(tl!("Copy text")).clicked() {
                ui.ctx().copy_text(a.contents.clone().unwrap_or_default());
                ui.close();
            }
            ui.separator();
            // The engine's own check: a locked comment, or one with a locked reply, stays.
            if ui.add_enabled(allowed && deletable(page, index), egui::Button::new(tl!("Delete"))).clicked() {
                view.comments.selected = None;
                action = Some(CanvasAction::Edit(Box::new(Edit::DeleteAnnotation { page, index })));
                ui.close();
            }
            if ui.add_enabled(allowed, egui::Button::new(tl!("Properties…"))).clicked() {
                action = Some(CanvasAction::Properties(page, index));
                ui.close();
            }
            if ui.add_enabled(tool_for(a).is_some(), egui::Button::new(tl!("Make Current Properties Default"))).clicked() {
                view.comments.default_request = Some((page, index));
                ui.close();
            }
        }
        None => {
            if let Some((page, at)) = view.comments.context_at
                && ui.add_enabled(allowed, egui::Button::new(tl!("Add a sticky note here"))).clicked()
            {
                view.comments.composer = Some(Composer { page, at, kind: ComposerKind::Note, text: String::new(), focus: true });
                ui.close();
            }
            if ui.button(tl!("Comments panel")).clicked() {
                action = Some(CanvasAction::OpenComments);
                ui.close();
            }
        }
    }
    // Every comment of the document into its pages, as Comment ▸ Flatten comments does: signed
    // or reviewed pages whose comments should no longer move.
    if !info.annotations.is_empty() {
        ui.separator();
        if ui.add_enabled(can_modify, egui::Button::new(tl!("Flatten comments"))).clicked() {
            view.comments.selected = None;
            action = Some(CanvasAction::Edit(Box::new(Edit::Flatten { comments: true, fields: false })));
            ui.close();
        }
    }
    action
}

/// What the canvas asks the app to do.
#[derive(Clone, Debug, PartialEq)]
pub enum CanvasAction {
    Edit(Box<Edit>),
    OpenComments,
    /// Comment Properties for (page, index).
    Properties(usize, usize),
    /// Copy Image: image (page, index in `page_images`) to the clipboard.
    CopyImage(usize, usize),
    /// Copy Image in Original Format: the same image as a file (JPEG as stored, else PNG).
    CopyImageFile(usize, usize),
}

/// How many custom colours the "Recent colours" row keeps.
pub(super) const RECENT_CUSTOM: usize = 5;

/// Return the stable egui id for the recent custom-colour palette.
fn recent_custom_id() -> egui::Id {
    egui::Id::unique("pdfcraft-recent-custom-colours")
}

/// Custom colours picked with "Other colour…" this session, newest first (#1193).
pub fn recent_custom_colours(ctx: &egui::Context) -> Vec<Rgb> {
    ctx.data(|d| d.get_temp::<Vec<Rgb>>(recent_custom_id())).unwrap_or_default()
}

/// Put `c` first in the recent custom colours (once, at most [`RECENT_CUSTOM`]). Colours that are
/// already swatches are not repeated there.
pub fn remember_custom_colour(ctx: &egui::Context, c: Rgb) {
    if !c.iter().all(|v| v.is_finite()) || SWATCHES.iter().any(|(_, s)| same_colour(s, &c)) {
        return;
    }
    let c = c.map(|v| v.clamp(0.0, 1.0));
    ctx.data_mut(|d| {
        let recent = d.get_temp_mut_or_default::<Vec<Rgb>>(recent_custom_id());
        recent.retain(|r| !same_colour(r, &c));
        recent.insert(0, c);
        recent.truncate(RECENT_CUSTOM);
    });
}

pub(super) fn same_colour(a: &Rgb, b: &Rgb) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 0.02)
}

fn rgb_of(c: Color32) -> Rgb {
    [c.r(), c.g(), c.b()].map(|v| f64::from(v) / 255.0)
}

/// One round swatch; `on` rings it as the current colour.
fn swatch(ui: &mut egui::Ui, c: Rgb, on: bool, tooltip: &str) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tooltip));
    ui.painter().circle_filled(r.center(), 9.0, color32(c));
    ui.painter().circle_stroke(r.center(), 9.0, Stroke::new(1.0, Color32::from_black_alpha(40)));
    if on || resp.hovered() {
        ui.painter().circle_stroke(r.center(), 11.0, Stroke::new(1.5, SELECT_BLUE));
    }
    resp.on_hover_text(tooltip)
}

/// The "+" swatch that opens "Other colour…": a ring with a plus, the size of a swatch.
fn other_colour_swatch(ui: &mut egui::Ui, open: bool) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(vec2(22.0, 22.0), Sense::click());
    let ink = ui.visuals().widgets.inactive.fg_stroke.color;
    ui.painter().circle_stroke(r.center(), 9.0, Stroke::new(1.0, ink.gamma_multiply(0.6)));
    let c = r.center();
    for d in [vec2(4.5, 0.0), vec2(0.0, 4.5)] {
        ui.painter().line_segment([c - d, c + d], Stroke::new(1.5, ink));
    }
    if open || resp.hovered() {
        ui.painter().circle_stroke(c, 11.0, Stroke::new(1.5, SELECT_BLUE));
    }
    let label = tl!("Other colour…");
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, open, label));
    resp.on_hover_text(label)
}

/// A grid of colour swatches with a "+" swatch for "Other colour…" (a full picker, as Acrobat's
/// quick colours offer) and, once used, the recent custom colours; returns the colour chosen.
/// The "+" sits in the first row, so the grid is no taller than before until the picker opens.
pub fn swatch_grid(ui: &mut egui::Ui, current: Option<Rgb>) -> Option<Rgb> {
    let mut picked = None;
    let on = |c: &Rgb| current.is_some_and(|cur| same_colour(&cur, c));
    // Keyed on this grid's own place in the parent, so two grids in one dialog open their
    // pickers separately.
    let base = ui.next_auto_id();
    ui.skip_ahead_auto_ids(1);
    let open_id = base.with("other-colour");
    let value_id = open_id.with("value");
    let mut open = ui.data(|d| d.get_temp::<bool>(open_id)).unwrap_or(false);
    egui::Grid::new(base.with("swatches")).spacing(vec2(6.0, 6.0)).show(ui, |ui| {
        for (i, (name, c)) in SWATCHES.iter().enumerate() {
            if swatch(ui, *c, on(c), tl!(name)).clicked() {
                picked = Some(*c);
            }
            if i == 4 && other_colour_swatch(ui, open).clicked() {
                open = !open;
                // Start from the current colour each time the picker opens.
                ui.data_mut(|d| d.insert_temp(value_id, current.map_or(Color32::BLACK, color32)));
            }
            if i % 5 == 4 {
                ui.end_row();
            }
        }
    });
    let recent = recent_custom_colours(ui.ctx());
    if !recent.is_empty() {
        ui.label(egui::RichText::new(tl!("Recent colours")).small());
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            for c in &recent {
                let [r, g, b, _] = color32(*c).to_array();
                if swatch(ui, *c, on(c), &format!("#{r:02X}{g:02X}{b:02X}")).clicked() {
                    picked = Some(*c);
                }
            }
        });
    }
    if open && picked.is_none() {
        let mut value = ui.data(|d| d.get_temp::<Color32>(value_id)).unwrap_or_else(|| current.map_or(Color32::BLACK, color32));
        egui::widgets::color_picker::color_picker_color32(ui, &mut value, egui::widgets::color_picker::Alpha::Opaque);
        ui.data_mut(|d| d.insert_temp(value_id, value));
        // Dragging in the picker previews nothing; Apply commits once (one undo step).
        if ui.button(tl!("Apply")).clicked() {
            let c = rgb_of(value);
            remember_custom_colour(ui.ctx(), c);
            picked = Some(c);
        }
    }
    if picked.is_some() {
        open = false;
    }
    ui.data_mut(|d| d.insert_temp(open_id, open));
    picked
}

/// "No fill" and the colour swatches; returns the choice clicked (`Some(None)`: no fill).
pub fn fill_picker(ui: &mut egui::Ui, current: Option<Rgb>) -> Option<Option<Rgb>> {
    let mut picked = None;
    ui.push_id("fill-picker", |ui| {
        ui.vertical(|ui| {
            if ui.selectable_label(current.is_none(), tl!("No fill")).clicked() {
                picked = Some(None);
            }
            if let Some(c) = swatch_grid(ui, current) {
                picked = Some(Some(c));
            }
        });
    });
    picked
}

/// The comment tools' extra quick-bar controls: pin, colour, fill, opacity and thickness.
pub(crate) fn quick_bar_controls(ui: &mut egui::Ui, tool: CommentTool, prefs: &mut CommentPrefs) -> Option<Rgb> {
    let style = prefs.style(tool);
    let mut selected_color = None;
    if icons::button(ui, "pin", 32.0, prefs.pinned, if prefs.pinned { tl!("Keep tool selected: on") } else { tl!("Keep tool selected") }).clicked() {
        prefs.pinned = !prefs.pinned;
    }
    let (r, resp) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::click());
    ui.painter().circle_filled(r.center(), 9.0, color32(style.color));
    ui.painter().circle_stroke(r.center(), 9.0, Stroke::new(1.0, Color32::from_black_alpha(50)));
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tl!("Colour")));
    let resp = resp.on_hover_text(tl!("Colour"));
    egui::Popup::menu(&resp).align(egui::RectAlign::RIGHT_START).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        if let Some(c) = swatch_grid(ui, Some(style.color)) {
            prefs.set_color(tool, c);
            selected_color = Some(c);
            ui.close();
        }
    });
    if tool.has_fill() {
        let resp = fill_button(ui, style.fill);
        egui::Popup::menu(&resp).align(egui::RectAlign::RIGHT_START).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
            ui.label(egui::RichText::new(tl!("Fill colour")).font(theme::semibold(12.0)));
            if let Some(f) = fill_picker(ui, style.fill) {
                prefs.set_fill(tool, f);
                ui.close();
            }
        });
    }
    let resp = icons::button(ui, "blend", 32.0, false, tl!("Opacity"));
    egui::Popup::menu(&resp).align(egui::RectAlign::RIGHT_START).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        ui.set_min_width(180.0);
        let mut percent = (style.opacity * 100.0).round();
        ui.label(egui::RichText::new(tl!("Opacity")).font(theme::semibold(12.0)));
        if ui.add(egui::Slider::new(&mut percent, 10.0..=100.0).step_by(5.0).suffix(" %")).changed() {
            prefs.set_opacity(tool, percent / 100.0);
        }
    });
    if tool.has_width() {
        let resp = icons::button(ui, "sliders-horizontal", 32.0, false, tl!("Line thickness"));
        egui::Popup::menu(&resp).align(egui::RectAlign::RIGHT_START).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
            ui.set_min_width(180.0);
            let mut w = style.width;
            ui.label(egui::RichText::new(tl!("Line thickness")).font(theme::semibold(12.0)));
            if ui.add(egui::Slider::new(&mut w, tool.min_width()..=12.0).step_by(0.5).suffix(" pt")).changed() {
                prefs.set_width(tool, w);
            }
        });
    }
    selected_color
}

/// The quick bar's fill button: a filled square in the fill colour, or a crossed-out outline
/// for no fill.
fn fill_button(ui: &mut egui::Ui, fill: Option<Rgb>) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::click());
    let sq = Rect::from_center_size(r.center(), vec2(16.0, 16.0));
    let edge = Stroke::new(1.0, Color32::from_black_alpha(90));
    match fill {
        Some(c) => {
            ui.painter().rect_filled(sq, 2.0, color32(c));
        }
        None => {
            ui.painter().line_segment([sq.left_bottom(), sq.right_top()], Stroke::new(1.5, Color32::from_rgb(0xD3, 0x2F, 0x2F)));
        }
    }
    ui.painter().rect_stroke(sq, 2.0, edge, egui::StrokeKind::Inside);
    if resp.hovered() {
        ui.painter().rect_stroke(r.shrink(2.0), 6.0, Stroke::new(1.0, Color32::from_black_alpha(40)), egui::StrokeKind::Inside);
    }
    let label = match fill {
        Some(_) => tl!("Fill colour"),
        None => tl!("Fill colour: none"),
    };
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    resp.on_hover_text(label)
}

/// Status badge icon and label for a review state name.
pub fn status_badge(state: &str) -> Option<(&'static str, &'static str, Color32)> {
    match state {
        "Accepted" => Some(("thumbs-up", "Accepted", Color32::from_rgb(0x2D, 0x9D, 0x5B))),
        "Rejected" => Some(("thumbs-down", "Rejected", Color32::from_rgb(0xD3, 0x2F, 0x2F))),
        "Cancelled" => Some(("circle-x", "Cancelled", Color32::from_rgb(0x8E, 0x8E, 0x8E))),
        "Completed" => Some(("check", "Completed", Color32::from_rgb(0x2D, 0x9D, 0x5B))),
        _ => None,
    }
}
