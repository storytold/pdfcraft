//! The Comments panel (Acrobat's right-hand comment list; 11-visual-spec §3, audit `a15-*`).
//!
//! The list, its search, filters, sort, folding, inline editors and reply box are the framework's
//! [`CommentsPanel`]. This module maps the document's annotations to its threads and its edits
//! back to engine edits, and draws the header's filter and "…" buttons for the chrome.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use craft::behavior::IconId;
use craft::review::ui::comments::{CommentEdit, CommentsHeader, CommentsPanel, comments_filter_menu, comments_more_menu};
use craft::review::{CommentAnchor, CommentThread, PageOrder, ReviewStatus, ReviewerStatus, ThreadId};
use craft::ui::buttons::IconButton;
use craft::ui::contract::{Entry, Props as _, SelectMode, Show as _};
use craft::ui::primitives::{IconSpec, SubmitKey};
use pdfcraft_engine::{Edit, NoteIcon, ReviewState, Shape};
use pdfcraft_render::{Annotation, AttachmentSource, DocInfo, ReviewStates};

use crate::canvas::DocView;
// PLAN: assumed `crate::comments::{kind_of, pdf_date_seconds, thread_id, rgb8}` re-exported (`rgb8` is private in w7's bridge) and `CommentView::{panel, select_keys, copy_text_request}` (pdf-comments-w7)
use crate::comments::{CommentPrefs, CommentTool, kind_of, pdf_date_seconds, rgb8, thread_id};
use crate::panels::Nav;

/// Where a new note goes: this far from the page's right edge and down from its top, in page units.
const NOTE_FROM_RIGHT: f32 = 44.0;
const NOTE_FROM_TOP: f32 = 24.0;

/// Reader-friendly names for annotation subtypes (the PDF names are shown in tooltips).
pub fn subtype_label(s: &str) -> &str {
    match s {
        "Text" => "Note",
        "FreeText" => "Text box",
        "StrikeOut" => "Strikethrough",
        "Square" => "Rectangle",
        "Circle" => "Oval",
        "Ink" => "Drawing",
        "PolyLine" => "Polyline",
        "FileAttachment" => "Attachment",
        "Caret" => "Insert text",
        other => other,
    }
}

/// The comment type as the panel names it, telling callouts, clouds and replaced text apart by `/IT`.
pub fn kind_label(a: &Annotation) -> &str {
    match a.intent.as_deref() {
        // Replace Text: the caret (`/IT /Replace`) is the group's primary; earlier builds made
        // the strikeout primary.
        Some("StrikeOutTextEdit" | "Replace") => "Replace text",
        Some("FreeTextCallout") => "Callout",
        Some("PolygonCloud") => "Cloud",
        Some("FreeTextTypeWriter") => "Typewriter",
        Some("FreehandHighlight") if a.subtype == "Ink" => "Freehand-Highlight",
        _ => subtype_label(&a.subtype),
    }
}

/// The number shown in the header: comments, not their replies or status changes.
pub fn count(info: &DocInfo) -> usize {
    info.annotations.iter().filter(|a| a.is_comment()).count()
}

/// The replies under the header's count, at any depth: not status changes, checkmarks or group
/// subordinates (`/RT /Group`, such as Replace Text's strikeout: part of the comment, not a reply).
pub fn reply_count(info: &DocInfo) -> usize {
    info.annotations.iter().filter(|a| a.is_reply() && a.state.is_none() && !a.is_group_member()).count()
}

/// The items a card's menu adds after the framework's own rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CardRow {
    SaveAttachment,
    OpenAttachment,
    ToggleCheck,
    CopyText,
    Properties,
    MakeDefault,
    CopyComment,
}

/// Which annotation each thread and message is, for turning the panel's edits back into the
/// engine's `(page, index)` keys.
struct ThreadMap {
    /// Per thread, the annotation of each message: the comment first, then its replies.
    keys: HashMap<ThreadId, Vec<(usize, usize)>>,
    /// The thread every annotation of a thread belongs to, group members and replies included.
    owner: HashMap<(usize, usize), ThreadId>,
}

impl ThreadMap {
    /// The annotation of message `index` of `thread`.
    fn key(&self, thread: ThreadId, index: usize) -> Option<(usize, usize)> {
        self.keys.get(&thread)?.get(index).copied()
    }
}

