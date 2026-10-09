//! Edit a PDF ▸ the floating format bar: one row of formatting over the document, beside what
//! it formats. It serves the Text tool (the style new text gets), a box of text being typed, a
//! selected added text item, a paragraph being edited in place, and a selected added image.
//! It floats instead of opening above the tool list, so the list never moves. Its grip drags it
//! anywhere (it then stays put); a double-click on the grip docks it again.

use egui::{Color32, CornerRadius, Pos2, Rect, Sense, Stroke, vec2};
use pdfcraft_engine::{AddedContent, AddedImage, AddedText, Edit, FontFamily, TextAlign};

use crate::theme::Tokens;
use crate::{LeftPanel, Mode, PdfCraftApp, QuickTool};

/// What the bar formats, by priority.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Target {
    /// An existing paragraph open for editing in place.
    Paragraph,
    /// A box of text being typed (new, or retyped added text).
    Draft,
    /// A selected added text item: (page, index among the page's items).
    Text(usize, usize),
    /// A selected added image.
    Image(usize, usize),
    /// The Text tool, before a box is opened: the style new text gets.
    NewText,
}

fn rect_id() -> egui::Id {
    egui::Id::new("format-bar-rect")
}

fn detached_id() -> egui::Id {
    egui::Id::new("format-bar-detached")
}

/// The added item `index` among page `page`'s items.
fn added_item(app: &PdfCraftApp, page: usize, index: usize) -> Option<&pdfcraft_engine::Added> {
    let (_, id) = app.active_ids()?;
    app.session.get(id)?.added.iter().filter(|a| a.page == page).nth(index)
}

fn target(app: &PdfCraftApp) -> Option<Target> {
    // Read mode hides the tool panel, and the bar with it.
    let editing = (app.left_open && app.left == LeftPanel::Tool("edit")) || matches!(app.quick_tool, QuickTool::AddText | QuickTool::EditText);
    if !editing || app.mode == Mode::Read {
        return None;
    }
    let (i, _) = app.active_ids()?;
    let view = app.views.get(i)?;
    if view.line_editor.is_some() {
        return Some(Target::Paragraph);
    }
    if view.content.draft.is_some() {
        return Some(Target::Draft);
    }
    if let Some((page, index)) = view.content.selected {
        match added_item(app, page, index).map(|a| &a.content) {
            Some(AddedContent::Text(_)) => return Some(Target::Text(page, index)),
            Some(AddedContent::Image(_)) => return Some(Target::Image(page, index)),
            _ => {}
        }
    }
    (app.quick_tool == QuickTool::AddText).then_some(Target::NewText)
}

/// The tool-panel command whose row owns what is being formatted, or the picked tool, so the
/// panel can mark it.
pub(crate) fn active_command(app: &PdfCraftApp) -> Option<&'static str> {
    match (target(app), app.quick_tool) {
        (Some(Target::Paragraph), _) | (_, QuickTool::EditText) => Some("edit.edit_text"),
        (Some(Target::Draft | Target::Text(..) | Target::NewText), _) => Some("edit.text"),
        (Some(Target::Image(..)), _) => Some("edit.image"),
        (None, QuickTool::Field(f)) => Some(f.command()),
        _ => None,
    }
}

/// Whether a press at `p` is on the bar or in one of its open popups: the in-place editors
/// don't take it for a click away.
pub(crate) fn holds(ctx: &egui::Context, p: Pos2) -> bool {
    let Some(r) = ctx.data(|d| d.get_temp::<Rect>(rect_id())) else { return false };
    r.expand(2.0).contains(p) || egui::Popup::is_any_open(ctx)
}

