use super::{DocInfo, HashSet, LayerOp, PageXform, PdfCraftApp, Pos2, Rect, Vec2, pos2, vec2};

/// Whether what the pages show of the signatures differs between `before` and `after`: a verdict,
/// or whether a signature's appearance draws it.
pub(crate) fn signature_states_changed(before: &[pdfcraft_engine::SignatureInfo], after: &[pdfcraft_engine::SignatureInfo]) -> bool {
    before.len() != after.len()
        || before.iter().zip(after).any(|(b, a)| b.field != a.field || b.status != a.status || b.verdict_shown != a.verdict_shown)
}

/// The mark's side as a share of the widget's shorter side.
pub(super) const MARK_SIDE_SHARE: f32 = 0.25;

/// The largest side of the mark, in screen points, so zooming in stops enlarging it.
const MARK_SIDE_MAX: f32 = 48.0;

/// The smallest legible side of the mark, in screen points: a widget whose shorter side is under
/// `MARK_SIDE_MIN / MARK_SIDE_SHARE` (32 points) keeps only its outline. A field over that at 100%
/// can fall under it when the page is zoomed out to fit.
pub(super) const MARK_SIDE_MIN: f32 = 8.0;

/// The gap between the mark and the widget's edges, as a share of the mark's side.
const MARK_INSET_SHARE: f32 = 0.2;

/// The icon's size as a share of the mark's side; the icon is centred in it.
pub(super) const MARK_ICON_SHARE: f32 = 0.64;

/// The screen position of a widget's bottom-right corner on the page (`rect`, user space): where
/// its validation mark goes, wherever the page's /Rotate and the view turn it.
pub(super) fn mark_anchor(xf: &PageXform, info: &DocInfo, page: usize, rect: [f64; 4]) -> Pos2 {
    let [_, bottom, right, _] = rect.map(|v| v as f32);
    xf.user_rect(info, page, [right, bottom, right, bottom]).min
}

/// Where the validation mark sits in the part of a signature widget's screen rectangle that is on
/// the page (`page`, on screen too), or `None` when that part is too small for a legible one. It
/// goes in the corner nearest `anchor`, the screen position of the widget's bottom-right corner on
/// the page, so it turns with the page and the view: off the middle, where a signer's artwork
/// usually is, and clear of the state a named-layer appearance captions across its top.
pub(super) fn verdict_mark_rect(widget: Rect, page: Rect, anchor: Pos2) -> Option<Rect> {
    // A NaN or infinite corner has no place on screen.
    if !widget.is_finite() {
        return None;
    }
    // A widget past the crop box's edge shows its mark on the page, not beside it.
    let widget = widget.intersect(page);
    let side = (widget.width().min(widget.height()) * MARK_SIDE_SHARE).min(MARK_SIDE_MAX);
    // Also refuses the negative side of an inverted rectangle.
    if !(MARK_SIDE_MIN..).contains(&side) {
        return None;
    }
    let inset = side * MARK_INSET_SHARE;
    // A NaN anchor compares false either way and takes the bottom-right corner.
    let left = anchor.x - widget.min.x < widget.max.x - anchor.x;
    let top = anchor.y - widget.min.y < widget.max.y - anchor.y;
    let x = if left { widget.min.x + inset } else { widget.max.x - inset - side };
    let y = if top { widget.min.y + inset } else { widget.max.y - inset - side };
    Some(Rect::from_min_size(pos2(x, y), vec2(side, side)))
}

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
        // A signature whose appearance draws its state keeps the badge too: the document controls
        // its own layers and can clip the drawn state away or cover it with its own artwork,
        // which it cannot do to the badge.
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
/// taken from this one: a 3 pt inset, a 12 pt icon, 5 pt padding and 4 pt corners.
pub(super) const SIGN_TAB_MAX: f32 = 18.0;
/// The smallest sign tab's height, in points: a field too small for this chip gets no tab.
const SIGN_TAB_MIN: f32 = 10.0;
/// The share of its field's height on screen that a sign tab takes, between the two sizes.
const SIGN_TAB_SHARE: f32 = 0.4;
/// The smallest sign tab that carries the label; below it the tab is the icon alone.
const SIGN_TAB_LABEL_MIN: f32 = 14.0;

