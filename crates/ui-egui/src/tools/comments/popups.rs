use super::state::PageCx;
use super::{Annotation, Color32, CommentPrefs, CornerRadius, DocInfo, DocView, Edit, PagePointer, PageXform, Pos2, QuickTool, Rect, pos2, theme};

/// A comment with a pop-up window of its own: not a text box, a Fill & Sign mark, a reply or a
/// status (replies are found by `/IRT` reference, [`Annotation::is_comment`]).
pub(super) fn has_popup(a: &Annotation) -> bool {
    a.subtype != "FreeText" && !a.fill_sign && a.is_comment()
}

/// The connector from a comment to its open pop-up, in the comment's colour, while the pointer is
/// on either or the comment is selected (Acrobat's "lines connecting comment markups to their
/// pop-ups on mouse rollover"). It is a faint wedge: a point on the comment where it is
/// nearest the window, widening to a base on the window's side that faces
/// the comment ([`wedge_base`]).
pub(super) fn paint_connectors(painter: &egui::Painter, cx: &PageCx<'_>, view: &DocView, pointer: Option<Pos2>) {
    let cv = &view.comments;
    for ((page, index), window) in &cv.popup_rects {
        if *page != cx.page {
            continue;
        }
        let Some(a) = cx.get(*index) else { continue };
        if !a.popup.is_some_and(|p| p.open) {
            continue;
        }
        let rects: Vec<Rect> = cx.screen_rects(a).collect();
        let on_mark = pointer.is_some_and(|p| rects.iter().any(|r| r.expand(3.0).contains(p)));
        if !(on_mark || cv.selected == Some((*page, *index)) || cv.popup_hovered == Some((*page, *index))) {
            continue;
        }
        let Some(mark) = rects.first() else { continue };
        let from = nearest_on(*mark, window.center());
        let to = nearest_on(*window, from);
        if from.distance(to) < 4.0 {
            continue;
        }
        let color = a
            .color
            .map_or(Color32::from_rgb(0xDC, 0x26, 0x26), |c| Color32::from_rgb((c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8));
        let faded = Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), WEDGE_ALPHA);
        let [b0, b1] = wedge_base(*window, from, to);
        let mut wedge = egui::Mesh::default();
        for p in [from, b0, b1] {
            wedge.colored_vertex(p, faded);
        }
        wedge.add_triangle(0, 1, 2);
        painter.add(egui::Shape::mesh(wedge));
    }
}

/// How opaque a connector's wedge is (of 255).
const WEDGE_ALPHA: u8 = 90;

/// The two ends of a connector's base on `window`: centred on `to` (the point of the window
/// nearest the comment) along the side that faces `from`, as wide as a small part of the window
/// and kept on that side. At a corner, the side is the one the comment lies further beyond.
fn wedge_base(window: Rect, from: Pos2, to: Pos2) -> [Pos2; 2] {
    let half = (window.width().min(window.height()) * 0.08).clamp(5.0, 18.0);
    // `half` along [lo, hi] around `c`, moved in to stay on the side (a side shorter than the
    // base gives all of it).
    let spread = |c: f32, lo: f32, hi: f32| {
        let h = half.min((hi - lo).max(0.0) / 2.0);
        let c = c.clamp(lo + h, hi - h);
        (c - h, c + h)
    };
    let on_horizontal = (to.y - window.top()).abs() < 0.5 || (to.y - window.bottom()).abs() < 0.5;
    let on_vertical = (to.x - window.left()).abs() < 0.5 || (to.x - window.right()).abs() < 0.5;
    let beyond_x = (from.x - to.x).abs() / window.width().max(1.0);
    let beyond_y = (from.y - to.y).abs() / window.height().max(1.0);
    if on_horizontal && !(on_vertical && beyond_x > beyond_y) {
        let (a, b) = spread(to.x, window.left(), window.right());
        [pos2(a, to.y), pos2(b, to.y)]
    } else {
        let (a, b) = spread(to.y, window.top(), window.bottom());
        [pos2(to.x, a), pos2(to.x, b)]
    }
}

