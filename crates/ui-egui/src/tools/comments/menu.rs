use super::input::delete_selected;
use super::paint::edit_anchor;
use super::state::{Composer, ComposerKind, Gesture};
use pdfcraft_engine::CommentProps;

use super::{
    Annotation, Color32, CommentPrefs, CommentTool, CornerRadius, DocInfo, DocView, Edit, PageCx, PagePointer, QuickTool, Rect, ReviewState, Rgb,
    SELECT_BLUE, SWATCHES, Sense, Stroke, color32, defaultable, icons, pos2, theme, vec2,
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

/// What the canvas context menu may offer.
#[derive(Clone, Copy, Debug)]
pub(crate) struct MenuRights {
    /// Comments may be added and changed.
    pub(crate) allowed: bool,
    /// Copied comments wait to be pasted.
    pub(crate) can_paste: bool,
    /// The document's pages may change (Flatten comments draws into them).
    pub(crate) can_modify: bool,
}

/// Items of the canvas context menu for the selected comment (or the page).
/// `deletable`: whether Delete would delete the comment at (page, index) (the engine's
/// `Document::annotation_deletable`: unlocked, and no locked reply to it).
pub(crate) fn context_menu(
    ui: &mut egui::Ui,
    view: &mut DocView,
    info: &DocInfo,
    deletable: &dyn Fn(usize, usize) -> bool,
    prefs: &CommentPrefs,
    rights: MenuRights,
) -> Option<CanvasAction> {
    let MenuRights { allowed, can_paste, can_modify } = rights;
    let mut action = None;
    // Connected lines, a polygon or a cloud being drawn: finish it from the menu, as other
    // editors do (Enter and a double-click finish it too), take the last point back, or stop.
    if let Some(Gesture::Draw { tool, points, .. }) = &view.comments.gesture
        && tool.clicks_points()
    {
        let enough = points.len() >= if *tool == CommentTool::PolyLine { 2 } else { 3 };
        if ui.add_enabled(enough, egui::Button::new(tl!("Complete"))).clicked() {
            view.comments.finish_request = true;
            ui.close();
        }
        if ui.button(tl!("Undo last point")).clicked() {
            view.comments.undo_point();
            ui.close();
        }
        if ui.button(tl!("Cancel drawing")).clicked() {
            view.comments.gesture = None;
            ui.close();
        }
        return None;
    }
    let selected = view.comments.selected.and_then(|(p, i)| info.annotations.iter().find(|a| a.page == p && a.index == i && !a.is_reply()));
    let several = view.comments.also.len();
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
                        // Several selected: the app gives each of them the colour.
                        action = Some(if several > 0 {
                            CanvasAction::Colour(c)
                        } else {
                            CanvasAction::Edit(Box::new(Edit::StyleAnnotation {
                                page,
                                index,
                                color: Some(c),
                                opacity: None,
                                width: None,
                                endings: None,
                            }))
                        });
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
            // The comments themselves, to paste into this or another document (⌘C / ⌘V too).
            let copy_key = crate::commands::shortcut_label(ui.ctx(), crate::commands::COPY);
            if ui.add(egui::Button::new(tl!(if several > 0 { "Copy comments" } else { "Copy comment" })).shortcut_text(copy_key)).clicked() {
                view.comments.copy_request = true;
                ui.close();
            }
            let paste_key = crate::commands::shortcut_label(ui.ctx(), crate::commands::PASTE);
            if ui.add_enabled(allowed && can_paste, egui::Button::new(tl!("Paste comments")).shortcut_text(paste_key)).clicked() {
                let at = view.comments.context_at.unwrap_or((page, [f64::from(a.rect[0]), f64::from(a.rect[1])]));
                view.comments.paste_request = Some(at);
                ui.close();
            }
            ui.separator();
            // The engine's own check, for each selected comment: a locked comment, or one with a
            // locked reply, stays.
            let deletes = deletable(page, index) && view.comments.also.iter().all(|&(p, i)| deletable(p, i));
            if ui.add_enabled(allowed && deletes, egui::Button::new(tl!(if several > 0 { "Delete comments" } else { "Delete" }))).clicked() {
                action = delete_selected(&mut view.comments).map(|e| CanvasAction::Edit(Box::new(e)));
                ui.close();
            }
            if ui.add_enabled(allowed, egui::Button::new(tl!("Properties…"))).clicked() {
                action = Some(CanvasAction::Properties(page, index));
                ui.close();
            }
            if ui.add_enabled(defaultable(a), egui::Button::new(tl!("Make Current Properties Default"))).clicked() {
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
    /// Give every selected comment this colour.
    Colour(Rgb),
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

pub(crate) fn quick_bar_colour(ui: &mut egui::Ui, current: Rgb) -> Option<Rgb> {
    let (r, resp) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::click());
    ui.painter().circle_filled(r.center(), 9.0, color32(current));
    ui.painter().circle_stroke(r.center(), 9.0, Stroke::new(1.0, Color32::from_black_alpha(50)));
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tl!("Colour")));
    let resp = resp.on_hover_text(tl!("Colour"));
    let mut picked = None;
    egui::Popup::menu(&resp).align(egui::RectAlign::RIGHT_START).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        if let Some(c) = swatch_grid(ui, Some(current)) {
            picked = Some(c);
            ui.close();
        }
    });
    picked
}