fn review_status(state: &str) -> Option<ReviewStatus> {
    Some(match state {
        "None" => ReviewStatus::None,
        "Accepted" => ReviewStatus::Accepted,
        "Rejected" => ReviewStatus::Rejected,
        "Cancelled" => ReviewStatus::Cancelled,
        "Completed" => ReviewStatus::Completed,
        _ => return None,
    })
}

fn review_state(status: ReviewStatus) -> ReviewState {
    match status {
        ReviewStatus::None => ReviewState::None,
        ReviewStatus::Accepted => ReviewState::Accepted,
        ReviewStatus::Rejected => ReviewState::Rejected,
        ReviewStatus::Cancelled => ReviewState::Cancelled,
        ReviewStatus::Completed => ReviewState::Completed,
    }
}

/// Each reviewer's latest status on `a` with the text of the status reply that set it. A state
/// is "for the specified user" (ISO 32000-2 §12.5.6.3); an unknown state name is skipped.
fn reviewer_statuses(states: &ReviewStates<'_>, a: &Annotation) -> Vec<ReviewerStatus> {
    states
        .statuses(a)
        .into_iter()
        .filter_map(|(who, state)| {
            let status = review_status(state)?;
            let text = states
                .history(a)
                .iter()
                .rev()
                .find(|r| !r.is_mark() && r.state.is_some() && r.author.as_deref().unwrap_or_default().trim() == who.trim())
                .and_then(|r| r.contents.clone())
                .unwrap_or_default();
            Some(ReviewerStatus { reviewer: who.to_owned(), status, text })
        })
        .collect()
}

fn message(a: &Annotation, deletable: bool) -> craft::review::CommentMessage {
    craft::review::CommentMessage {
        author: a.author.clone().unwrap_or_default(),
        created: a.modified.as_deref().and_then(pdf_date_seconds).unwrap_or(0),
        modified: None,
        text: a.contents.clone().unwrap_or_default(),
        reactions: Vec::new(),
        read: true,
        deletable,
    }
}

/// The document's comments as the panel's threads, in document order, and the map back to the
/// annotations. Replies find their parent by reference (`/NM` need not be unique) and a reply at
/// any depth joins the thread of the comment it starts from. Status and checkmark replies are
/// read through `ReviewStates`, so they are no messages. A group (`/RT /Group`, ISO 32000-2
/// §12.5.6.2) is one comment, its primary's: a subordinate is no reply, and a reply to one is
/// the group's.
fn threads(doc: &pdfcraft_engine::Document, prefs: &CommentPrefs) -> (Vec<CommentThread>, ThreadMap) {
    let info = &doc.info;
    let tops = pdfcraft_render::thread_tops(&info.annotations);
    let states = ReviewStates::new(&info.annotations);
    let mut replies: HashMap<(usize, usize), Vec<&Annotation>> = HashMap::new();
    for a in &info.annotations {
        if a.is_group_member() {
            continue;
        }
        if let Some(parent) = a.parent
            && a.state.is_none()
        {
            replies.entry(tops.get(&(a.page, a.index)).copied().unwrap_or(parent)).or_default().push(a);
        }
    }
    let mut list = Vec::new();
    let mut keys = HashMap::new();
    for a in info.annotations.iter().filter(|a| a.is_comment() && !a.is_group_member()) {
        let key = (a.page, a.index);
        let id = thread_id(key.0, key.1);
        let replies = replies.get(&key).map(Vec::as_slice).unwrap_or(&[]);
        let mut messages = vec![message(a, false)];
        let mut members = vec![key];
        for r in replies {
            messages.push(message(r, false));
            members.push((r.page, r.index));
        }
        list.push(CommentThread {
            id,
            anchor: CommentAnchor::Page { page: a.page, at: [f64::from(a.rect[0]), f64::from(a.rect[1])] },
            kind: Some(kind_of(&info.annotations, a)),
            color: a.color.map(rgb8),
            messages,
            statuses: reviewer_statuses(&states, a),
            reply_statuses: replies.iter().flat_map(|r| reviewer_statuses(&states, r)).collect(),
            marked: states.marked_by(a, &prefs.author),
            locked: a.locked,
            resolved: false,
        });
        keys.insert(id, members);
    }
    let owner = info
        .annotations
        .iter()
        .filter_map(|a| {
            let key = (a.page, a.index);
            let top = tops.get(&key).copied().unwrap_or(key);
            let id = thread_id(top.0, top.1);
            keys.contains_key(&id).then_some((key, id))
        })
        .collect();
    (list, ThreadMap { keys, owner })
}