/// The point of `r`'s boundary nearest `p` (`p` inside `r` is pushed to the nearer vertical edge).
fn nearest_on(r: Rect, p: Pos2) -> Pos2 {
    let x = p.x.clamp(r.left(), r.right());
    let y = p.y.clamp(r.top(), r.bottom());
    if r.left() < x && x < r.right() && r.top() < y && y < r.bottom() {
        pos2(if (p.x - r.left()).abs() <= (p.x - r.right()).abs() { r.left() } else { r.right() }, y)
    } else {
        pos2(x, y)
    }
}

/// A pop-up window's size on screen, in points, whatever the zoom (the readers keep theirs the
/// same as the page zooms), kept within these bounds whatever size the file gives it.
const MIN_WIDTH: f32 = 200.0;
const MAX_WIDTH: f32 = 800.0;
const MIN_HEIGHT: f32 = 120.0;
const MAX_HEIGHT: f32 = 800.0;
/// The side of the grip at a window's lower right corner that resizes it.
const RESIZE_GRIP: f32 = 10.0;
/// How far a reply's panel sits in from the panel around it.
const NEST: i8 = 6;
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
/// at [`MAX_NESTING`], so drawing it recurses no deeper than that.
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

/// A pop-up window's colours: the comment's colour, lightened for the body and the title bar,
/// and ink that reads on them. Fixed, not from the app's theme: the window is the comment's, as
/// it looks on paper.
#[derive(Clone, Copy)]
struct Palette {
    edge: Color32,
    title: Color32,
    title_ink: Color32,
    body: Color32,
    panel_edge: Color32,
    ink: Color32,
    faint: Color32,
}

fn palette(color: Color32) -> Palette {
    // `color` mixed into white, `f` of the way.
    let mix = |f: f32| {
        let c = |v: u8| (255.0 + (f32::from(v) - 255.0) * f).round().clamp(0.0, 255.0) as u8;
        Color32::from_rgb(c(color.r()), c(color.g()), c(color.b()))
    };
    let title = mix(0.8);
    let light = |c: Color32| 0.299 * f32::from(c.r()) + 0.587 * f32::from(c.g()) + 0.114 * f32::from(c.b()) > 150.0;
    Palette {
        edge: color,
        title,
        title_ink: if light(title) { Color32::from_gray(20) } else { Color32::WHITE },
        body: mix(0.35),
        panel_edge: mix(0.7),
        ink: Color32::from_gray(20),
        faint: Color32::from_gray(80),
    }
}

/// What a pop-up window's parts share as they draw.
struct Parts<'v> {
    allowed: bool,
    author: &'v str,
    pal: Palette,
    cv: &'v mut super::state::CommentView,
    edits: &'v mut Vec<Edit>,
}

/// One comment's or reply's text in its box, typed in place (saved when the box loses focus);
/// read-only when comments can't be changed or it is locked.
fn text_box(ui: &mut egui::Ui, parts: &mut Parts<'_>, a: &Annotation, name: &str) {
    let key = (a.page, a.index);
    if parts.allowed && !a.locked {
        let typed = parts.cv.popup_text.as_ref().filter(|(p, i, _)| (*p, *i) == key).map(|(_, _, s)| s.clone());
        let mut text = typed.unwrap_or_else(|| a.contents.clone().unwrap_or_default());
        let field = ui
            .add(
                egui::TextEdit::multiline(&mut text)
                    .desired_rows(1)
                    .desired_width(f32::INFINITY)
                    .font(theme::regular(12.5))
                    .text_color(parts.pal.ink)
                    .background_color(Color32::WHITE)
                    .margin(egui::Margin::symmetric(4, 2))
                    .id_salt(("popup-text", a.page, a.index)),
            )
            .accessible_name(name);
        if field.changed() {
            parts.cv.popup_text = Some((a.page, a.index, text.clone()));
        }
        if field.lost_focus()
            && let Some((_, _, s)) = parts.cv.popup_text.take_if(|(p, i, _)| (*p, *i) == key)
            && s.trim_end() != a.contents.as_deref().unwrap_or_default()
        {
            parts.edits.push(Edit::SetAnnotationContents { page: a.page, index: a.index, text: s.trim_end().to_string() });
        }
    } else {
        egui::Frame::NONE.fill(Color32::WHITE).inner_margin(egui::Margin::symmetric(4, 2)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(egui::RichText::new(a.contents.as_deref().unwrap_or_default()).font(theme::regular(12.5)).color(parts.pal.ink));
        });
    }
}