/// The comment tools' extra quick-bar controls: pin, colour, fill, opacity and thickness.
pub(crate) fn quick_bar_controls(ui: &mut egui::Ui, tool: CommentTool, prefs: &mut CommentPrefs) -> Option<Rgb> {
    let style = prefs.style(tool);
    if icons::button(ui, "pin", 32.0, prefs.pinned, if prefs.pinned { tl!("Keep tool selected: on") } else { tl!("Keep tool selected") }).clicked() {
        prefs.pinned = !prefs.pinned;
    }
    let selected_color = quick_bar_colour(ui, style.color);
    if let Some(c) = selected_color {
        prefs.set_color(tool, c);
    }
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
        // The whole comment's: its fill and text take it too.
        if ui.add(egui::Slider::new(&mut percent, 10.0..=100.0).step_by(5.0).suffix(" %")).changed() {
            prefs.set_opacity(tool, percent / 100.0);
            prefs.set_part_opacity(tool, None, None);
        }
    });
    if tool.has_width() {
        let resp = icons::button(ui, "sliders-horizontal", 32.0, false, tl!("Line thickness"));
        egui::Popup::menu(&resp).align(egui::RectAlign::RIGHT_START).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
            ui.set_min_width(180.0);
            let mut w = style.width;
            ui.label(egui::RichText::new(tl!("Line thickness")).font(theme::semibold(12.0)));
            if ui.add(egui::Slider::new(&mut w, tool.min_width()..=super::MAX_LINE_WIDTH).step_by(0.5).suffix(" pt")).changed() {
                prefs.set_width(tool, w);
            }
        });
    }
    selected_color
}

/// A border-colour button: a hollow square in the border's colour (`current`, else the text's,
/// `text`), with a menu of "Same as text" and the swatches. Returns the choice (`Some(None)`:
/// the text's colour again).
pub(crate) fn border_colour_button(ui: &mut egui::Ui, current: Option<Rgb>, text: Rgb) -> Option<Option<Rgb>> {
    let (r, resp) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::click());
    let sq = Rect::from_center_size(r.center(), vec2(15.0, 15.0));
    ui.painter().rect_stroke(sq, 1.0, Stroke::new(3.0, color32(current.unwrap_or(text))), egui::StrokeKind::Inside);
    if resp.hovered() {
        ui.painter().rect_stroke(r.shrink(2.0), 4.0, Stroke::new(1.0, Color32::from_black_alpha(40)), egui::StrokeKind::Inside);
    }
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, tl!("Border colour")));
    let resp = resp.on_hover_text(tl!("Border colour"));
    let mut picked = None;
    egui::Popup::menu(&resp).align(egui::RectAlign::RIGHT_START).show(|ui| {
        ui.label(egui::RichText::new(tl!("Border colour")).font(theme::semibold(12.0)));
        if ui.add(egui::Button::new(tl!("Same as text")).selected(current.is_none())).clicked() {
            picked = Some(None);
            ui.close();
        }
        if let Some(c) = swatch_grid(ui, current) {
            picked = Some(Some(c));
            ui.close();
        }
    });
    picked
}

