use craft::review::ui::thread::{NOTE_POPUP_LIMITS, NoteDrafts, NotePopup, NotePopupMessage};
use craft::ui::contract::overlay::PlacementKind;

use super::state::PageCx;
use super::{Annotation, CommentPrefs, DocInfo, DocView, Edit, Rect};

/// A comment with a pop-up window of its own: not a text box, a Fill & Sign mark, a reply or a
/// status (replies are found by `/IRT` reference, [`Annotation::is_comment`]).
pub(super) fn has_popup(a: &Annotation) -> bool {
    a.subtype != "FreeText" && !a.fill_sign && a.is_comment()
}

/// Past this depth a reply's panel stops nesting and sits under the one before it.
const MAX_NESTING: usize = 8;

/// `replies` (all filed under comment `top`) in thread order, each with its depth: a reply to
/// `top` is 1, and each reply comes right after the one it answers, followed by its own replies.
/// One whose parent isn't among them (a status reply, say) answers `top`. A cycle of `/IRT`
/// references shows each reply once.
fn thread<'a>(top: (usize, usize), replies: &[&'a Annotation]) -> Vec<(&'a Annotation, usize)> {
    use std::collections::{HashMap, HashSet};
    let known: HashSet<(usize, usize)> = replies.iter().map(|r| (r.page, r.index)).collect();
    let mut answered: HashMap<(usize, usize), Vec<&'a Annotation>> = HashMap::new();
    for r in replies {
        let parent = r.parent.filter(|p| known.contains(p) && *p != (r.page, r.index)).unwrap_or(top);
        answered.entry(parent).or_default().push(r);
    }
    let mut out = Vec::with_capacity(replies.len());
    let mut seen = HashSet::new();
    // Depth first, answers in document order: the stack holds them reversed.
    let below = |key: (usize, usize), depth: usize| answered.get(&key).into_iter().flatten().rev().map(move |r| (*r, depth));
    let mut stack: Vec<(&Annotation, usize)> = below(top, 1).collect();
    let mut next = 0;
    loop {
        while let Some((r, depth)) = stack.pop() {
            if !seen.insert((r.page, r.index)) {
                continue;
            }
            out.push((r, depth));
            stack.extend(below((r.page, r.index), depth.saturating_add(1)));
        }
        // Replies only a cycle reaches: shown from their first, as if they answered `top`. The
        // search resumes where it stopped, so the whole of it is one pass.
        while next < replies.len() && replies.get(next).is_some_and(|r| seen.contains(&(r.page, r.index))) {
            next += 1;
        }
        match replies.get(next) {
            Some(r) => stack.push((*r, 1)),
            None => break,
        }
    }
    out
}

/// A thread as nested panels: each reply ([`thread`] order) and the positions of the replies
/// drawn inside its panel; then the positions of the replies to the comment itself. Nesting stops
/// at [`MAX_NESTING`], so reading it recurses no deeper than that.
fn nest<'a>(order: &[(&'a Annotation, usize)]) -> (Vec<(&'a Annotation, Vec<usize>)>, Vec<usize>) {
    let mut nodes: Vec<(&Annotation, Vec<usize>)> = Vec::with_capacity(order.len());
    let mut roots = Vec::new();
    // The panels open at each depth above the reply being placed.
    let mut open: Vec<usize> = Vec::new();
    for (i, (r, depth)) in order.iter().enumerate() {
        let depth = (*depth).clamp(1, MAX_NESTING);
        open.truncate(depth - 1);
        match open.last().and_then(|p| nodes.get_mut(*p)) {
            Some((_, kids)) => kids.push(i),
            None => roots.push(i),
        }
        nodes.push((r, Vec::new()));
        open.push(i);
    }
    (nodes, roots)
}

/// The replies of `nodes` below `at`, each with its depth, in the order the window reads them.
fn flatten<'a>(nodes: &[(&'a Annotation, Vec<usize>)], at: usize, depth: usize, out: &mut Vec<(&'a Annotation, usize)>) {
    let Some((r, kids)) = nodes.get(at) else { return };
    out.push((r, depth));
    for kid in kids {
        flatten(nodes, *kid, depth + 1, out);
    }
}

/// One message of a window, owned until the window is built.
struct Owned {
    key: (usize, usize),
    id: u64,
    title: String,
    author: String,
    text: String,
    modified: Option<i64>,
    depth: usize,
    editable: bool,
}