/// The row under a comment's or reply's text: a Reply button at its end (the start is kept for
/// more of a reply's controls), and the box to type that reply in once it is pressed (Post, or
/// Ctrl/⌘+Enter, posts it; Enter starts a new line, as in the readers' pop-ups).
fn footer(ui: &mut egui::Ui, parts: &mut Parts<'_>, a: &Annotation) {
    let key = (a.page, a.index);
    // Small buttons, as the readers' footers have.
    ui.spacing_mut().button_padding = egui::vec2(6.0, 1.0);
    let mut opened = false;
    if parts.allowed {
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let label = egui::RichText::new(tl!("Reply")).font(theme::regular(11.0)).color(parts.pal.ink);
                let button = egui::Button::new(label)
                    .fill(Color32::WHITE.gamma_multiply(0.6))
                    .stroke(egui::Stroke::new(1.0, parts.pal.panel_edge))
                    .min_size(egui::vec2(0.0, 16.0));
                if ui.add(button).clicked() {
                    parts.cv.popup_reply = Some((a.page, a.index, String::new()));
                    opened = true;
                }
            });
        });
    }
    let Some(mut reply) = parts.cv.popup_reply.as_ref().filter(|(p, i, _)| (*p, *i) == key).map(|(_, _, s)| s.clone()) else { return };
    let field = ui.add(
        egui::TextEdit::multiline(&mut reply)
            .desired_rows(1)
            .desired_width(f32::INFINITY)
            .hint_text(tl!("Add a reply"))
            .font(theme::regular(12.5))
            .text_color(parts.pal.ink)
            .background_color(Color32::WHITE)
            .margin(egui::Margin::symmetric(4, 2))
            .id_salt(("popup-reply", a.page, a.index)),
    );
    if opened {
        field.request_focus();
    }
    if field.changed() {
        parts.cv.popup_reply = Some((a.page, a.index, reply.clone()));
    }
    let enter = super::submit_key(ui, &field, false);
    let ok = !reply.trim().is_empty();
    let (post, cancel) = ui
        .horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let small = |s: &str| egui::RichText::new(s).font(theme::regular(11.0));
                let post = ui.add_enabled(ok, egui::Button::new(small(tl!("Post"))).min_size(egui::vec2(0.0, 16.0))).clicked();
                let cancel = ui.add(egui::Button::new(small(tl!("Cancel"))).min_size(egui::vec2(0.0, 16.0))).clicked();
                (post, cancel)
            })
            .inner
        })
        .inner;
    if (post || enter) && ok {
        parts.cv.popup_reply = None;
        parts.edits.push(Edit::ReplyToAnnotation { page: a.page, index: a.index, text: reply.trim().to_string(), author: parts.author.to_string() });
    } else if cancel {
        parts.cv.popup_reply = None;
    }
}

