use super::{Color32, HashSet, LayerOp, PdfCraftApp, Pos2, Rect, Tokens, Vec2, icons, pos2, theme, vec2};

/// A validation badge drawn over a signature widget: the widget rectangle (user space), the verdict it shows, and its hover text.
pub(crate) struct SignatureBadge {
    pub rect: [f64; 4],
    pub status: pdfcraft_engine::SignatureStatus,
    pub summary: &'static str,
}

/// The validation badges drawn over one page's signature widgets: signed, visible widgets
/// on this page with coordinates that survive the cast to screen space. Hostile files can
/// carry NaN, infinite or gigantic rectangles, which must never reach the painter.
pub(crate) fn signature_badges(signatures: &[pdfcraft_engine::SignatureInfo], page: usize) -> Vec<SignatureBadge> {
    signatures
        .iter()
        .filter(|s| s.signed && s.visible && s.page == Some(page))
        .filter_map(|s| {
            s.rect.filter(|r| r.iter().all(|v| v.is_finite() && v.abs() < f32::MAX as f64)).map(|rect| SignatureBadge {
                rect,
                status: s.status,
                summary: s.summary(),
            })
        })
        .collect()
}

/// The empty signature fields on one page that get a sign tab, as their names and widget
/// rectangles (user space). Each is unsigned and visible, on this page, with a rectangle that
/// passes the same screen-space guard as [`signature_badges`]. A click must also be able to sign
/// it, as in `forms_ui::page_input`: the document allows filling (`can_fill`), and the form field
/// of that name is a signature field that isn't read-only, with a shown widget at that place
/// that isn't read-only on its own.
pub(crate) fn sign_tabs<'a>(
    signatures: &'a [pdfcraft_engine::SignatureInfo],
    form: &[pdfcraft_engine::FormField],
    page: usize,
    can_fill: bool,
) -> Vec<(&'a str, [f64; 4])> {
    let mut empty = signatures
        .iter()
        .filter(|s| can_fill && !s.signed && s.visible && s.page == Some(page))
        .filter_map(|s| s.rect.filter(|r| r.iter().all(|v| v.is_finite() && v.abs() < f32::MAX as f64)).map(|r| (s.field.as_str(), r)))
        .peekable();
    if empty.peek().is_none() {
        return Vec::new();
    }
    // The widgets a click signs, by field name and exact rectangle, from one pass over the form:
    // a file with thousands of fields costs no more per frame than the field hit test does.
    let signable: HashSet<(&str, [u64; 4])> = form
        .iter()
        .filter(|f| f.kind == pdfcraft_engine::FormFieldKind::Signature && !f.read_only())
        .flat_map(|f| {
            f.widgets.iter().filter(|w| w.page == Some(page) && !w.hidden && !w.read_only).map(move |w| (f.name.as_str(), w.rect.map(f64::to_bits)))
        })
        .collect();
    empty.filter(|(name, r)| signable.contains(&(*name, r.map(f64::to_bits)))).collect()
}

/// The largest sign tab's height, in points. Every other size is a share of the tab's height,
/// taken from this one: a 3 pt inset, a 12 pt icon, 5 pt padding, an 11 pt label and 4 pt corners.
pub(super) const SIGN_TAB_MAX: f32 = 18.0;
/// The smallest sign tab's height, in points: a field too small for this chip gets no tab.
const SIGN_TAB_MIN: f32 = 10.0;
/// The share of its field's height on screen that a sign tab takes, between the two sizes.
const SIGN_TAB_SHARE: f32 = 0.4;
/// The smallest sign tab that carries the label; below it the tab is the icon alone.
const SIGN_TAB_LABEL_MIN: f32 = 14.0;
/// The label's font size on the largest sign tab; it shrinks with the tab.
const SIGN_TAB_FONT: f32 = 11.0;

/// Where one sign tab is drawn on screen: its chip, the icon's square and, when the label fits,
/// the point where the label starts (its left edge, on the chip's center line), with the
/// label's font size and the chip's corner radius at this tab's size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SignTab {
    pub rect: Rect,
    pub icon: Rect,
    pub label: Option<Pos2>,
    pub font: f32,
    pub radius: f32,
}

