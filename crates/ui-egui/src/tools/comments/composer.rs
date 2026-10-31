use super::geometry::{drawn_text_box_rect, page_rotation, text_box_rect};
use super::paint::{edited_in_view, new_comment};
use super::state::{ComposerKind, InlineText, PageCx};
use super::{
    Color32, CommentPrefs, CommentTool, CornerRadius, DocInfo, DocView, Edit, LineEnding, NewAnnotation, NoteIcon, PagePointer, QuickTool, Rect,
    SELECT_BLUE, Shape, Stroke, Tokens, color32, pos2, theme, vec2,
};

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
                _ => text_box_rect(c.at, &text, 12.0, border, rotation),
            };
            Some(new_comment(&cx, CommentTool::TextBox, Shape::TextBox { rect, font_size: 12.0 }, text))
        }
        ComposerKind::Callout { point } => {
            view.comments.tool_done = true;
            // A callout's leader and box stay in user-space axes (its appearance isn't turned).
            let rect = text_box_rect(c.at, &text, 10.0, prefs.style(CommentTool::Callout).width, 0);
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

/// What keeping the text typed into a text box on the page does: the new box (sized to its text,
/// or to the box dragged out, as the composer makes one), or the changed text (the engine fits
/// the box to it). `None` when nothing changed, or a new box has no text.
pub(crate) fn inline_edit(it: &InlineText, info: &DocInfo, prefs: &CommentPrefs) -> Option<Edit> {
    if !it.has_changes() {
        return None;
    }
    let text = it.text.trim_end().to_string();
    match it.index {
        Some(_) => Some(Edit::SetAnnotationContents { page: it.page, index: it.target(info)?.index, text }),
        None => {
            let style = prefs.style(CommentTool::TextBox);
            let rect = match (it.size, info.pages.get(it.page)) {
                (Some(size), Some(p)) => drawn_text_box_rect(p, it.at, size, &text, 12.0, style.width),
                _ => text_box_rect(it.at, &text, 12.0, style.width, 0),
            };
            Some(Edit::AddAnnotation(NewAnnotation {
                page: it.page,
                shape: Shape::TextBox { rect, font_size: 12.0 },
                style,
                contents: text,
                author: prefs.author.clone(),
            }))
        }
    }
}

/// The text box being typed into on the page (see [`InlineText`]): an editor over the box in its
/// font size, colour and fill at the page's zoom, which grows with the text as the box will and
/// covers the box's old drawing while it is open. Returns the edit once the text is kept.
pub(crate) fn inline_text(
    ctx: &egui::Context,
    view: &mut DocView,
    info: &DocInfo,
    doc: &pdfcraft_engine::Document,
    prefs: &CommentPrefs,
) -> Option<Edit> {
    let it = view.comments.inline.clone()?;
    // Its page has gone (an edit removed it): so has the text typed on it.
    let Some(page) = info.pages.get(it.page) else {
        view.comments.inline = None;
        return None;
    };
    // Scrolled away: the text waits until the page is back. egui drops the focus of a widget
    // that isn't drawn, so the editor asks for the keyboard again when it is drawn again (or a
    // click away could never keep the text, and new text boxes would wait for it).
    let Some(xf) = view.page_xform(it.page) else {
        if let Some(t) = view.comments.inline.as_mut() {
            t.focus = true;
        }
        return None;
    };
    let zoom = xf.rect.width() / xf.pw.max(1.0);
    // The box's look: its own, or the text box tool's for a new one.
    let look = match it.index {
        Some(_) => {
            // The comment where the page lists it now; the editor follows it there.
            let target = it.target(info).and_then(|a| Some((a, doc.comment_props(it.page, a.index)?)));
            if let (Some((a, _)), Some(t)) = (target.as_ref(), view.comments.inline.as_mut()) {
                t.index = Some(a.index);
            }
            target.map(|(a, p)| (p.color.unwrap_or([0.0; 3]), p.font_size.unwrap_or(12.0), p.fill, Some(xf.user_rect(info, it.page, a.rect))))
        }
        None => {
            let style = prefs.style(CommentTool::TextBox);
            // A box dragged out shows at its drawn size, and its text wraps inside it.
            let drawn = it.size.map(|[w, h]| {
                let v = page.user_to_view(it.at[0] as f32, it.at[1] as f32);
                Rect::from_min_size(xf.norm_to_screen(v[0] / xf.pw, v[1] / xf.ph), vec2(w as f32 * zoom, h as f32 * zoom))
            });
            Some((style.color, 12.0, style.fill, drawn))
        }
    };
    // The comment is gone (undone, deleted elsewhere): nothing left to type into.
    let Some((color, size, fill, old)) = look else {
        view.comments.inline = None;
        return None;
    };
    let anchor = old.map_or_else(
        || {
            let v = page.user_to_view(it.at[0] as f32, it.at[1] as f32);
            xf.norm_to_screen(v[0] / xf.pw, v[1] / xf.ph)
        },
        |r| r.left_top(),
    );
    let font = egui::FontId::proportional((size as f32 * zoom).max(8.0));
    let pad = 2.0 * zoom;
    let text_color = color32(color);
    let back = fill.map_or(Color32::WHITE, color32);
    // As wide as the longest line (the hint's, before any is typed), at least the box's own
    // width, wrapping past the widest a box is sized to (or past the box, when it is wider).
    let shown = if it.text.is_empty() { tl!("Type text").to_string() } else { it.text.clone() };
    let longest = ctx.fonts_mut(|f| f.layout_no_wrap(shown, font.clone(), text_color).size().x);
    let floor = old.map_or(size as f32 * zoom, |r| (r.width() - 2.0 * pad).max(1.0));
    let ceiling = if it.size.is_some() { floor } else { (pdfcraft_engine::annot_text::TEXT_BOX_MAX_WIDTH as f32 * zoom - 2.0 * pad).max(floor) };
    let width = (longest + font.size * 0.5).clamp(floor, ceiling);
    let (mut keep, mut cancel, mut left) = (false, false, false);
    egui::Area::new(egui::Id::unique(("comment-inline-text", view.id.0))).order(egui::Order::Foreground).fixed_pos(anchor).show(ctx, |ui| {
        let Some(t) = view.comments.inline.as_mut() else { return };
        ui.visuals_mut().text_cursor.stroke.color = crate::edit_text_ui::caret_color_on(back);
        // The area's max rect is last frame's size, which caps the editor's width; widen it so
        // the box grows as text is typed.
        ui.set_min_width(width + 2.0 * pad);
        let cover = ui.painter().add(egui::Shape::Noop);
        let r = ui.add(
            egui::TextEdit::multiline(&mut t.text)
                .font(font.clone())
                .text_color(text_color)
                .frame(egui::Frame::NONE.inner_margin(egui::Margin::same(pad.round().clamp(0.0, 100.0) as i8)))
                .desired_width(width)
                .desired_rows(1)
                .hint_text(tl!("Type text"))
                .id_salt(("comment-inline-edit", t.page, t.index)),
        );
        // The box's old drawing stays under the editor until the text is kept.
        let area = old.map_or(r.rect, |o| o.union(r.rect));
        ui.painter().set(cover, egui::Shape::rect_filled(area, CornerRadius::ZERO, back));
        ui.painter().rect_stroke(area.expand(1.0), CornerRadius::ZERO, Stroke::new(1.0, SELECT_BLUE), egui::StrokeKind::Outside);
        if t.focus {
            r.request_focus();
            t.focus = false;
        }
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            cancel = true;
        } else if submit_key(ui, &r, false) {
            keep = true;
        } else if r.lost_focus() {
            (keep, left) = (true, true);
        }
    });
    if cancel {
        view.comments.inline = None;
        return None;
    }
    if !keep {
        return None;
    }
    let it = view.comments.inline.take()?;
    view.comments.inline_left = left;
    if it.index.is_none() {
        // A one-shot tool, as with the composer: back to Select unless pinned.
        view.comments.tool_done = true;
    }
    inline_edit(&it, info, prefs)
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