/// Where one sign tab is drawn on screen: its chip, the icon's square and, when the label fits,
/// the point where the label starts (its left edge, on the chip's center line), with the
/// chip's corner radius at this tab's size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SignTab {
    pub rect: Rect,
    pub icon: Rect,
    pub label: Option<Pos2>,
    pub radius: f32,
}

/// Lays out a sign tab inside the top-left corner of `field` (the widget on screen), for a label
/// `label_width` points wide (the theme's small text role, which does not scale with the chip).
/// The tab is [`SIGN_TAB_SHARE`] of the field's height, from [`SIGN_TAB_MIN`] to
/// [`SIGN_TAB_MAX`], and its inset, icon, padding and corners scale with it. It carries the label from
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
    // PLAN: sheet note 9 scales the label with the chip, but `TextRun` has no size, only a role, so the label keeps its width
    let full = pad + icon_side + pad + label_width + pad;
    let with_label = h >= SIGN_TAB_LABEL_MIN && label_width.is_finite() && label_width > 0.0 && full <= field.width() / 2.0;
    let width = if with_label { full } else { h };
    let rect = Rect::from_min_size(field.min + Vec2::splat(inset), vec2(width, h));
    let icon_x = if with_label { rect.left() + pad } else { rect.center().x - icon_side / 2.0 };
    let icon = Rect::from_min_size(pos2(icon_x, rect.center().y - icon_side / 2.0), Vec2::splat(icon_side));
    let label = with_label.then(|| pos2(icon.right() + pad, rect.center().y));
    Some(SignTab { rect, icon, label, radius: 4.0 * scale })
}

/// [`sign_tab_layout`] in the part of `field` that is on `page`: form fields take clicks on the
/// page only, so a widget that straddles the page's edge gets its tab where a click reaches it,
/// or none when that part has no room for one. `label_width` is the measured "Sign" label
/// (`page_overlays::SignTabMetrics`), so the one layout serves both the tab's part and its paint.
pub(crate) fn sign_tab_on_page(field: Rect, page: Rect, label_width: f32) -> Option<SignTab> {
    sign_tab_layout(field.intersect(page), label_width)
}

/// Run a push button's action (the ones that need no JavaScript engine).
pub(super) fn run_button(
    app: &mut PdfCraftApp,
    ctx: &egui::Context,
    index: usize,
    name: &str,
    identity: u64,
    action: pdfcraft_engine::form_scripts::ButtonAction,
) {
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
            app.views[index].session.forms.focus = None;
            app.views[index].session.pending_edit = Some(pdfcraft_engine::Edit::ResetForm { names });
        }
        B::Named(n) => match n.as_str() {
            "Print" => app.open_print(),
            // What PdfCraft's XFA buttons use, and what a script's `execMenuItem("Save")`
            // does too: the Save As dialog, so a click never overwrites the file unasked.
            "SaveAs" => app.run_command(ctx, "file.save_as"),
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
                    formatted: false,
                    read_only: None,
                    required: None,
                    display: Some(if hide { 1 } else { 0 }),
                    caption: None,
                    text_color: None,
                    fill_color: None,
                    ..Default::default()
                })
                .collect();
            if !changes.is_empty() {
                let label = if hide { "Hide a field" } else { "Show a field" };
                app.views[index].session.pending_edit =
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
        B::ImportIcon => app.choose_field_image_target(app.views[index].id, name, Some(identity)),
        B::Script(js) => {
            let id = app.views[index].id;
            app.run_button_script_target(ctx, id, name, Some(identity), &js);
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