/// Lays out a sign tab inside the top-left corner of `field` (the widget on screen), for a label
/// `label_width` points wide at [`SIGN_TAB_FONT`], its size on the largest tab. The tab is
/// [`SIGN_TAB_SHARE`] of the field's height, from [`SIGN_TAB_MIN`] to [`SIGN_TAB_MAX`], and its
/// inset, icon, padding, label and corners scale with it. It carries the label from
/// [`SIGN_TAB_LABEL_MIN`] up when the whole tab fits in half the field's width, and is a square
/// icon chip otherwise. It is left out when even that chip doesn't fit, or the field isn't finite.
pub(crate) fn sign_tab_layout(field: Rect, label_width: f32) -> Option<SignTab> {
    if !field.is_finite() {
        return None;
    }
    // Finite, so the clamp has a number to work on (and its bounds are constants in order).
    let h = (field.height() * SIGN_TAB_SHARE).clamp(SIGN_TAB_MIN, SIGN_TAB_MAX);
    let scale = h / SIGN_TAB_MAX;
    let (inset, icon_side, pad) = (3.0 * scale, 12.0 * scale, 5.0 * scale);
    if field.height() < h + 2.0 * inset || h > field.width() / 2.0 {
        return None;
    }
    let full = pad + icon_side + pad + label_width * scale + pad;
    let with_label = h >= SIGN_TAB_LABEL_MIN && label_width.is_finite() && label_width > 0.0 && full <= field.width() / 2.0;
    let width = if with_label { full } else { h };
    let rect = Rect::from_min_size(field.min + Vec2::splat(inset), vec2(width, h));
    let icon_x = if with_label { rect.left() + pad } else { rect.center().x - icon_side / 2.0 };
    let icon = Rect::from_min_size(pos2(icon_x, rect.center().y - icon_side / 2.0), Vec2::splat(icon_side));
    let label = with_label.then(|| pos2(icon.right() + pad, rect.center().y));
    Some(SignTab { rect, icon, label, font: SIGN_TAB_FONT * scale, radius: 4.0 * scale })
}

/// The sign tab's label, "Sign", laid out at `size` points.
fn sign_tab_label(ui: &egui::Ui, size: f32) -> std::sync::Arc<egui::Galley> {
    ui.fonts_mut(|f| f.layout_no_wrap(tl!("Sign").to_string(), theme::semibold(size), Color32::WHITE))
}

/// The sign tab of an empty signature field whose widget is at `field` on screen, on the page at
/// `page`, laid out for its label measured at its largest size: where it is painted and where a
/// click on it signs.
pub(super) fn sign_tab_at(ui: &egui::Ui, field: Rect, page: Rect) -> Option<SignTab> {
    sign_tab_on_page(field, page, sign_tab_label(ui, SIGN_TAB_FONT).size().x)
}

/// [`sign_tab_layout`] in the part of `field` that is on `page`: form fields take clicks on the
/// page only, so a widget that straddles the page's edge gets its tab where a click reaches it,
/// or none when that part has no room for one.
pub(crate) fn sign_tab_on_page(field: Rect, page: Rect, label_width: f32) -> Option<SignTab> {
    sign_tab_layout(field.intersect(page), label_width)
}

/// Paints a sign tab laid out by [`sign_tab_at`]: an accent chip with the signature icon and
/// "Sign" in white, as the app's primary buttons are. The label is drawn at the tab's size; text
/// scales with its size, and the padding takes up any rounding. Screen-only chrome, like the
/// validation badges.
pub(super) fn paint_sign_tab(ui: &egui::Ui, painter: &egui::Painter, tab: &SignTab, t: &Tokens) {
    painter.rect_filled(tab.rect, egui::epaint::CornerRadiusF32::same(tab.radius), t.accent);
    icons::paint(ui, tab.icon, "signature", tab.icon.width(), Color32::WHITE);
    if let Some(at) = tab.label {
        let galley = sign_tab_label(ui, tab.font);
        painter.galley(at - vec2(0.0, galley.size().y / 2.0), galley, Color32::WHITE);
    }
}

/// Suppression window for egui's eased copy of point input.
///
/// egui 0.36.2 drains `WheelState::after_events` tail deltas by 90% per 0.1 s.
pub(super) const SUPPRESS_SECS: f64 = 0.25;

/// Short rolling extension while egui still has an unprocessed wheel tail.
pub(super) const SUPPRESS_TAIL_SECS: f64 = 0.05;

// Keep the viewer's scroll identity tied to the document instead of its parent UI.
pub(super) fn page_scroll<R>(
    ui: &mut egui::Ui,
    scroll: egui::ScrollArea,
    id: egui::Id,
    draw: impl FnOnce(&mut egui::Ui, egui::Rect) -> R,
) -> egui::scroll_area::ScrollAreaOutput<R> {
    ui.scope_builder(egui::UiBuilder::new().scope_id(id), |ui| scroll.show_viewport(ui, draw)).inner
}

