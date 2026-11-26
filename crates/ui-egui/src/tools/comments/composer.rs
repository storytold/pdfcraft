use super::bridge::new_comment;
use super::geometry::{drawn_text_box_rect, page_rotation, text_box_rect};
use super::state::{ComposerKind, PageCx};
use super::{CommentPrefs, CommentTool, DocInfo, DocView, Edit, LineEnding, Markup, NoteIcon, Rect, Shape};
use craft::review::ui::thread::{CommentComposer, ComposerClose, ComposerDraft};

/// The floating composer card (new note, new text box, edit). Returns the edit to apply.
pub(crate) fn composer(ui: &mut egui::Ui, view: &mut DocView, info: &DocInfo, prefs: &CommentPrefs) -> Option<Edit> {
    let c = view.session.comments.composer.as_ref()?;
    let page = c.page;
    // Scrolled away: keep the draft until the page is back.
    let xf = view.page_xform(page)?;
    let cx = PageCx { page, xf: &xf, info, prefs, allowed: true, hidden: false };
    let mut anchor = Some(cx.to_screen(c.at));
    let viewport = view.viewport_rect();
    let typewriter_edit = matches!(c.kind, ComposerKind::Edit(index) if info.annotations.iter().any(|a|
        a.page == page && a.index == index && a.subtype == "FreeText" && a.intent.as_deref() == Some("FreeTextTypeWriter")
    ));
    let c = view.session.comments.composer.as_mut()?;
    let title = match c.kind {
        ComposerKind::Note => "Sticky note",
        ComposerKind::TextNote { .. } => "Add comment",
        ComposerKind::TextBox { .. } => "Text box",
        ComposerKind::Callout { .. } => "Callout",
        ComposerKind::Caret { .. } => "Inserted text",
        ComposerKind::Replace => "Replacement text",
        ComposerKind::Edit(_) => "Edit comment",
    };
    // Its comment scrolled out of the document: keep the draft until it is back, rather than
    // pushing the card into the window over the mode bar and toolbar (#4021). An edited comment
    // that is only partly scrolled away keeps its card, at the nearest point inside the document.
    if let Some(at) = anchor
        && !viewport.contains(at)
    {
        let part_visible = matches!(c.kind, ComposerKind::Edit(index) if edited_in_view(&cx, index, viewport));
        if part_visible {
            anchor = Some(at.clamp(viewport.min, viewport.max));
        } else {
            return None;
        }
    }
    let hint = match c.kind {
        ComposerKind::TextBox { .. } | ComposerKind::Callout { .. } => tl!("Type text"),
        ComposerKind::Caret { .. } => tl!("Text to insert"),
        ComposerKind::Replace => tl!("Replacement text"),
        _ => tl!("Add a comment"),
    };
    let draft = match c.kind {
        ComposerKind::Edit(_) if typewriter_edit => ComposerDraft::EditVerbatim,
        ComposerKind::Edit(_) => ComposerDraft::Edit,
        _ => ComposerDraft::New,
    };
    let output = CommentComposer::new(anchor, &mut c.text)
        .header(tl!(title), &prefs.author)
        .hint(hint)
        .draft_kind(draft)
        .close(ComposerClose::OnPostOrCancel)
        // The card keeps one size, so its Post button stays inside it near the bottom of the page.
        .fixed_rows()
        .show(ui);
    if output.cancelled {
        view.session.comments.composer = None;
        return None;
    }
    let text = output.posted?;
    let c = view.session.comments.composer.take()?;
    match c.kind {
        ComposerKind::Edit(index) => {
            info.annotations.iter().any(|a| a.page == page && a.index == index).then_some(Edit::SetAnnotationContents { page, index, text })
        }
        ComposerKind::Note => {
            view.session.comments.tool_done = true;
            Some(new_comment(page, prefs, CommentTool::Note, Shape::Note { at: c.at, icon: NoteIcon::Comment }, text))
        }
        ComposerKind::TextNote { quads } => {
            view.session.comments.tool_done = true;
            Some(new_comment(page, prefs, CommentTool::Highlight, Shape::TextMarkup { kind: Markup::Highlight, quads }, text))
        }
        ComposerKind::TextBox { size } => {
            view.session.comments.tool_done = true;
            let rotation = page_rotation(info, page);
            let border = prefs.style(CommentTool::TextBox).width;
            let font_size = prefs.font_size(CommentTool::TextBox).unwrap_or(12.0);
            let rect = match (size, info.pages.get(page)) {
                (Some(size), Some(p)) => drawn_text_box_rect(p, c.at, size, &text, font_size, border),
                _ => text_box_rect(c.at, &text, font_size, rotation),
            };
            Some(new_comment(page, prefs, CommentTool::TextBox, Shape::TextBox { rect, font_size }, text))
        }
        ComposerKind::Callout { point } => {
            view.session.comments.tool_done = true;
            // A callout's leader and box stay in user-space axes (its appearance isn't turned).
            let font_size = prefs.font_size(CommentTool::Callout).unwrap_or(10.0);
            let rect = text_box_rect(c.at, &text, font_size, 0);
            let knee = pdfcraft_engine::callout_knee(rect, point);
            Some(new_comment(page, prefs, CommentTool::Callout, Shape::Callout { rect, knee, point, font_size, ending: LineEnding::OpenArrow }, text))
        }
        ComposerKind::Replace => {
            view.session.comments.tool_done = true;
            let quads = view.session.comments.replace.take()?;
            Some(Edit::ReplaceText {
                page,
                quads,
                text,
                author: prefs.author.clone(),
                strike: prefs.style(CommentTool::ReplaceText),
                // One proposal, one look: the caret, the group's primary, takes the Replace text
                // tool's style as the strikeout does, so a default saved from either part (Make
                // Current Properties Default) applies to the next proposal as a whole.
                caret: prefs.style(CommentTool::ReplaceText),
            })
        }
        ComposerKind::Caret { size } => {
            view.session.comments.tool_done = true;
            let [x, y] = c.at;
            let half = size / 2.0;
            Some(new_comment(page, prefs, CommentTool::Caret, Shape::Caret { rect: [x - half, y - size, x + half, y] }, text))
        }
    }
}

/// Whether comment `index` on the composer's page is at least partly inside `viewport`, as drawn.
pub(crate) fn edited_in_view(cx: &PageCx<'_>, index: usize, viewport: Rect) -> bool {
    cx.info.annotations.iter().any(|a| a.page == cx.page && a.index == index && viewport.intersects(cx.screen_rect(a)))
}
