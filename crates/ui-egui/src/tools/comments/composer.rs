use super::geometry::{drawn_text_box_rect, page_rotation, text_box_rect};
use super::paint::{edited_in_view, new_comment};
use super::state::{ComposerKind, PageCx};
use super::*;

/// The floating composer card (new note, new text box, edit). Returns the edit to apply.
pub(crate) fn composer(ctx: &egui::Context, view: &mut DocView, info: &DocInfo, prefs: &CommentPrefs) -> Option<Edit> {
    let c = view.comments.composer.as_ref()?;
    let page = c.page;
    let Some(xf) = view.page_xform(page) else {
        // Scrolled away: keep the draft until the page is back, and give it the keyboard then.
        if let Some(c) = view.comments.composer.as_mut() {
            c.focus = true;
        }
        return None;
    };
    let cx = PageCx {
        page,
        xf: &xf,
        ptr: &PagePointer::default(),
        info,
        tool: QuickTool::Select,
        prefs,
        allowed: true,
        hidden: false,
        redact_mark: ([0.89, 0.13, 0.13], 1.0),
    };
    let anchor = cx.to_screen(c.at);
    let viewport = view.viewport_rect();
    let t = Tokens::get(ctx);
    let mut post = false;
    let mut cancel = false;
    let c = view.comments.composer.as_mut()?;
    let title = match c.kind {
        ComposerKind::Note => "Sticky note",
        ComposerKind::TextBox { .. } => "Text box",
        ComposerKind::Callout { .. } => "Callout",
        ComposerKind::Caret { .. } => "Inserted text",
        ComposerKind::Replace => "Replacement text",
        ComposerKind::Edit(_) => "Edit comment",
    };
    // Its comment scrolled out of the document: keep the draft until it is back, rather than
    // pushing the card into the window over the mode bar and toolbar (#4021). An edited comment
    // that is only partly scrolled away keeps its card, at the nearest point inside the document.
    let mut anchor = anchor;
    if !viewport.contains(anchor) {
        let part_visible = matches!(c.kind, ComposerKind::Edit(index) if edited_in_view(&cx, index, viewport));
        if !part_visible {
            c.focus = true;
            return None;
        }
        anchor = anchor.clamp(viewport.min, viewport.max);
    }
    let pos = pos2(anchor.x + 12.0, anchor.y);
    let area = egui::Area::new(egui::Id::unique(("comment-composer", view.id.0))).order(egui::Order::Foreground).constrain_to(viewport);
    area.fixed_pos(pos).show(ctx, |ui| {
        egui::Frame::popup(ui.style()).inner_margin(egui::Margin::same(12)).corner_radius(CornerRadius::same(8)).show(ui, |ui| {
            ui.set_width(260.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(prefs.author.clone()).font(theme::semibold(13.0)));
                ui.label(egui::RichText::new(tl!(title)).font(theme::regular(11.5)).color(t.text_faint));
            });
            ui.add_space(6.0);
            let hint = match c.kind {
                ComposerKind::TextBox { .. } | ComposerKind::Callout { .. } => tl!("Type text"),
                ComposerKind::Caret { .. } => tl!("Text to insert"),
                ComposerKind::Replace => tl!("Replacement text"),
                _ => tl!("Add a comment"),
            };
            let edit = ui
                .add(egui::TextEdit::multiline(&mut c.text).hint_text(hint).desired_rows(3).desired_width(f32::INFINITY).id_salt("composer-text"))
                // 0.37 kittest rejects an unnamed text input; the hint is its logical name.
                .accessible_name(hint);
            if c.focus {
                edit.request_focus();
                c.focus = false;
            }
            let enter = submit_key(ui, &edit, false);
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                cancel = true;
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let can_post = !c.text.trim().is_empty() || matches!(c.kind, ComposerKind::Edit(_));
                    if ui
                        .add_enabled(
                            can_post,
                            egui::Button::new(egui::RichText::new(tl!("Post")).color(Color32::WHITE)).fill(t.accent).corner_radius(14),
                        )
                        .clicked()
                        || (enter && can_post)
                    {
                        post = true;
                    }
                    if ui.add(egui::Button::new(tl!("Cancel")).corner_radius(14)).clicked() {
                        cancel = true;
                    }
                });
            });
        });
    });
    if cancel {
        view.comments.composer = None;
        return None;
    }
    if !post {
        return None;
    }
    let c = view.comments.composer.take()?;
    let text = c.text.trim_end().to_string();
    match c.kind {
        ComposerKind::Edit(index) => Some(Edit::SetAnnotationContents { page, index, text }),
        ComposerKind::Note => {
            view.comments.tool_done = true;
            Some(new_comment(&cx, CommentTool::Note, Shape::Note { at: c.at, icon: NoteIcon::Comment }, text))
        }
        ComposerKind::TextBox { size } => {
            view.comments.tool_done = true;
            let rotation = page_rotation(info, page);
            let border = prefs.style(CommentTool::TextBox).width;
            let rect = match (size, info.pages.get(page)) {
                (Some(size), Some(p)) => drawn_text_box_rect(p, c.at, size, &text, 12.0, border),
                _ => text_box_rect(c.at, &text, 12.0, rotation),
            };
            Some(new_comment(&cx, CommentTool::TextBox, Shape::TextBox { rect, font_size: 12.0 }, text))
        }
        ComposerKind::Callout { point } => {
            view.comments.tool_done = true;
            // A callout's leader and box stay in user-space axes (its appearance isn't turned).
            let rect = text_box_rect(c.at, &text, 10.0, 0);
            let side = if point[0] < rect[0] {
                rect[0]
            } else if point[0] > rect[2] {
                rect[2]
            } else {
                (rect[0] + rect[2]) / 2.0
            };
            let knee = [(point[0] + side) / 2.0, (rect[1] + rect[3]) / 2.0];
            Some(new_comment(&cx, CommentTool::Callout, Shape::Callout { rect, knee, point, font_size: 10.0, ending: LineEnding::OpenArrow }, text))
        }
        ComposerKind::Replace => {
            view.comments.tool_done = true;
            let quads = view.comments.replace.take()?;
            Some(Edit::ReplaceText {
                page,
                quads,
                text,
                author: prefs.author.clone(),
                strike: prefs.style(CommentTool::ReplaceText),
                caret: prefs.style(CommentTool::Caret),
            })
        }
        ComposerKind::Caret { size } => {
            view.comments.tool_done = true;
            let [x, y] = c.at;
            let half = size / 2.0;
            Some(new_comment(&cx, CommentTool::Caret, Shape::Caret { rect: [x - half, y - size, x + half, y] }, text))
        }
    }
}

/// Submit only from the focused comment editor. Match each key event's modifiers exactly,
/// so Shift+Enter stays a newline and another editor cannot submit this draft. Replies also
/// keep their existing unmodified Enter-to-post behavior.
pub(crate) fn submit_key(ui: &mut egui::Ui, field: &egui::Response, plain_enter: bool) -> bool {
    field.has_focus()
        && ui.input_mut(|input| {
            let mut submit = false;
            input.events.retain(|event| {
                let matched = matches!(event,
                    egui::Event::Key { key: egui::Key::Enter, pressed: true, modifiers, .. }
                        if modifiers.matches_exact(egui::Modifiers::CTRL)
                            || modifiers.matches_exact(egui::Modifiers::COMMAND)
                            || plain_enter && modifiers.matches_exact(egui::Modifiers::NONE)
                );
                submit |= matched;
                !matched
            });
            submit
        })
}