fn annotation_at(info: &DocInfo, key: (usize, usize)) -> Option<&Annotation> {
    info.annotations.iter().find(|a| (a.page, a.index) == key)
}

/// The rows a comment's card menu adds: its attachment, checkmark, copy and properties. Replies
/// add none.
fn card_rows(doc: &pdfcraft_engine::Document, map: &ThreadMap, thread: &CommentThread, index: usize) -> Vec<Entry<CardRow>> {
    let info = &doc.info;
    let Some(a) = (index == 0).then(|| map.key(thread.id, 0)).flatten().and_then(|key| annotation_at(info, key)) else {
        return Vec::new();
    };
    let mut rows = Vec::new();
    let source = AttachmentSource::Annotation { page: a.page, index: a.index };
    if let Some(file) = info.attachments.iter().find(|f| f.source == source) {
        rows.push(Entry::new(CardRow::SaveAttachment, tl!("Save attachment…").to_string()));
        if file.name.to_lowercase().ends_with(".pdf") {
            rows.push(Entry::new(CardRow::OpenAttachment, tl!("Open in a new tab").to_string()));
        }
        rows.push(Entry::separator());
    }
    let check = if thread.marked { tl!("Remove checkmark") } else { tl!("Mark with checkmark") };
    rows.push(Entry::new(CardRow::ToggleCheck, check.to_string()));
    rows.push(Entry::new(CardRow::CopyText, tl!("Copy text").to_string()));
    rows.push(Entry::new(CardRow::Properties, tl!("Properties…").to_string()));
    let defaultable = crate::comments::defaultable(a);
    let mut make_default = Entry::new(CardRow::MakeDefault, tl!("Make Current Properties Default").to_string()).enabled(defaultable);
    if !defaultable {
        make_default = make_default.disabled_reason(tl!("Comments of this kind have no tool to set defaults for").to_string());
    }
    rows.push(make_default);
    rows.push(Entry::new(CardRow::CopyComment, tl!("Copy comment").to_string()));
    rows
}

// PLAN: the comments area has no shared clock yet (w10's note popups read one too); this is the only read here.
fn now_seconds() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).ok().and_then(|d| i64::try_from(d.as_secs()).ok()).unwrap_or(0)
}