/// Where the bar goes: where it was dragged, else just above its target (below when there's
/// no room), else centred at the top of the document area. Kept inside the document area.
fn place(view_rect: Rect, anchor: Option<Rect>, detached: Option<Pos2>, size: egui::Vec2) -> Pos2 {
    const GAP: f32 = 8.0;
    let at = match (detached, anchor) {
        (Some(p), _) => p,
        (None, Some(a)) => {
            let above = a.top() - GAP - size.y;
            let y = if above < view_rect.top() + 4.0 { a.bottom() + GAP } else { above };
            Pos2::new(a.center().x - size.x / 2.0, y)
        }
        (None, None) => Pos2::new(view_rect.center().x - size.x / 2.0, view_rect.top() + 10.0),
    };
    // `max` before `min`: a document area narrower than the bar pins it to the left/top edge.
    let x = at.x.min(view_rect.right() - size.x - 4.0).max(view_rect.left() + 4.0);
    let y = at.y.min(view_rect.bottom() - size.y - 4.0).max(view_rect.top() + 4.0);
    Pos2::new(x, y)
}

/// What the bar's controls changed this frame.
#[derive(Default)]
struct Changes {
    style: Option<AddedText>,
    /// Underline or spacing (a paragraph's extras).
    extras: bool,
    /// A button or list was used: give the keyboard back to the text being typed.
    refocus: bool,
    image: Option<ImageAction>,
}

enum ImageAction {
    Update(AddedContent),
    Replace,
}

/// Show the bar for whatever is being formatted, and apply what it changes.
pub(crate) fn show(app: &mut PdfCraftApp, ctx: &egui::Context) {
    if !bar(app, ctx) {
        // Hidden: no stale area for the editors to keep clicks on.
        ctx.data_mut(|d| d.remove::<Rect>(rect_id()));
    }
}

/// The bar, when there is something to format; `false` when there isn't.
fn bar(app: &mut PdfCraftApp, ctx: &egui::Context) -> bool {
    let Some(target) = target(app) else { return false };
    let Some((i, _)) = app.active_ids() else { return false };
    let Some(view) = app.views.get(i) else { return false };
    let Some(doc) = app.session.get(view.id) else { return false };
    let info = &doc.info;
    let view_rect = view.viewport_rect();
    // The target on screen, and the values the controls start from.
    let screen = |page: usize, r: [f64; 4]| {
        info.pages.get(page)?;
        view.page_xform(page).map(|xf| crate::content_ui::screen_rect(&xf, info, page, r))
    };
    let (anchor, style, image) = match target {
        Target::Paragraph => (view.line_editor.as_ref().map(|e| e.rect), view.line_editor.as_ref().map(|e| e.look.clone()), None),
        Target::Draft => {
            (view.content.draft.as_ref().and_then(|d| screen(d.page, d.rect)), view.content.draft.as_ref().map(|d| d.style.clone()), None)
        }
        Target::Text(page, index) => match added_item(app, page, index).map(|a| &a.content) {
            Some(AddedContent::Text(t)) => (screen(page, t.rect), Some(t.clone()), None),
            _ => return false,
        },
        Target::Image(page, index) => match added_item(app, page, index).map(|a| &a.content) {
            Some(AddedContent::Image(img)) => (screen(page, img.rect), None, Some(img.clone())),
            _ => return false,
        },
        Target::NewText => (None, Some(app.text_style.clone()), None),
    };
    let mut extras = view.line_editor.as_ref().map(|e| e.extras);
    let last = ctx.data(|d| d.get_temp::<Rect>(rect_id()));
    let size = last.map_or(vec2(420.0, 36.0), |r| r.size());
    let mut detached = ctx.data(|d| d.get_temp::<Pos2>(detached_id()));
    let pos = place(view_rect, anchor, detached, size);
    let t = Tokens::get(ctx);
    let mut changes = Changes::default();
    let shown = egui::Area::new(egui::Id::new("format-bar")).order(egui::Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
        egui::Frame::NONE
            .fill(t.card)
            .stroke(Stroke::new(1.0, t.border))
            .corner_radius(CornerRadius::same(10))
            .shadow(egui::Shadow { offset: [0, 2], blur: 10, spread: 0, color: Color32::from_black_alpha(if t.dark() { 80 } else { 22 }) })
            .inner_margin(egui::Margin::same(4))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = vec2(2.0, 2.0);
                ui.horizontal(|ui| {
                    // The grip: drag to move the bar; double-click to dock it again.
                    let (r, grip) = ui.allocate_exact_size(vec2(12.0, 28.0), Sense::click_and_drag());
                    grip.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, tl!("Move the format bar")));
                    for (dx, dy) in [(-2.5, -5.0), (2.5, -5.0), (-2.5, 0.0), (2.5, 0.0), (-2.5, 5.0), (2.5, 5.0)] {
                        ui.painter().circle_filled(r.center() + vec2(dx, dy), 1.3, t.text_faint);
                    }
                    if grip.dragged() {
                        detached = Some(detached.unwrap_or(pos) + grip.drag_delta());
                    }
                    // A triple too: egui counts a quick click elsewhere just before as the first.
                    if grip.double_clicked() || grip.triple_clicked() {
                        detached = None;
                    }
                    let grip = grip.on_hover_text(tl!("Drag to move; double-click to put it back"));
                    if grip.dragged() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                    } else if grip.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
                    }
                    match (&style, &image) {
                        (Some(s), _) => text_controls(ui, &t, s, extras.as_mut(), &mut changes),
                        (None, Some(img)) => changes.image = image_controls(ui, img),
                        (None, None) => {}
                    }
                });
            });
    });
    ctx.data_mut(|d| {
        d.insert_temp(rect_id(), shown.response.rect);
        match detached {
            Some(p) => {
                d.insert_temp(detached_id(), p);
            }
            None => d.remove::<Pos2>(detached_id()),
        }
    });
    apply(app, i, target, changes, extras);
    true
}