/// The floating bar over the selected comment (Acrobat shows a mini toolbar on a selected
/// comment): its colour and opacity, changed in place without opening Properties. Only for a
/// comment whose appearance can be redrawn and that isn't locked, never while a comment is
/// being drawn, moved or typed, and not while `off` (comments hidden, or a drawing tool on).
/// Several selected: the choice goes to every selected comment that can take it, as one step
/// (the context menu's Colour does the same). `props_of`: a comment's properties, the engine's
/// `Document::comment_props`. Returns the edit to apply.
pub(crate) fn style_bar(
    ctx: &egui::Context,
    view: &DocView,
    info: &DocInfo,
    props_of: &dyn Fn(usize, usize) -> Option<CommentProps>,
    prefs: &CommentPrefs,
    allowed: bool,
    off: bool,
) -> Option<Edit> {
    // An opacity drag in progress. Forgotten whenever the bar isn't shown, so a drag the bar
    // vanished under (a tool change, Escape) never reaches the next comment the bar opens for.
    let draft = egui::Id::unique(("comment-style-bar-opacity-draft", view.id.0));
    let Some((page, index, props, bounds)) = bar_placement(view, info, props_of, prefs, allowed && !off) else {
        ctx.data_mut(|d| d.remove::<f64>(draft));
        return None;
    };
    let takes_style = |&(p, i): &(usize, usize)| {
        (p, i) != (page, index)
            && info.annotations.iter().any(|a| a.page == p && a.index == i && !a.is_reply())
            && props_of(p, i).is_some_and(|c| c.restylable && !c.locked)
    };
    let others: Vec<(usize, usize)> = view.comments.also.iter().filter(|k| takes_style(k)).copied().collect();
    let viewport = view.viewport_rect();
    let mut edit = None;
    // Centred just above the comment; `constrain_to` keeps it inside the document area when the
    // comment touches the top.
    let area = egui::Area::new(egui::Id::unique(("comment-style-bar", view.id.0)))
        .order(egui::Order::Foreground)
        .pivot(egui::Align2::CENTER_BOTTOM)
        .constrain_to(viewport);
    area.fixed_pos(pos2(bounds.center().x, bounds.top() - 6.0)).show(ctx, |ui| {
        egui::Frame::popup(ui.style()).inner_margin(egui::Margin::symmetric(4, 2)).corner_radius(CornerRadius::same(8)).show(ui, |ui| {
            edit = style_controls(ui, page, index, &props, draft);
        });
    });
    let edit = edit?;
    let (Edit::StyleAnnotation { color, opacity, .. }, false) = (&edit, others.is_empty()) else { return Some(edit) };
    let (color, opacity) = (*color, *opacity);
    let mut edits = vec![edit];
    edits.extend(others.into_iter().map(|(page, index)| Edit::StyleAnnotation { page, index, color, opacity, width: None, endings: None }));
    Some(Edit::Batch { label: "Change comment properties".into(), edits })
}