/// A reply's panel: who wrote it and when, its text and footer, and the panels of its own
/// replies inside it.
fn reply_panel(ui: &mut egui::Ui, parts: &mut Parts<'_>, nodes: &[(&Annotation, Vec<usize>)], at: usize) {
    let Some((r, kids)) = nodes.get(at) else { return };
    let pal = parts.pal;
    egui::Frame::NONE.stroke(egui::Stroke::new(1.0, pal.panel_edge)).corner_radius(CornerRadius::same(3)).inner_margin(egui::Margin::same(4)).show(
        ui,
        |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 3.0;
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(m) = &r.modified {
                        ui.label(egui::RichText::new(m).font(theme::regular(10.5)).color(pal.faint));
                    }
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        let who = r.author.as_deref().unwrap_or("?");
                        ui.add(egui::Label::new(egui::RichText::new(who).font(theme::semibold(11.5)).color(pal.ink)).truncate());
                    });
                });
            });
            text_box(ui, parts, r, r.author.as_deref().unwrap_or("?"));
            footer(ui, parts, r);
            for kid in kids {
                egui::Frame::NONE.inner_margin(egui::Margin { left: NEST, ..Default::default() }).show(ui, |ui| reply_panel(ui, parts, nodes, *kid));
            }
        },
    );
}

