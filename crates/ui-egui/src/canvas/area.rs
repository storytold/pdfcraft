use super::*;

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

/// The hover popup for an annotation. Replies and Fill & Sign marks (typed text, checks, signatures)
/// show none: they are page content being edited, not comments.
pub(super) fn annotation_hover_text(a: &pdfcraft_render::Annotation) -> Option<String> {
    if a.is_reply() || a.fill_sign {
        return None;
    }
    let who = a.author.clone().unwrap_or_else(|| a.subtype.clone());
    let body = a.contents.clone().unwrap_or_default();
    Some(if body.is_empty() { who } else { format!("{who}\n{body}") })
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