/// Font, size, bold, italic, (underline), alignment, colour, (spacing) and a hint, in one row.
fn text_controls(ui: &mut egui::Ui, t: &Tokens, style: &AddedText, extras: Option<&mut crate::edit_text_ui::Extras>, out: &mut Changes) {
    let mut s = style.clone();
    egui::ComboBox::from_id_salt("font-family").selected_text(s.family.label()).width(96.0).show_ui(ui, |ui| {
        for f in [FontFamily::Helvetica, FontFamily::Times, FontFamily::Courier] {
            ui.selectable_value(&mut s.family, f, f.label());
        }
    });
    // A list rather than a drag value: every change is an undo step.
    egui::ComboBox::from_id_salt("font-size").selected_text(format!("{} pt", s.size)).width(70.0).show_ui(ui, |ui| {
        for size in [8.0, 9.0, 10.0, 11.0, 12.0, 14.0, 16.0, 18.0, 20.0, 24.0, 28.0, 36.0, 48.0, 72.0] {
            ui.selectable_value(&mut s.size, size, format!("{size} pt"));
        }
    });
    ui.separator();
    if crate::icons::button(ui, "bold", 26.0, s.bold, tl!("Bold")).clicked() {
        s.bold = !s.bold;
    }
    if crate::icons::button(ui, "italic", 26.0, s.italic, tl!("Italic")).clicked() {
        s.italic = !s.italic;
    }
    let mut extras = extras;
    if let Some(e) = extras.as_deref_mut()
        && crate::icons::button(ui, "underline", 26.0, e.underline, tl!("Underline")).clicked()
    {
        e.underline = !e.underline;
        out.extras = true;
        out.refocus = true;
    }
    ui.separator();
    for (a, icon, tip) in [
        (TextAlign::Left, "align-left", tl!("Align left")),
        (TextAlign::Center, "align-center", tl!("Centre")),
        (TextAlign::Right, "align-right", tl!("Align right")),
        (TextAlign::Justify, "align-justify", tl!("Justify")),
    ] {
        if crate::icons::button(ui, icon, 26.0, s.align == a, tip).clicked() {
            s.align = a;
        }
    }
    ui.separator();
    // Colour: a swatch that opens the palette.
    let (r, swatch) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::click());
    swatch.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, tl!("Colour")));
    if swatch.hovered() {
        ui.painter().rect_filled(r, CornerRadius::same(6), t.hover);
    }
    ui.painter().circle(r.center(), 8.0, crate::edit_text_ui::color32(s.color), Stroke::new(1.0, t.border));
    let swatch = swatch.on_hover_text(tl!("Colour"));
    egui::Popup::menu(&swatch).gap(6.0).show(|ui| {
        if let Some(picked) = crate::comments::swatch_grid(ui, Some(s.color)) {
            s.color = picked;
            ui.close();
        }
    });
    // A paragraph's spacing and scale, in a popover that stays open while they're adjusted.
    if let Some(e) = extras {
        let more = crate::icons::button(ui, "ellipsis", 26.0, false, tl!("More options"));
        egui::Popup::menu(&more).gap(6.0).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
            out.extras |= crate::edit_text_ui::extras_panel(ui, e);
        });
    }
    let tip = format!(
        "{} {}",
        tl!(
            "Click on the page to add text; each click starts a new box, and ✓ finishes. Drag items to move them, drag a corner to resize, double-click text to edit it."
        ),
        tl!("Standard fonts; text outside Windows-1252 isn't supported yet.")
    );
    crate::icons::button(ui, "info", 26.0, false, &tip);
    out.refocus |= s != *style;
    out.style = (s != *style).then_some(s);
}