/// The open pop-up windows of the comments on screen, laid out as the readers' are: a title bar
/// with the comment's kind and author, its date and ×; the comment's text, editable in place; its
/// replies as panels, each inside the one it answers; and a Reply button under each, which opens
/// a box to answer it. The window keeps its size on screen as the page zooms, and its text and
/// replies scroll inside it. The title bar drags the window and its lower right corner resizes
/// it, both saved with the comment; × closes it. Returns the edits to apply (one, or a batch when
/// a click both saves typed text and closes). Hidden comments show none.
pub(crate) fn popups(ctx: &egui::Context, view: &mut DocView, info: &DocInfo, prefs: &CommentPrefs, hidden: bool, allowed: bool) -> Option<Edit> {
    let pointer = ctx.input(|i| i.pointer.hover_pos());
    let mut edits: Vec<Edit> = Vec::new();
    let mut rects = Vec::new();
    let mut hovered = None;
    // The comment each reply files under, as the Comments panel threads them (by reference).
    let tops = pdfcraft_render::thread_tops(&info.annotations);
    let xforms: Vec<(usize, PageXform)> = view.page_xforms();
    for (page, xf) in xforms {
        let shown: Vec<Annotation> =
            info.annotations.iter().filter(|a| !hidden && a.page == page && has_popup(a) && a.popup.is_some_and(|p| p.open)).cloned().collect();
        for a in shown {
            let Some(p) = a.popup else { continue };
            let key = (page, a.index);
            let placed = xf.user_rect(info, page, p.rect);
            // Its place follows the page; its size is the pop-up's `/Rect` in points on screen,
            // or the size its corner is being dragged to.
            // A `/Rect` with numbers that aren't finite gives the least size.
            let side = |a: f32, b: f32| Some((b - a).abs()).filter(|v| v.is_finite()).unwrap_or(0.0);
            let saved = egui::vec2(side(p.rect[0], p.rect[2]), side(p.rect[1], p.rect[3]));
            let clamp = |s: egui::Vec2| egui::vec2(s.x.clamp(MIN_WIDTH, MAX_WIDTH), s.y.clamp(MIN_HEIGHT, MAX_HEIGHT));
            let resizing = view.comments.popup_resize.filter(|(rp, ri, _)| (*rp, *ri) == key).map(|(_, _, size)| size);
            let size = clamp(resizing.unwrap_or(saved));
            let dragging = view.comments.popup_drag.filter(|(dp, di, _)| (*dp, *di) == key).map(|(_, _, at)| at);
            let pos = dragging.unwrap_or(placed.left_top());
            let id = egui::Id::unique(("comment-popup", view.id.0, page, a.index));
            let color = a
                .color
                .map_or(Color32::from_rgb(0xFF, 0xD6, 0x5C), |c| Color32::from_rgb((c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8));
            let pal = palette(color);
            // A screen rectangle on this page in user space, to save as the pop-up's `/Rect`. Only
            // converts: the pointer and the redaction look are unused.
            let to_user = |r: Rect| {
                let cx = PageCx {
                    page,
                    xf: &xf,
                    ptr: &PagePointer::default(),
                    info,
                    tool: QuickTool::Select,
                    prefs: &CommentPrefs::default(),
                    allowed,
                    hidden,
                    redact_mark: ([0.0; 3], 1.0),
                };
                cx.rect_to_user(r)
            };
            let area = egui::Area::new(id).order(egui::Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
                let frame = egui::Frame::popup(ui.style()).inner_margin(egui::Margin::ZERO).corner_radius(CornerRadius::same(3)).fill(pal.body);
                let window = frame.stroke(egui::Stroke::new(1.0, pal.edge)).show(ui, |ui| {
                    ui.set_width(size.x);
                    // The whole size, however short its thread.
                    ui.set_min_height(size.y);
                    ui.spacing_mut().item_spacing.y = 3.0;
                    // Title bar: the comment's kind and author, its date and ×, on one line; it
                    // drags the window.
                    let bar = egui::Frame::NONE.fill(pal.title).inner_margin(egui::Margin::symmetric(6, 3)).show(ui, |ui| {
                        ui.set_width(size.x - 12.0);
                        ui.horizontal(|ui| {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                // Closing changes the comment's /Open, which needs comment permission.
                                if ui.add_enabled_ui(allowed, |ui| crate::icons::button(ui, "x", 14.0, false, tl!("Close"))).inner.clicked() {
                                    edits.push(Edit::SetAnnotationPopup { page, index: a.index, rect: None, open: Some(false) });
                                }
                                if let Some(m) = &a.modified {
                                    ui.label(egui::RichText::new(m).font(theme::regular(10.5)).color(pal.title_ink));
                                }
                                // The kind and author take the rest of the line, cut short to fit.
                                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                                    let kind = tl!(crate::comments_panel::kind_label(&a));
                                    let title = match a.author.as_deref() {
                                        Some(who) => format!("{kind} – {who}"),
                                        None => kind.to_string(),
                                    };
                                    ui.add(egui::Label::new(egui::RichText::new(title).font(theme::semibold(12.0)).color(pal.title_ink)).truncate());
                                });
                            })
                        });
                    });
                    // The bar drags the window, except where the × is: a region registered after
                    // the button would take its clicks.
                    let bar_rect = bar.response.rect;
                    let grip = ui.interact(
                        Rect::from_min_max(bar_rect.min, pos2(bar_rect.right() - 24.0, bar_rect.bottom())),
                        id.with("grip"),
                        egui::Sense::drag(),
                    );
                    if grip.hovered() || grip.dragged() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
                    }
                    if grip.drag_started() {
                        view.comments.popup_drag = Some((page, a.index, pos));
                    }
                    if grip.dragged()
                        && let Some((_, _, at)) = view.comments.popup_drag.as_mut()
                    {
                        *at += grip.drag_delta();
                    }
                    if grip.drag_stopped()
                        && let Some((_, _, at)) = view.comments.popup_drag.take()
                        && allowed
                        && at.distance(placed.left_top()) >= 2.0
                    {
                        // The same size on the page, at the new place. Only converts the
                        // rectangle to user space: the pointer and the redaction look are unused.
                        let rect = to_user(Rect::from_min_size(at, placed.size()));
                        edits.push(Edit::SetAnnotationPopup { page, index: a.index, rect: Some(rect), open: None });
                    }
                    // Replies, as the thread reads: text replies only, filed under this
                    // comment by `/IRT` reference as in the Comments panel, each under the
                    // one it answers. A review status or a checkmark (`/State`) is a reply too,
                    // but not one to read here.
                    let replies: Vec<&Annotation> = info
                        .annotations
                        .iter()
                        .filter(|r| {
                            r.state.is_none() && r.parent.is_some_and(|parent| tops.get(&(r.page, r.index)).copied().unwrap_or(parent) == key)
                        })
                        .collect();
                    let order = thread(key, &replies);
                    let (nodes, roots) = nest(&order);
                    let mut parts = Parts { allowed, author: &prefs.author, pal, cv: &mut view.comments, edits: &mut edits };
                    // The body keeps the window's height: the text and replies scroll in what the
                    // title bar leaves, with a bar that shows whenever there is more (egui's show
                    // only near the pointer), in the ink's colour.
                    egui::Frame::NONE.inner_margin(egui::Margin::same(4)).show(ui, |ui| {
                        ui.set_width(size.x - 8.0);
                        ui.spacing_mut().scroll = egui::style::ScrollStyle {
                            dormant_handle_opacity: 0.4,
                            dormant_background_opacity: 0.1,
                            foreground_color: true,
                            ..egui::style::ScrollStyle::solid()
                        };
                        ui.visuals_mut().override_text_color = Some(pal.ink);
                        let body_height = (size.y - bar_rect.height() - 8.0).max(24.0);
                        let scroll =
                            egui::ScrollArea::vertical().id_salt(("popup-body", page, a.index)).max_height(body_height).auto_shrink([false, false]);
                        scroll.show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 3.0;
                            text_box(ui, &mut parts, &a, tl!("Add a comment"));
                            footer(ui, &mut parts, &a);
                            for root in &roots {
                                reply_panel(ui, &mut parts, &nodes, *root);
                            }
                        });
                    });
                });
                // The lower right corner resizes the window; its new size is saved with the
                // comment, its upper left corner where it was, when the drag ends.
                let corner = window.response.rect.right_bottom();
                let grip =
                    ui.interact(Rect::from_min_max(corner - egui::vec2(RESIZE_GRIP, RESIZE_GRIP), corner), id.with("resize"), egui::Sense::drag());
                if grip.hovered() || grip.dragged() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeNwSe);
                }
                // Two short diagonal strokes mark the grip.
                for inset in [3.0, 7.0] {
                    let stroke = egui::Stroke::new(1.0, pal.faint);
                    ui.painter().line_segment([corner - egui::vec2(3.0, inset), corner - egui::vec2(inset, 3.0)], stroke);
                }
                if grip.drag_started() {
                    view.comments.popup_resize = Some((page, a.index, size));
                }
                if grip.dragged()
                    && let Some((_, _, s)) = view.comments.popup_resize.as_mut()
                {
                    *s += grip.drag_delta();
                }
                if grip.drag_stopped()
                    && let Some((_, _, s)) = view.comments.popup_resize.take()
                    && allowed
                {
                    let s = clamp(s);
                    // A press on the grip that ends where it began changes nothing.
                    // The window's size in points is the `/Rect`'s, its upper left corner kept
                    // where it is on screen: the rectangle of that size on the page there. A page
                    // turned a quarter (its `/Rotate` and the view's) shows the `/Rect`'s width
                    // down the screen.
                    let zoom = xf.zoom();
                    if (s - clamp(saved)).length() >= 2.0 && zoom.is_finite() && zoom > 0.0 {
                        let turn = info.pages.get(page).map_or(0, |pg| u32::from(pg.rotation)) + u32::from(xf.rot);
                        let on_page = if turn % 180 == 90 { egui::vec2(s.y, s.x) } else { s } * zoom;
                        let rect = to_user(Rect::from_min_size(placed.left_top(), on_page));
                        edits.push(Edit::SetAnnotationPopup { page, index: a.index, rect: Some(rect), open: None });
                    }
                }
            });
            let shown_at = area.response.rect;
            if pointer.is_some_and(|p| shown_at.contains(p)) {
                hovered = Some(key);
            }
            rects.push((key, shown_at));
        }
    }
    view.comments.popup_rects = rects;
    view.comments.popup_hovered = hovered;
    match edits.len() {
        0 => None,
        1 => edits.pop(),
        _ => Some(Edit::Batch { label: "Edit comment".into(), edits }),
    }
}