/// Draw the panel. Returns an edit to apply (posting, replying, deleting…).
pub(crate) fn show(
    ui: &mut egui::Ui,
    doc: &pdfcraft_engine::Document,
    view: &mut DocView,
    prefs: &CommentPrefs,
    allowed: bool,
    nav: &mut Option<Nav>,
) -> Option<Edit> {
    let info = &doc.info;
    let (mut list, map) = threads(doc, prefs);
    // Keep the panel's selection in step with the page's (clicks on the page, or newly added comments),
    // without disturbing an open inline editor.
    let mut selected_threads = Vec::new();
    for key in view.session.comments.selection() {
        if let Some(&id) = map.owner.get(&key)
            && !selected_threads.contains(&id)
        {
            selected_threads.push(id);
        }
    }
    let lead = selected_threads.first().copied();
    let synced = craft::behavior::Selection::new(selected_threads, lead, lead);
    if view.session.comments.panel.selected.items != synced.items || view.session.comments.panel.selected.lead != synced.lead {
        view.session.comments.panel.selected = synced;
    }
    // A comment selected elsewhere (on the page, or just added) is revealed in this same pass: the
    // panel unfolds its page and scrolls its card in. A reply or group member reveals its comment.
    // `reveal` drops an inline edit when the lead changes, so preserve a newly opened editor on the
    // revealed comment or an unsaved note being typed in another card.
    if std::mem::take(&mut view.session.comments.reveal)
        && let Some(id) = view.session.comments.selection().first().and_then(|key| map.owner.get(key).copied())
    {
        let saved_edit = view.session.comments.panel.editing.clone();
        view.session.comments.panel.reveal(id);
        match saved_edit {
            Some((thread, index, text)) if thread == id => view.session.comments.panel.edit(thread, index, text),
            Some((thread, index, text)) if !text.is_empty() => {
                view.session.comments.panel.editing = Some((thread, index, text));
            }
            _ => {}
        }
    }
    // Only the lead card exposes message menus. Checking every annotation would repeatedly scan the PDF's
    // annotation list for every row, making a large comments panel quadratic even when its pages are folded.
    if let Some(lead) = view.session.comments.panel.selected.lead
        && let Some(thread) = list.iter_mut().find(|thread| thread.id == lead)
    {
        for (index, message) in thread.messages.iter_mut().enumerate() {
            message.deletable = map.key(lead, index).is_some_and(|(page, at)| doc.annotation_deletable(page, at));
        }
    }
    let output = CommentsPanel::new(tl!("Comments"), &list, &mut view.session.comments.panel)
        .header(CommentsHeader::Hosted)
        .page_order(PageOrder::Document)
        .select_mode(SelectMode::Multi)
        .reply_submit(SubmitKey::Enter)
        .read_only(!allowed)
        .now(now_seconds())
        .page_label(|page| info.pages.get(page).map_or("?", |p| p.label.as_str()).to_owned())
        .card_rows(|thread, index| card_rows(doc, &map, thread, index))
        .show(ui);
    // Picking a card in the panel (even one already selected on the page) acts on the card's
    // comment, forgetting any group subordinate a page click remembered (#4358).
    if output.response.contains_pointer() && ui.input(|i| i.pointer.any_pressed()) {
        view.session.comments.member = None;
    }
    let mut applied = Vec::new();
    for e in output.edits {
        match e {
            CommentEdit::Add { text, .. } => {
                let page = view.current().min(info.pages.len().saturating_sub(1));
                if let Some(p) = info.pages.get(page) {
                    let at = p.view_to_user(p.width - NOTE_FROM_RIGHT, NOTE_FROM_TOP);
                    applied.push(Edit::AddAnnotation(pdfcraft_engine::NewAnnotation {
                        page,
                        shape: Shape::Note { at: [at[0] as f64, at[1] as f64], icon: NoteIcon::Comment },
                        style: prefs.style(CommentTool::Note),
                        contents: text.trim().to_string(),
                        author: prefs.author.clone(),
                    }));
                }
            }
            CommentEdit::Reply { thread, text, .. } => {
                if let Some((page, index)) = map.key(thread, 0) {
                    applied.push(Edit::ReplyToAnnotation { page, index, text: text.trim().to_string(), author: prefs.author.clone() });
                }
            }
            CommentEdit::Edit { thread, index, text, .. } => {
                if let Some((page, index)) = map.key(thread, index) {
                    applied.push(Edit::SetAnnotationContents { page, index, text });
                }
            }
            CommentEdit::Delete { thread, index, .. } => {
                if let Some((page, at)) = map.key(thread, index)
                    && doc.annotation_deletable(page, at)
                {
                    if index == 0 {
                        view.session.comments.clear_selection();
                    } else if let Some((et, ei, _)) = view.session.comments.panel.editing.as_mut()
                        && *et == thread
                    {
                        if *ei == index {
                            view.session.comments.panel.editing = None;
                        } else if *ei > index {
                            *ei -= 1;
                        }
                    }
                    applied.push(Edit::DeleteAnnotation { page, index: at });
                }
            }
            CommentEdit::SetStatus { thread, status, .. } => {
                if let Some((page, index)) = map.key(thread, 0) {
                    applied.push(Edit::SetAnnotationStatus { page, index, state: review_state(status), author: prefs.author.clone() });
                }
            }
            CommentEdit::Mark { thread, marked, .. } => {
                if let Some((page, index)) = map.key(thread, 0) {
                    applied.push(Edit::MarkAnnotation { page, index, marked, author: prefs.author.clone() });
                }
            }
            CommentEdit::Select(ids) => {
                // The lead goes first: `select_keys` makes its first key the one with the handles.
                let lead = view.session.comments.panel.selected.lead;
                let ordered = lead.into_iter().chain(ids.iter().copied().filter(|id| Some(*id) != lead));
                let keys: Vec<(usize, usize)> = ordered.filter_map(|id| map.key(id, 0)).collect();
                view.session.comments.select_keys(&keys);
                if let Some(&key) = keys.first()
                    && let Some(a) = annotation_at(info, key)
                {
                    let rotation = info.pages.get(a.page).map_or(0, |p| p.rotation);
                    *nav = Some(Nav::Flash(a.page, a.shown_rect(rotation)));
                }
            }
            CommentEdit::CopyText { thread, index, .. } => {
                if let Some(a) = map.key(thread, index).and_then(|key| annotation_at(info, key)) {
                    view.session.comments.copy_text_request = Some(a.contents.clone().unwrap_or_default());
                }
            }
            _ => {}
        }
    }
    if let Some((id, index, row)) = output.host
        && let Some(key) = map.key(id, index)
    {
        match row {
            CardRow::SaveAttachment => view.session.comments.attachment_request = Some((key.0, key.1, false)),
            CardRow::OpenAttachment => view.session.comments.attachment_request = Some((key.0, key.1, true)),
            CardRow::ToggleCheck => {
                let marked = list.iter().find(|t| t.id == id).is_some_and(|t| t.marked);
                applied.push(Edit::MarkAnnotation { page: key.0, index: key.1, marked: !marked, author: prefs.author.clone() });
            }
            CardRow::CopyText => {
                let text = annotation_at(info, key).and_then(|a| a.contents.clone()).unwrap_or_default();
                view.session.comments.copy_text_request = Some(text);
            }
            CardRow::Properties => view.session.comments.props_request = Some(key),
            CardRow::MakeDefault => view.session.comments.default_request = Some(key),
            CardRow::CopyComment => {
                // The card's comment alone (the page's menu copies the whole selection).
                view.session.comments.select_only(key);
                view.session.comments.copy_request = true;
            }
        }
    }
    match applied.len() {
        0 | 1 => applied.pop(),
        _ => Some(Edit::Batch { label: tl!("Comments").to_string(), edits: applied }),
    }
}