/// Rotate, flip, replace and crop a selected added image.
fn image_controls(ui: &mut egui::Ui, img: &AddedImage) -> Option<ImageAction> {
    let mut out = None;
    let r = img.rect;
    let turn = |k: u8| {
        let mut i = img.clone();
        i.rotation = (i.rotation + k) % 4;
        // The box turns with the picture, around its centre.
        let (cx, cy, w, h) = ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0, r[2] - r[0], r[3] - r[1]);
        i.rect = [cx - h / 2.0, cy - w / 2.0, cx + h / 2.0, cy + w / 2.0];
        i
    };
    if crate::icons::button(ui, "rotate-ccw", 28.0, false, tl!("Rotate counterclockwise")).clicked() {
        out = Some(ImageAction::Update(AddedContent::Image(turn(1))));
    }
    if crate::icons::button(ui, "rotate-cw", 28.0, false, tl!("Rotate clockwise")).clicked() {
        out = Some(ImageAction::Update(AddedContent::Image(turn(3))));
    }
    if crate::icons::button(ui, "flip-horizontal-2", 28.0, false, tl!("Flip horizontal")).clicked() {
        out = Some(ImageAction::Update(AddedContent::Image(AddedImage { flip_h: !img.flip_h, ..img.clone() })));
    }
    if crate::icons::button(ui, "flip-vertical-2", 28.0, false, tl!("Flip vertical")).clicked() {
        out = Some(ImageAction::Update(AddedContent::Image(AddedImage { flip_v: !img.flip_v, ..img.clone() })));
    }
    if crate::icons::button(ui, "replace", 28.0, false, tl!("Replace image")).clicked() {
        out = Some(ImageAction::Replace);
    }
    ui.separator();
    // Crop (% trimmed from each side), in a popover that stays open while it's adjusted.
    let crop_button = crate::icons::button(ui, "crop", 28.0, img.crop != [0.0; 4], tl!("Crop (% trimmed from each side)"));
    egui::Popup::menu(&crop_button).gap(6.0).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        ui.label(egui::RichText::new(tl!("Crop (% trimmed from each side)")).small());
        let mut crop = img.crop.map(|v| (v * 100.0).round());
        let mut changed = false;
        ui.horizontal(|ui| {
            for (k, label) in ["L", "B", "R", "T"].into_iter().enumerate() {
                ui.label(label);
                // Applied when the drag ends, so a drag is one undo step.
                let r = ui.add(egui::DragValue::new(&mut crop[k]).range(0.0..=45.0).speed(0.5).suffix("%"));
                changed |= r.drag_stopped() || (r.changed() && !r.dragged());
            }
        });
        let crop = crop.map(|v| v / 100.0);
        if changed && crop != img.crop {
            out = Some(ImageAction::Update(AddedContent::Image(AddedImage { crop, ..img.clone() })));
        }
    });
    out
}