/// Run a push button's action (the ones that need no JavaScript engine).
pub(super) fn run_button(app: &mut PdfCraftApp, index: usize, name: &str, action: pdfcraft_engine::form_scripts::ButtonAction) {
    use pdfcraft_engine::form_scripts::ButtonAction as B;
    let pages = app.session.get(app.views[index].id).map_or(0, |d| d.info.pages.len());
    match action {
        B::Reset { fields, exclude } => {
            let all: Vec<String> = app.session.get(app.views[index].id).map(|d| d.form.iter().map(|f| f.name.clone()).collect()).unwrap_or_default();
            let listed = |n: &String| fields.iter().any(|f| n == f || n.starts_with(&format!("{f}.")));
            let names = match (fields.is_empty(), exclude) {
                (true, _) => None,
                (false, false) => Some(all.iter().filter(|n| listed(n)).cloned().collect()),
                (false, true) => Some(all.iter().filter(|n| !listed(n)).cloned().collect()),
            };
            app.views[index].forms.focus = None;
            app.views[index].pending_edit = Some(pdfcraft_engine::Edit::ResetForm { names });
        }
        B::Named(n) => match n.as_str() {
            "Print" => app.open_print(),
            // What PdfCraft's XFA buttons use, and what a script's `execMenuItem("Save")`
            // does too: the Save As dialog, so a click never overwrites the file unasked.
            "SaveAs" => app.run_command("file.save_as"),
            "NextPage" => app.views[index].step_page(true),
            "PrevPage" => app.views[index].step_page(false),
            "FirstPage" => app.views[index].go_to_page(0),
            "LastPage" => app.views[index].go_to_page(pages.saturating_sub(1)),
            other => app.notify_fmt("{name}: the {other} action isn't supported yet", &[("name", name), ("other", other)]),
        },
        B::Uri(u) => app.request_document_url(&u, crate::LinkOrigin::Button),
        B::GoTo(p) => app.views[index].go_to_page(p.min(pages.saturating_sub(1))),
        B::ShowHide { fields, hide } => {
            let listed = |n: &String| fields.iter().any(|f| n == f || n.starts_with(&format!("{f}.")));
            // `display.hidden` 1, `display.visible` 0, as a script setting `field.display` would.
            let changes: Vec<pdfcraft_engine::FieldChange> = app
                .session
                .get(app.views[index].id)
                .map(|d| d.form.iter().filter(|f| listed(&f.name)).map(|f| f.name.clone()).collect::<Vec<_>>())
                .unwrap_or_default()
                .into_iter()
                .map(|name| pdfcraft_engine::FieldChange {
                    name,
                    value: None,
                    read_only: None,
                    required: None,
                    display: Some(if hide { 1 } else { 0 }),
                })
                .collect();
            if !changes.is_empty() {
                let label = if hide { "Hide a field" } else { "Show a field" };
                app.views[index].pending_edit =
                    Some(pdfcraft_engine::Edit::Batch { label: label.into(), edits: vec![pdfcraft_engine::Edit::ApplyScriptChanges { changes }] });
            }
        }
        B::SetLayers { changes, preserve_rb } => {
            use pdfcraft_engine::form_scripts::LayerOp as Op;
            let changes: Vec<(LayerOp, (u32, u16))> = changes
                .into_iter()
                .map(|(op, ocg)| {
                    let op = match op {
                        Op::On => LayerOp::On,
                        Op::Off => LayerOp::Off,
                        Op::Toggle => LayerOp::Toggle,
                    };
                    (op, ocg)
                })
                .collect();
            set_layers(app, index, &changes, preserve_rb);
        }
        B::Alert(m) => app.notify(m),
        B::Submit(url) => app.notify_fmt(
            "{name} submits the form to {url}; PdfCraft doesn't send form data. Save the document to keep your entries.",
            &[("name", name), ("url", &url)],
        ),
        B::ImportIcon => app.choose_field_image(name),
        B::Script(js) => {
            let id = app.views[index].id;
            app.run_button_script(id, name, &js);
        }
    }
}

/// Run a set-layer-visibility action from a button or link. It changes what is shown, as the
/// Layers panel does (which follows), not the document.
pub(super) fn set_layers(app: &mut PdfCraftApp, index: usize, changes: &[(LayerOp, (u32, u16))], preserve_rb: bool) {
    let id = app.views[index].id;
    if app.session.set_layer_state(id, changes, preserve_rb) {
        app.views[index].invalidate_content();
    }
}

/// Where a pinch or ⌘-scroll zooms the view about: the pointer, wherever it is over the view. Not
/// page input: it is read before the pages' widget exists, once `rect_contains_pointer` has found
/// the view under the pointer.
pub(super) fn zoom_anchor(ui: &egui::Ui) -> Option<Pos2> {
    ui.input(|i| i.pointer.hover_pos())
}

/// The pointer and frame time that scroll the view while a comment is drawn. Not page input: it is
/// read before the pages' widget exists, and the pointer may be past the view's edge.
pub(super) fn drawing_edge_pointer(ui: &egui::Ui) -> (Option<Pos2>, f32) {
    ui.input(|i| (i.pointer.latest_pos(), i.stable_dt))
}

/// A press anywhere, or a key, stops the touchpad's glide (#531).
pub(super) fn glide_interrupted(ui: &egui::Ui) -> bool {
    ui.input(|i| i.pointer.any_pressed() || i.events.iter().any(|e| matches!(e, egui::Event::Key { pressed: true, .. })))
}

/// Full screen's click on the document, which turns the page: a primary click this frame with the
/// view on top under it in `area`. Not page input: it is read before the pages' widget exists, and
/// the turn waits until the pages have had the click.
pub(super) fn presentation_click(ui: &egui::Ui, area: Rect) -> bool {
    ui.rect_contains_pointer(area) && ui.input(|i| i.pointer.primary_clicked())
}