/// Where the bar goes: the comment with the handles (page, index), its properties and its
/// screen bounds, when the bar is shown at all (see [`style_bar`]).
fn bar_placement(
    view: &DocView,
    info: &DocInfo,
    props_of: &dyn Fn(usize, usize) -> Option<CommentProps>,
    prefs: &CommentPrefs,
    allowed: bool,
) -> Option<(usize, usize, CommentProps, Rect)> {
    let cv = &view.comments;
    if !allowed || cv.composer.is_some() || cv.gesture.is_some() {
        return None;
    }
    let (page, index) = cv.selected?;
    let props = props_of(page, index).filter(|p| p.restylable && !p.locked)?;
    let a = info.annotations.iter().find(|a| a.page == page && a.index == index && !a.is_reply())?;
    let xf = view.page_xform(page)?;
    let cx = PageCx {
        page,
        xf: &xf,
        ptr: &PagePointer::default(),
        info,
        tool: QuickTool::Select,
        prefs,
        allowed,
        hidden: false,
        redact_mark: ([0.89, 0.13, 0.13], 1.0),
    };
    let bounds = cx.screen_rects(a).reduce(|u, r| u.union(r))?;
    view.viewport_rect().intersects(bounds).then_some((page, index, props, bounds))
}

/// How many of the palette's swatches the style bar shows (the rest stay in the context menu's
/// Colour submenu and in Properties).
const BAR_SWATCHES: usize = 6;

/// The style bar's controls: the first [`BAR_SWATCHES`] palette swatches, applied with one click,
/// and an opacity button opening a slider (`draft`: where its drag in progress is kept; dropped
/// while the popup is closed). The labels differ from the quick bar's ("Colour", "Blue",
/// "Opacity"), which can show at the same time. Returns the edit a choice makes.
pub(super) fn style_controls(ui: &mut egui::Ui, page: usize, index: usize, props: &CommentProps, draft: egui::Id) -> Option<Edit> {
    let mut edit = None;
    let style = |color, opacity| Edit::StyleAnnotation { page, index, color, opacity, width: None, endings: None };
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for (name, c) in SWATCHES.iter().take(BAR_SWATCHES) {
            let on = props.color.is_some_and(|cur| same_colour(&cur, c));
            let label = crate::i18n::fmt(tl!("Comment colour: {name}"), &[("name", tl!(name))]);
            if swatch(ui, *c, on, &label).clicked() {
                edit = Some(style(Some(*c), None));
            }
        }
        let resp = icons::button(ui, "blend", 32.0, false, tl!("Comment opacity"));
        let open =
            egui::Popup::menu(&resp).align(egui::RectAlign::BOTTOM_START).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
                ui.set_min_width(200.0);
                if let Some(o) = opacity_slider(ui, props.opacity, draft) {
                    edit = Some(style(None, Some(o)));
                }
            });
        if open.is_none() {
            ui.data_mut(|d| d.remove::<f64>(draft));
        }
    });
    edit
}

/// A 10–100 % slider starting at `current`, or at the drag in progress kept under `draft`. The
/// opacity to commit: on release (a click on the rail, or the end of a drag: one undo step for
/// the whole drag), or at once for an arrow key (a step of its own). A press moves the value at
/// once but commits nothing until it is released.
fn opacity_slider(ui: &mut egui::Ui, current: f64, draft: egui::Id) -> Option<f64> {
    // ponytail: no live preview while dragging; the page shows the new opacity on release. A
    // preview would need an engine edit per frame (undo spam) or a preview path in paint.
    let current = if current.is_finite() { current.clamp(0.1, 1.0) } else { 1.0 };
    let dragging = ui.data(|d| d.get_temp::<f64>(draft));
    let mut percent = dragging.unwrap_or_else(|| (current * 100.0).round());
    let slider = ui.add(egui::Slider::new(&mut percent, 10.0..=100.0).step_by(5.0).suffix(" %").text(tl!("Opacity")));
    let held = slider.is_pointer_button_down_on();
    let released = (slider.drag_stopped() || slider.clicked()) && (dragging.is_some() || slider.changed());
    let keyed = slider.changed() && !held && !slider.dragged();
    if released || keyed {
        ui.data_mut(|d| d.remove::<f64>(draft));
        return Some((percent / 100.0).clamp(0.1, 1.0));
    }
    if slider.changed() || slider.dragged() {
        ui.data_mut(|d| d.insert_temp(draft, percent));
    }
    None
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