/// Apply the bar's changes to what it formats. A box being typed or a paragraph being edited
/// gets the keyboard back after a change, so typing carries on.
fn apply(app: &mut PdfCraftApp, i: usize, target: Target, changes: Changes, extras: Option<crate::edit_text_ui::Extras>) {
    // New text follows the latest style picked for text.
    let remember = |app: &mut PdfCraftApp, s: &AddedText| app.text_style = AddedText { text: String::new(), rect: [0.0; 4], ..s.clone() };
    match target {
        Target::Paragraph => {
            let Some(mut ed) = app.views.get(i).and_then(|v| v.line_editor.clone()) else { return };
            if changes.style.is_none() && !changes.extras {
                return;
            }
            if let Some(s) = changes.style {
                ed.look = s;
            }
            if changes.refocus {
                ed.refocus();
            }
            if let Some(e) = extras {
                ed.extras = e;
            }
            let edit = Edit::EditTextBlock { page: ed.page, block: ed.block, text: ed.text.clone(), style: ed.style() };
            if app.apply_edit(edit) {
                ed.applied();
                if let Some(doc) = app.views.get(i).and_then(|v| app.session.get(v.id))
                    && let Some(block) = doc.text_blocks(ed.page).get(ed.block)
                {
                    ed.refresh_source(block);
                }
            }
            if let Some(v) = app.views.get_mut(i) {
                v.line_editor = Some(ed);
            }
        }
        Target::Draft => {
            let Some(s) = changes.style else { return };
            remember(app, &s);
            if let Some(d) = app.views.get_mut(i).and_then(|v| v.content.draft.as_mut()) {
                d.style = AddedText { text: String::new(), ..s };
                d.focus = true;
            }
        }
        Target::Text(page, index) => {
            let Some(s) = changes.style else { return };
            remember(app, &s);
            app.apply_edit(Edit::UpdateContent { page, index, content: AddedContent::Text(s) });
        }
        Target::Image(page, index) => match changes.image {
            Some(ImageAction::Update(content)) => {
                app.apply_edit(Edit::UpdateContent { page, index, content });
            }
            Some(ImageAction::Replace) => app.replace_image_dialog(page, index),
            None => {}
        },
        Target::NewText => {
            if let Some(s) = changes.style {
                app.text_style = s;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> Rect {
        Rect::from_min_size(Pos2::new(300.0, 100.0), vec2(1000.0, 700.0))
    }

    #[test]
    fn the_bar_sits_above_its_target_or_below_without_room() {
        let size = vec2(400.0, 36.0);
        let a = Rect::from_min_size(Pos2::new(600.0, 400.0), vec2(200.0, 30.0));
        assert_eq!(place(view(), Some(a), None, size), Pos2::new(500.0, 356.0));
        let near_top = a.translate(vec2(0.0, -290.0));
        assert_eq!(place(view(), Some(near_top), None, size).y, near_top.bottom() + 8.0);
    }

    #[test]
    fn the_bar_stays_inside_the_document_area() {
        let size = vec2(400.0, 36.0);
        let left = Rect::from_min_size(Pos2::new(310.0, 400.0), vec2(40.0, 20.0));
        assert_eq!(place(view(), Some(left), None, size).x, 304.0);
        assert_eq!(place(view(), None, Some(Pos2::new(5000.0, -50.0)), size), Pos2::new(896.0, 104.0));
        // Narrower than the bar: pinned to the left edge, never NaN or past the left.
        let narrow = Rect::from_min_size(Pos2::new(300.0, 100.0), vec2(100.0, 20.0));
        assert_eq!(place(narrow, None, None, size), Pos2::new(304.0, 104.0));
        // Nothing to follow: centred at the top.
        assert_eq!(place(view(), None, None, size), Pos2::new(600.0, 110.0));
    }
}