fn modified_seconds(a: &Annotation) -> Option<i64> {
    a.modified.as_deref().and_then(super::bridge::pdf_date_seconds)
}

/// The window's messages: the comment, then its text replies in thread order. Replies are the comment's text
/// replies only, filed by `/IRT` reference as in the Comments panel: a review status or a checkmark (`/State`)
/// is a reply too but not one to read here, and a member of the comment's group (`/RT /Group`) is part of the
/// comment. Replace Text as `main` writes it keeps the replacement on the caret grouped under an empty
/// strikeout: the window shows and edits the caret's text, and locking the comment locks that text too.
/// Returns the messages and the annotation holding the comment's text.
fn messages(
    info: &DocInfo,
    tops: &std::collections::HashMap<(usize, usize), (usize, usize)>,
    a: &Annotation,
    allowed: bool,
) -> (Vec<Owned>, (usize, usize)) {
    let key = (a.page, a.index);
    let replies: Vec<&Annotation> = info
        .annotations
        .iter()
        .filter(|r| r.state.is_none() && !r.grouped && r.parent.is_some_and(|parent| tops.get(&(r.page, r.index)).copied().unwrap_or(parent) == key))
        .collect();
    let mut holder = info
        .annotations
        .iter()
        .find(|r| {
            r.grouped
                && r.parent == Some(key)
                && a.contents.as_deref().unwrap_or("").is_empty()
                && r.contents.as_deref().is_some_and(|c| !c.is_empty())
        })
        .cloned()
        .unwrap_or_else(|| a.clone());
    holder.locked |= a.locked;
    let kind = tl!(crate::comments_panel::kind_label(a));
    let mut out = vec![Owned {
        key: (holder.page, holder.index),
        // PLAN: assumed `bridge::id_of(page: usize, index: usize) -> AnnotationId` (pdf-comments-w7)
        id: super::bridge::id_of(holder.page, holder.index).0,
        title: kind.to_string(),
        author: a.author.clone().unwrap_or_default(),
        text: holder.contents.clone().unwrap_or_default(),
        modified: modified_seconds(a),
        depth: 0,
        editable: allowed && !holder.locked,
    }];
    let (nodes, roots) = nest(&thread(key, &replies));
    let mut ordered = Vec::with_capacity(nodes.len());
    for root in roots {
        flatten(&nodes, root, 1, &mut ordered);
    }
    for (r, depth) in ordered {
        let who = r.author.clone().unwrap_or_else(|| "?".to_string());
        out.push(Owned {
            key: (r.page, r.index),
            id: super::bridge::id_of(r.page, r.index).0,
            title: who.clone(),
            author: who,
            text: r.contents.clone().unwrap_or_default(),
            modified: modified_seconds(r),
            depth,
            editable: false,
        });
    }
    (out, (holder.page, holder.index))
}