/// The Comments panel header's filter and "…" buttons, with their menus. Returns the command of
/// the "…" menu row picked, for the caller to run.
pub(crate) fn header(
    ui: &mut egui::Ui,
    doc: &pdfcraft_engine::Document,
    view: &mut DocView,
    prefs: &CommentPrefs,
    enabled: impl Fn(&str) -> bool,
) -> Option<&'static str> {
    let (list, _) = threads(doc, prefs);
    let more = IconButton::new(IconSpec::svg(IconId::new("more"))).label(tl!("More options").to_string()).show(ui);
    let hide_all = if doc.comments_hidden() { tl!("Show all comments") } else { tl!("Hide all comments") };
    let rows = [
        Entry::new("comment.hide_all", hide_all.to_string()).enabled(enabled("comment.hide_all")),
        Entry::new("comment.summarize", tl!("Create PDF comment summary…").to_string()).enabled(enabled("comment.summarize")),
        Entry::new("comment.export", tl!("Export all to data file…").to_string()).enabled(enabled("comment.export")),
        Entry::new("comment.import", tl!("Import comments…").to_string()).enabled(enabled("comment.import")),
        // Into the pages, for good once saved: apart from the list's other commands.
        Entry::separator(),
        Entry::new("comment.flatten", tl!("Flatten comments").to_string()).enabled(enabled("comment.flatten")),
    ];
    let command = comments_more_menu(ui, &more, &list, &mut view.session.comments.panel, &rows);
    let filter = IconButton::new(IconSpec::svg(IconId::new("funnel")))
        .label(tl!("Filter comments").to_string())
        .selected(view.session.comments.panel.filter.is_active())
        .show(ui);
    comments_filter_menu(ui, &filter, &list, &mut view.session.comments.panel);
    command
}

#[cfg(test)]
mod tests {
    use pdfcraft_render::Annotation;

    use super::*;

    fn mock_annotation(subtype: &str, intent: Option<&str>) -> Annotation {
        Annotation {
            page: 0,
            subtype: subtype.into(),
            author: None,
            contents: None,
            modified: None,
            name: None,
            in_reply_to: None,
            parent: None,
            grouped: false,
            rect: [0.0; 4],
            color: None,
            index: 0,
            state: None,
            quads: Vec::new(),
            locked: false,
            intent: intent.map(str::to_owned),
            fill_sign: false,
            no_rotate: false,
            points: Vec::new(),
            text_box: None,
            popup: None,
        }
    }

    #[test]
    fn freehand_highlight_kind_label() {
        let freehand = mock_annotation("Ink", Some("FreehandHighlight"));
        assert_eq!(kind_label(&freehand), "Freehand-Highlight");

        let plain_ink = mock_annotation("Ink", None);
        assert_eq!(kind_label(&plain_ink), "Drawing");

        let non_ink = mock_annotation("Square", Some("FreehandHighlight"));
        assert_eq!(kind_label(&non_ink), "Rectangle");
    }
}