/// The open pop-up windows of the comments on screen: each a framework note window with its comment's
/// text, editable in place, and its replies. The window keeps its size on screen as the page zooms; its
/// place and size are saved with the comment when a move or resize ends, and × closes it. Returns the edits
/// to apply (one, or a batch when a gesture both saves typed text and closes). Hidden comments show none.
/// `hovered` is the comment the page body reports under the pointer; its connector shows while it is.
// PLAN: pdf-canvas-w8 passes `hovered` from the page body's `PageParts` output.
pub(crate) fn note_popups(
    ui: &mut egui::Ui,
    view: &mut DocView,
    info: &DocInfo,
    prefs: &CommentPrefs,
    hidden: bool,
    allowed: bool,
    hovered: Option<(usize, usize)>,
) -> Option<Edit> {
    let mut edits: Vec<Edit> = Vec::new();
    // The comment each reply files under, as the Comments panel threads them (by reference).
    let tops = pdfcraft_render::thread_tops(&info.annotations);
    let now = (craft::platform::clock::unix_ms() / 1000.0) as i64;
    let limits = NOTE_POPUP_LIMITS;
    let max = limits.max.unwrap_or(egui::Vec2::splat(f32::MAX));
    let clamp = |s: egui::Vec2| s.max(limits.min).min(max);
    for (page, xf) in view.page_xforms() {
        let pcx = PageCx { page, xf: &xf, info, prefs, allowed, hidden };
        let shown: Vec<&Annotation> =
            info.annotations.iter().filter(|a| !hidden && a.page == page && has_popup(a) && a.popup.is_some_and(|p| p.open)).collect();
        for a in shown {
            let Some(p) = a.popup else { continue };
            let placed = xf.user_rect(info, page, p.rect);
            // Its size is the pop-up's `/Rect` in points on screen whatever the zoom; a `/Rect` with numbers
            // that aren't finite gives the least size.
            let side = |lo: f32, hi: f32| Some((hi - lo).abs()).filter(|v| v.is_finite()).unwrap_or(0.0);
            let saved = clamp(egui::vec2(side(p.rect[0], p.rect[2]), side(p.rect[1], p.rect[3])));
            let rect = Rect::from_min_size(placed.left_top(), saved);
            let (owned, holder) = messages(info, &tops, a, allowed);
            let msgs: Vec<NotePopupMessage<'_>> = owned
                .iter()
                .map(|m| NotePopupMessage {
                    id: m.id,
                    title: &m.title,
                    author: &m.author,
                    text: &m.text,
                    modified: m.modified,
                    depth: m.depth,
                    editable: m.editable,
                })
                .collect();
            let mark = pcx.screen_rects(a).next();
            // PLAN: `Annotation::color` is f32 components in 0..=1; the window takes 8-bit.
            // A comment with no colour has the note yellow, as the page icon does.
            let color = Some(a.color.map_or([0xFF, 0xD6, 0x5C], |c| c.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)));
            let selected = view.session.comments.is_selected((page, a.index));
            let popup_id = view.session.comments.popup_id((page, a.index));
            let window_label = match owned.first() {
                Some(m) if !m.author.is_empty() => format!("{} – {}", m.title, m.author),
                Some(m) => m.title.clone(),
                None => String::new(),
            };
            let drafts: &mut NoteDrafts = view.session.comments.popup_drafts.entry((page, a.index)).or_default();
            let mut window = NotePopup::new(popup_id, window_label, rect, &msgs, drafts).color(color).selected(selected).now(now).allowed(allowed);
            if let Some(mark) = mark {
                window = window.anchor(mark, hovered == Some((page, a.index)));
            }
            let out = window.show(ui);
            if out.closed {
                edits.push(Edit::SetAnnotationPopup { page, index: a.index, rect: None, open: Some(false) });
            }
            if let Some(text) = out.body {
                let text = text.trim_end().to_string();
                let current = info.annotations.iter().find(|r| (r.page, r.index) == holder).and_then(|r| r.contents.as_deref()).unwrap_or_default();
                if text != current {
                    edits.push(Edit::SetAnnotationContents { page: holder.0, index: holder.1, text });
                }
            }
            if let Some((id, text)) = out.reply
                && let Some(m) = owned.iter().find(|m| m.id == id)
                && !text.trim().is_empty()
            {
                edits.push(Edit::ReplyToAnnotation { page: m.key.0, index: m.key.1, text: text.trim().to_string(), author: prefs.author.clone() });
            }
            if let Some((at, kind)) = out.placed
                && allowed
            {
                let resized = match kind {
                    PlacementKind::Size(_) => true,
                    PlacementKind::Keyboard => (at.size() - rect.size()).length() >= 0.5,
                    _ => false,
                };
                if resized {
                    // The window's size in points is the `/Rect`'s, its upper left corner kept where it is on
                    // screen. A page turned a quarter shows the `/Rect`'s width down the screen. A press that
                    // ends where it began changes nothing.
                    let s = clamp(at.size());
                    let zoom = xf.zoom();
                    if (s - saved).length() >= 2.0 && zoom.is_finite() && zoom > 0.0 {
                        let turn = info.pages.get(page).map_or(0, |pg| u32::from(pg.rotation)) + u32::from(xf.rot);
                        let on_page = if turn % 180 == 90 { egui::vec2(s.y, s.x) } else { s } * zoom;
                        let user = super::bridge::screen_to_user_rect(&pcx, Rect::from_min_size(placed.left_top(), on_page));
                        edits.push(Edit::SetAnnotationPopup { page, index: a.index, rect: Some(user), open: None });
                    }
                } else if at.left_top().distance(rect.left_top()) >= 2.0 {
                    // The same size on the page, at the new place.
                    let user = super::bridge::screen_to_user_rect(&pcx, Rect::from_min_size(at.left_top(), placed.size()));
                    edits.push(Edit::SetAnnotationPopup { page, index: a.index, rect: Some(user), open: None });
                }
            }
        }
    }
    match edits.len() {
        0 => None,
        1 => edits.pop(),
        _ => Some(Edit::Batch { label: "Edit comment".into(), edits }),
    }
}
