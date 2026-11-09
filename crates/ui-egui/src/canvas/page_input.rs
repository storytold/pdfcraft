//! Page input: what the tools do with the pointer on one page, what lies under it for the page
//! router, the text layer, and what is drawn over the page after its text.

use egui::{Color32, CornerRadius, Pos2, Rect, Stroke};
use pdfcraft_render::{DocInfo, PageText};

use super::{
    DocView, MARK_ICON_SHARE, PageEnv, PageFrame, PageSlot, PageXform, Selection, Span, cached_edit_images, click_unit, mark_anchor,
    signature_badges, verdict_mark_rect,
};
use crate::page_cursor::{Layer, Subject};
use crate::page_pointer::PagePointer;
use crate::page_router::{Owner, PagePress, top};
use crate::{QuickTool, comments, icons};

/// What the page's tools did with the pointer this frame.
pub(super) struct PageInput {
    /// What the press the pages hold is to this page.
    pub(super) press: PagePress,
    /// The press's drag is the page's text's: nobody holds it, or its owner lets its drag go (text
    /// markup: a click selects it, a drag selects the text under it).
    pub(super) text_drag: bool,
    /// A tool, a field or a comment took the pointer this frame (its own press, or a gesture of the
    /// active tool's): text selection and image pick leave it alone.
    pub(super) consumed: bool,
    /// Redact or Highlight draws boxes off text.
    pub(super) area_tool: bool,
    /// What a press under the pointer would take (the top hit there), for hover painting.
    pub(super) hover: Option<crate::page_router::Hit>,
    /// The press's click came up over what the press began on: a link's or a field's click counts
    /// only then, as a button's does.
    pub(super) released_over: bool,
}

/// Common geometry and pointer state for a tool acting on one page.
#[derive(Clone, Copy)]
pub(crate) struct PageToolContext<'a> {
    pub ui: &'a egui::Ui,
    pub resp: &'a egui::Response,
    pub xf: &'a PageXform,
    pub ptr: &'a PagePointer,
    pub page: usize,
    pub doc: &'a pdfcraft_engine::Document,
    pub info: &'a DocInfo,
}

/// The tools' input on one page. The hits at the press point decide who holds the press, and the
/// modules are told what the press is to this page (`PagePress`): their own, nobody's (the active
/// tool's: `tool_input`, and the tools still called here), or another page's. Edit text's group
/// paints from what lies under the pointer too (`hovered`). Text selection runs after, on what
/// nobody took.
pub(super) fn page_tool_input(ui: &mut egui::Ui, view: &mut DocView, frame: &mut PageFrame<'_>, slot: &PageSlot) -> PageInput {
    let PageEnv {
        added,
        allowed,
        can_fill,
        can_modify,
        comments_hidden,
        doc,
        doc_links,
        editing_content,
        form,
        info,
        placing,
        prefs,
        preparing,
        redact_mark,
        table_preview,
        text_style,
        tool,
        ..
    } = frame.env;
    let PageSlot { i, xf, .. } = *slot;
    let PageFrame { resp, ptr, pointer, .. } = *frame;
    let cx = PageToolContext { ui, resp, xf: &xf, ptr, page: i, doc, info };
    let pcx = comments::PageCx { page: i, xf: &xf, ptr, info, tool, prefs, allowed, hidden: comments_hidden, redact_mark };
    let (lines, images) =
        if matches!(tool, QuickTool::AddText | QuickTool::EditText) && can_modify { edit_inventory(view, doc, i) } else { Default::default() };
    let field_tool = match tool {
        QuickTool::Field(f) => Some(f),
        _ => None,
    };
    // While a table is being set up, nothing else is drawn or moved.
    let prepare_allowed = can_modify && table_preview.is_none() && frame.out.open_table.is_none();
    let hits = PageHits {
        page: i,
        xf: &xf,
        info,
        tool,
        pcx: &pcx,
        form,
        can_fill,
        can_modify,
        editing_content,
        preparing,
        field_tool,
        prepare_allowed,
        added,
        doc_links,
        lines: &lines,
        images: &images,
        clip: ui.clip_rect(),
    };
    if let Some(o) = ptr.pressed() {
        // A press with Shift or ⌘ in Edit text is the objects', whatever lies under it
        // (`object_ui::modified_hit`): the page's objects are read for it alone. Crop mode holds
        // it on its page as any other press, so a modified click away applies the crop rather
        // than taking the picture into a group (#6422).
        let adds = tool == QuickTool::EditText
            && can_modify
            && xf.rect.contains(o)
            && !hits.cropping(view)
            && crate::object_ui::adds(ptr.modifiers)
            && !crate::object_ui::menu_open(ui, resp);
        let objects = adds.then(|| view.objects.inventory(doc, i));
        let at = hits.at(view, o, objects.as_ref());
        // A press is the page's it lies on, or past its edge the one whose selected picture has its
        // rotate knob or a grip there (crop mode's grips too).
        let past_edge = |h: &crate::page_router::Hit| {
            use crate::page_router::PicturePart;
            matches!(h.owner, Owner::Picture { part: PicturePart::Knob | PicturePart::Corner(_) | PicturePart::Crop(_), .. })
        };
        if xf.rect.contains(o) || at.iter().any(past_edge) {
            view.router.press(i, at, tool);
        }
    }
    // A drag that the press's owner declines is nobody's once it starts.
    if ptr.drag_started {
        view.router.drag_began();
    }
    // What this page has to do with the press: its owner's, the active tool's, or another page's.
    let press = view.router.press_for(i);
    // What lies under the pointer before the tools act, on the frames that need it: for a click's
    // release, a right-click's tool checks, Fill & Sign's preview, and Edit text's hover outlines.
    let editing = matches!(tool, QuickTool::AddText | QuickTool::EditText) && can_modify;
    let wants_under = editing || resp.clicked() || resp.secondary_clicked() || matches!(tool, QuickTool::Fill(_)) && !ptr.any_down;
    let under = pointer.filter(|p| wants_under && xf.rect.contains(*p)).map(|p| hits.at(view, p, None)).unwrap_or_default();
    let hovered = top(under.iter().cloned(), tool);
    // A double-click on the objects selected together is the object's its press was on: the
    // double-click leaves the group (`object_ui`), and added text opens for typing, a picture or a
    // paragraph is the one taken (the user's ruling).
    let press = match press {
        PagePress::Mine(Owner::Objects { part: crate::page_router::ObjectsPart::Selection }) if resp.double_clicked() => {
            view.router.under_objects().map_or(PagePress::Free, PagePress::Mine)
        }
        press => press,
    };
    let released_over = match press {
        PagePress::Mine(owner) if resp.clicked() => under.iter().any(|h| h.owner == owner),
        _ => false,
    };
    // A right-click while the primary button is held (a comment moved, a box drawn) leaves the
    // selections that press acts on alone. egui ends the press's own interaction when any button is
    // released, the right-click's own frame included, so the button is what tells. A touch's long
    // press, which egui reports as a right-click with the finger still down, opens its menu.
    let press_held = ptr.primary_down && !resp.long_touched();
    // A right-click is the page's it lies on, or past its edge the one whose crop grip lies there,
    // as a press is (#5835): what a right-click there would go to is asked only off the page.
    let right = frame.right_click_on(|p| {
        use crate::page_router::PicturePart;
        xf.rect.contains(p) || matches!(hits.context_at(view, p, &[]), Some(Owner::Picture { part: PicturePart::Crop(_), .. }))
    });
    let context = right.and_then(|p| right_click(&hits, &under, view, p, press_held));
    // Edit text: a right-click on a picture selects it for its menu, and one on anything else on the
    // page unselects it.
    if right.is_some() && tool == QuickTool::EditText && can_modify && !press_held {
        let picture = match context {
            Some(Owner::Picture { index, .. }) => Some(index),
            _ => None,
        };
        crate::edit_text_ui::picture_context(i, picture, view);
    }
    // A mark already placed takes its own press (comments, below), as does one being moved on this
    // page until its move ends; with no button down the Fill & Sign tool's preview hides where a
    // press would be taken (a mark, its handles, a Prepare field).
    let over_mark = matches!(tool, QuickTool::Fill(_))
        && (matches!(press, PagePress::Mine(_)) || comments::moving_on(view, i) || !ptr.any_down && hovered.is_some());
    tool_input(ui, view, frame, slot, &pcx, press, over_mark);
    let on_field = if preparing && !placing {
        let o = crate::prepare::page_input(cx, form, field_tool, prepare_allowed, press, view);
        frame.out.field_props |= o.properties;
        frame.out.field_placed |= o.placed;
        frame.out.open_table = frame.out.open_table.or(o.table);
        o.consumed || field_tool.is_some()
    } else {
        tool == QuickTool::Select && !placing && crate::forms_ui::page_input(cx, form, can_fill, press, released_over, view)
    };
    // Redact draws boxes off text; so does Highlight (an area highlight, as in Acrobat).
    let area_tool = tool == QuickTool::Redact && can_modify || tool == QuickTool::Comment(comments::CommentTool::Highlight) && allowed;
    let boxing = area_tool && {
        let text = view.page_text(i);
        let over_text = |p: Pos2| {
            let (vx, vy) = xf.screen_to_view(p);
            text.as_ref().is_some_and(|t| t.glyph_at(vx, vy).is_some())
        };
        crate::redact_ui::page_input(cx, over_text, press == PagePress::Free, view)
    };
    // Edit text's group: the objects selected together, added content, the paragraphs and the
    // pictures, each told what the press is to this page. Edit text ignores input while a dialog,
    // the palette, another window or another tab has it (`gate`), as before.
    let gate = !(tool == QuickTool::EditText && view.objects.input_blocked());
    if tool == QuickTool::EditText && can_modify && gate {
        crate::edit_text_ui::picture_keys(ui.ctx(), view, i, &images, ptr.any_down);
    }
    let on_objects = tool == QuickTool::EditText && can_modify && crate::object_ui::page_input(cx, press, view);
    let picks_up = matches!(tool, QuickTool::Select | QuickTool::AddText | QuickTool::EditText);
    let ccx = crate::content_ui::ContentCx {
        added,
        press,
        gate,
        picks_up,
        adding_text: tool == QuickTool::AddText,
        edit_on_click: matches!(tool, QuickTool::AddText | QuickTool::EditText),
        style: text_style,
    };
    let on_content = !placing && editing_content && can_modify && crate::content_ui::page_input(cx, ccx, view);
    // Add text acts on the page's own paragraphs: the one its press was on, until the button is up,
    // and the one under the pointer. Edit text boxes every paragraph and picture.
    let paragraph = |o: Option<Owner>| matches!(o, Some(Owner::Paragraph { .. }));
    let source = tool == QuickTool::AddText && (paragraph(press.owner()) || paragraph(hovered.as_ref().map(|h| h.owner)));
    let on_edit_text = (tool == QuickTool::EditText || source) && can_modify && {
        let adding = (tool == QuickTool::AddText).then_some(added.as_slice());
        let pcx = crate::edit_text_ui::ParagraphCx { press, gate, hover: hovered.as_ref(), adding };
        let text_used = crate::edit_text_ui::page_input(cx, &lines, pcx, view);
        let image_cx = crate::edit_text_ui::ImageCx { ui, resp, xf: &xf, ptr, page: i, doc, info, press, gate };
        let image_used = crate::edit_text_ui::image_input(&image_cx, &images, view);
        text_used || image_used
    };
    if tool == QuickTool::EditText {
        // A press the objects or added content took leaves the picture unselected; so does one
        // while Edit text's input is blocked. Otherwise added content follows the picture selected.
        if on_objects || on_content {
            if ptr.pressed().is_some() || resp.clicked() || resp.secondary_clicked() {
                view.image_selection = None;
            }
        } else {
            crate::content_ui::sync_image_selection(view, i, &images, added);
        }
    }
    let (link_owner, link_context) = (link_of(press.owner()), link_of(context));
    let on_link = tool == QuickTool::Link && can_modify && crate::link_ui::page_input(cx, press == PagePress::Free, link_owner, link_context, view);
    if tool == QuickTool::Select
        && can_modify
        && let Some(index) = link_context
    {
        view.links.context = Some((i, index));
    }
    // Comments, every frame: their own presses, a commenting tool's, and a click elsewhere that
    // leaves them unselected. A press nobody holds that another tool took this frame (Prepare's
    // selection rectangle, added content) is not also a commenting tool's, as before.
    let taken = on_objects || on_edit_text || on_link || on_content || boxing || on_field;
    let on_comment = comments::page_input(ui, resp, &pcx, press, taken, view);
    let consumed = taken || on_comment;
    let hover = pointer.and_then(|p| claim_under(&hits, view, p, frame.env.unobstructed));
    PageInput { press, text_drag: !view.router.keeps_drag(), consumed, area_tool, hover, released_over }
}

/// What lies under the pointer at `p`, after the tools act: its top, what a press there would take,
/// for hover painting. With `claim` (nothing drawn over the pages, such as the palette), its cursor
/// and tip too, after the tools claimed theirs (the last claim on a layer stands).
fn claim_under(hits: &PageHits<'_>, view: &mut DocView, p: Pos2, claim: bool) -> Option<crate::page_router::Hit> {
    let under = hits.at(view, p, None);
    if claim {
        for h in &under {
            view.cursor.claim_with_tip(h.layer, h.icon, h.tip.clone());
        }
    }
    top(under, hits.tool)
}

/// The link `owner` is, by its `/Annots` index.
fn link_of(owner: Option<Owner>) -> Option<usize> {
    match owner {
        Some(Owner::Link { index }) => Some(index),
        _ => None,
    }
}

/// The active tool's own input on page `i`: the one gate for presses nobody holds. Measure, the
/// stamps, Fill & Sign, Sign area, zoom and snapshot, the camera's quadrilateral and crop act only
/// on a press that began on this page and that no hit won (`press` is Free), and keep their cursor
/// and gestures. The camera also drags a corner whose press it holds, and crop's double-click
/// opens Set Page Boxes wherever it is. `over_mark`: with Fill & Sign, a hit holds the press (a
/// mark's, or a Prepare field's), a mark is being moved on this page, or with no button down a press
/// under the pointer would be taken (a mark, its handles, a Prepare field).
fn tool_input(
    ui: &egui::Ui,
    view: &mut DocView,
    frame: &mut PageFrame<'_>,
    slot: &PageSlot,
    pcx: &comments::PageCx<'_>,
    press: PagePress,
    over_mark: bool,
) {
    let PageEnv {
        allowed,
        author,
        by_line,
        can_crop,
        can_modify,
        custom_stamp,
        date_text,
        doc,
        info,
        initials,
        signature,
        tool,
        units,
        fill_text_size,
        fill_text_color,
        fill_text_border,
        ..
    } = frame.env;
    let PageSlot { i, xf, .. } = *slot;
    let PageFrame { resp, ptr, .. } = *frame;
    let cx = PageToolContext { ui, resp, xf: &xf, ptr, page: i, doc, info };
    let free = press == PagePress::Free;
    if let QuickTool::Measure(measure_tool) = tool {
        crate::measure_ui::page_input(ui, crate::measure_ui::Clicks::of(resp, free), doc, view, pcx, measure_tool, units);
    }
    if let QuickTool::Stamp(kind) = tool
        && allowed
        && crate::stamps_ui::place(cx, kind, by_line, author, free, view)
    {
        frame.out.stamp_placed = true;
    }
    if let Some(cs) = custom_stamp.as_ref()
        && allowed
        && crate::stamps_ui::place_custom(cx, cs, author, free, view)
    {
        frame.out.stamp_placed = true;
    }
    if let QuickTool::Fill(ft) = tool
        && allowed
        && !over_mark
    {
        let input = crate::fill_sign::PageInput {
            tool: ft,
            text_size: fill_text_size,
            text_color: fill_text_color,
            text_border: fill_text_border,
            signature: signature.as_ref(),
            initials: initials.as_ref(),
            preview: frame.signature_preview,
            author,
            date_text,
        };
        match crate::fill_sign::page_input(cx, input, free, view) {
            Some(crate::fill_sign::FillAction::Edit(e)) => view.pending_edit = Some(*e),
            Some(crate::fill_sign::FillAction::Signature(e)) => {
                view.pending_edit = Some(*e);
                view.fill_signature_page = Some(i);
            }
            Some(crate::fill_sign::FillAction::CreateSignature) => frame.out.open_signature = true,
            Some(crate::fill_sign::FillAction::CreateInitials) => frame.out.open_initials = true,
            Some(crate::fill_sign::FillAction::Refused(why)) => frame.out.refused = Some(why),
            None => {}
        }
    }
    if matches!(tool, QuickTool::SignArea { .. }) {
        crate::sign_ui::page_input(cx, free, view);
    }
    if matches!(tool, QuickTool::MarqueeZoom | QuickTool::Snapshot) {
        crate::zoom_snap::page_input(ui, resp, &xf, ptr, i, free, view);
    }
    if tool == QuickTool::DynamicZoom {
        crate::zoom_snap::dynamic_zoom_input(ui, resp, &xf, ptr, i, free, view);
    }
    if tool == QuickTool::EnhanceCamera {
        crate::camera_ui::page_input(resp, &xf, ptr, i, view, can_modify, press);
    }
    if tool == QuickTool::Crop
        && crate::crop::page_input(ui, crate::crop::CropPageInput { resp, xf: &xf, ptr, page: i, info, view, allowed: can_crop, free })
    {
        view.current = i;
        frame.out.open_boxes = true;
    }
}

/// A right-click at `p`: the page's menu opens there, and it goes to what lies on top there
/// ([`PageHits::context_at`]), which the menu then acts on. A comment there is selected, and none
/// elsewhere; a Prepare field is selected; links and Edit text's pictures take theirs from what this
/// returns.
/// `under`: the hits at `p` before the tools act. `press_held`: the primary button is held, and the
/// comment and Prepare selections its press acts on are left alone until its release; links still
/// take theirs, as before.
fn right_click(hits: &PageHits<'_>, under: &[crate::page_router::Hit], view: &mut DocView, p: Pos2, press_held: bool) -> Option<Owner> {
    let context = hits.context_at(view, p, under);
    // Delete page, Paste and Copy Image act where the menu opened, whoever takes the right-click.
    comments::anchor(hits.pcx, p, view);
    // The menu is what the right-click went to (Add text's is the page's), when that is selected:
    // under a held press, the selections stay as they are.
    view.menu_for = context.filter(|_| !(hits.tool == QuickTool::AddText && hits.can_modify)).map(|o| (hits.page, o));
    if press_held {
        return context;
    }
    // Add text takes every right-click on a page it can change, the Link tool those on a link, and a
    // Prepare field tool the whole page, as before: comments see none of them.
    let tool_takes_it = match hits.tool {
        QuickTool::AddText => hits.can_modify,
        QuickTool::Link => hits.can_modify && matches!(context, Some(Owner::Link { .. })),
        _ => hits.field_tool.is_some(),
    };
    if !tool_takes_it {
        let comment = match context {
            Some(Owner::Comment { index, .. }) => Some(index),
            _ => None,
        };
        comments::context(hits.pcx, comment, view);
    }
    if let Some(Owner::PrepareField { field, widget }) = context {
        crate::prepare::context(hits.form, field, widget, view);
    }
    context
}

/// Edit text's paragraphs and pictures on page `i`, kept while the document is unchanged.
fn edit_inventory(
    view: &mut DocView,
    doc: &pdfcraft_engine::Document,
    i: usize,
) -> (Vec<pdfcraft_engine::TextBlock>, Vec<pdfcraft_engine::PageImage>) {
    let generation = doc.edit_generation();
    let lines = match view.edit_lines.get(&i) {
        Some((g, l)) if *g == generation => l.clone(),
        _ => {
            let l = doc.text_blocks(i);
            view.edit_lines.insert(i, (generation, l.clone()));
            l
        }
    };
    let images = cached_edit_images(view, doc, i).to_vec();
    (lines, images)
}

/// What the page modules have under a point on one page, for the page router.
struct PageHits<'a> {
    page: usize,
    xf: &'a PageXform,
    info: &'a DocInfo,
    tool: QuickTool,
    pcx: &'a comments::PageCx<'a>,
    form: &'a [pdfcraft_engine::FormField],
    can_fill: bool,
    can_modify: bool,
    /// Edit a PDF is open: added text and pictures can be picked up.
    editing_content: bool,
    preparing: bool,
    field_tool: Option<crate::prepare::FieldTool>,
    /// Prepare may draw and move fields (not while a table is being set up).
    prepare_allowed: bool,
    added: &'a [pdfcraft_engine::Added],
    /// The document's links, as the engine lists them (the Link tool edits every one).
    doc_links: &'a [pdfcraft_engine::LinkItem],
    lines: &'a [pdfcraft_engine::TextBlock],
    images: &'a [pdfcraft_engine::PageImage],
    /// The pages' visible area (the picture's rotate knob stays inside it).
    clip: Rect,
}

impl PageHits<'_> {
    /// Edit text's crop mode is on for a picture on this page.
    fn cropping(&self, view: &DocView) -> bool {
        self.tool == QuickTool::EditText && self.can_modify && view.image_selection.as_ref().is_some_and(|s| s.page == self.page && s.crop.is_some())
    }

    /// The hits at `p`, bottom up ([`Self::hits`]). For a press with Shift or ⌘ in Edit text, the
    /// page's `objects` ([`crate::object_ui::ObjectSelection::inventory`]), which take it whole.
    fn at(&self, view: &DocView, p: Pos2, objects: Option<&Result<Vec<pdfcraft_engine::EditableObject>, String>>) -> Vec<crate::page_router::Hit> {
        self.hits(view, p, false, objects)
    }

    /// What a right-click at `p` is on: what lies on top there, as for a press, but with every
    /// tool reaching comments, as a comment's menu shows whatever the tool, and a Prepare field
    /// whatever the field tool. Links count for the tools that reach them (Select; the Link tool,
    /// below). Edit text's paragraphs have no menu, and give way to the picture under them (the
    /// user's ruling): with Edit text, a picture under a link or a paragraph takes the right-click.
    /// Nor have added content or an open paragraph's frame: those give way to a picture under them,
    /// and to nothing else. (Where objects are selected together, the page view unselects the picture
    /// a right-click selects, and the menu is the page's.)
    /// A highlight over a link is the highlight's ([`comments::context_hit`]).
    /// The Link tool edits links and reaches no comment: on a document it can change, its
    /// right-click is what its press there would take (`under`, the hits at `p`), when that is
    /// anything.
    fn context_at(&self, view: &DocView, p: Pos2, under: &[crate::page_router::Hit]) -> Option<Owner> {
        if self.tool == QuickTool::Link
            && self.can_modify
            && let Some(press) = top(under.iter().cloned(), QuickTool::Link)
        {
            return Some(press.owner);
        }
        let reached: Vec<_> = self
            .hits(view, p, true, None)
            .into_iter()
            .filter(|h| (h.layer != Layer::Link || h.layer.reached_by(self.tool)) && !matches!(h.owner, Owner::Paragraph { .. }))
            .collect();
        let on_top = top(reached.iter().cloned(), QuickTool::Select)?.owner;
        if self.tool == QuickTool::EditText && matches!(on_top, Owner::Added { .. } | Owner::Objects { part: crate::page_router::ObjectsPart::Frame })
        {
            let pictures = reached.into_iter().filter(|h| matches!(h.owner, Owner::Picture { .. }));
            return Some(top(pictures, QuickTool::Select).map_or(on_top, |h| h.owner));
        }
        Some(on_top)
    }

    /// The hits at `p`, bottom up: links, comments; Edit text's pictures under its paragraphs (text
    /// on top of artwork takes the press), added content, the objects selected together and the
    /// open paragraph's frame, the selected picture's grips over them all; the camera tool's corners;
    /// then form fields (Prepare's, being moved, over the comments; filled ones under added content
    /// and the comments that move). For a press with Shift or ⌘ in Edit text, only the objects
    /// (`objects`, the page's); in crop mode, only the picture being cropped, which a right-click
    /// reaches only on the crop.
    /// For a right-click (`context`), comments by [`comments::context_hit`], and no camera corner (it
    /// has no menu).
    fn hits(
        &self,
        view: &DocView,
        p: Pos2,
        context: bool,
        objects: Option<&Result<Vec<pdfcraft_engine::EditableObject>, String>>,
    ) -> Vec<crate::page_router::Hit> {
        let PageHits {
            page,
            xf,
            info,
            tool,
            pcx,
            form,
            can_fill,
            can_modify,
            editing_content,
            preparing,
            field_tool,
            prepare_allowed,
            added,
            doc_links,
            lines,
            images,
            clip,
        } = *self;
        // A press with Shift or ⌘ is the objects' (`object_ui::modified_hit`), off crop mode's page.
        if let Some(objects) = objects {
            return crate::object_ui::modified_hit(view, xf, info, page, objects, clip, p).into_iter().collect();
        }
        // Crop mode holds every press on its page (`edit_text_ui::crop_hit`). A right-click is the
        // crop's only on the picture being cropped, its box or its grips (#5835), where it keeps crop
        // mode and opens no menu; elsewhere it goes to what lies there, as without crop mode.
        if self.cropping(view) {
            use crate::page_router::{CropPart, PicturePart};
            let crop = crate::edit_text_ui::crop_hit(xf, page, info, images, view, p);
            let on_crop = crop.as_ref().is_some_and(|h| !matches!(h.owner, Owner::Picture { part: PicturePart::Crop(CropPart::Page), .. }));
            if !context || on_crop {
                return crop.into_iter().collect();
            }
        }
        // A right-click reaches a Prepare field whatever the field tool, and while a table is set up,
        // on a document that can be changed.
        let (field_tool, prepare_allowed) = if context { (None, can_modify) } else { (field_tool, prepare_allowed) };
        let fields = if preparing {
            crate::prepare::hit(xf, info, page, form, field_tool, prepare_allowed, &view.prepare, p)
        } else if tool == QuickTool::Select {
            crate::forms_ui::hit(xf, info, page, form, can_fill, p)
        } else {
            None
        };
        // With Add text, only the paragraphs and pictures not under added content.
        let editing =
            can_modify && (tool == QuickTool::EditText || tool == QuickTool::AddText && !crate::content_ui::hits_added(xf, info, page, added, p));
        let (pictures, paragraphs, objects, grips) = if editing {
            let open = view.line_editor.as_ref().filter(|e| e.page == page);
            let frame = open
                .filter(|e| !e.has_changes())
                .and_then(|e| lines.get(e.block))
                .map(|l| xf.user_rect(info, page, l.rect.map(|v| v as f32)).expand(2.0));
            // Add text acts on the page's paragraphs only in their boxes, edges included (its click
            // there finishes the box being typed and opens the paragraph, `ParagraphCx::adding`):
            // not on pictures, nor beside a paragraph, where a click starts a new box.
            let edit_text = tool == QuickTool::EditText;
            let on_paragraphs = edit_text || crate::edit_text_ui::over_paragraph(xf, info, page, lines, p);
            (
                edit_text.then(|| crate::edit_text_ui::picture_hit(xf, page, info, images, view, p)).flatten(),
                on_paragraphs.then(|| crate::edit_text_ui::hit(xf, info, page, lines, open.map(|e| e.block), p)).flatten(),
                edit_text.then(|| crate::object_ui::hit(view, xf, info, page, frame, p)).flatten(),
                edit_text.then(|| crate::edit_text_ui::picture_handle_hit(clip, xf, page, info, images, view, p)).flatten(),
            )
        } else {
            (None, None, None, None)
        };
        // Added text and pictures, over the page's own paragraphs and under the objects selected
        // together.
        let picks_up = matches!(tool, QuickTool::Select | QuickTool::AddText | QuickTool::EditText);
        let added = (editing_content && can_modify && picks_up).then(|| crate::content_ui::hit(xf, info, page, added, view, p)).flatten();
        let corner = (!context && tool == QuickTool::EnhanceCamera && can_modify).then(|| crate::camera_ui::hit(xf, page, view, p)).flatten();
        let link = crate::link_ui::hit(xf, info, page, doc_links, tool, p);
        let comment = if context { comments::context_hit(pcx, p) } else { comments::hit(pcx, view, p, link.is_some()) };
        [link, comment, pictures, paragraphs, added, objects, grips, corner, fields].into_iter().flatten().collect()
    }
}

/// Text layer: find matches, selection, I-beam and drag-to-select.
pub(super) fn text_layer(ui: &mut egui::Ui, view: &mut DocView, frame: &mut PageFrame<'_>, slot: &PageSlot, input: &PageInput) {
    let PageEnv { selects_text, tool, .. } = frame.env;
    let PageSlot { i, r, xf } = *slot;
    let PageFrame { resp, ptr, pointer, .. } = *frame;
    if let Some(text) = view.texts.get(&i).cloned() {
        paint_text_marks(ui, view, &text, i, xf, resp.dragged());
        if selects_text
            && !input.consumed
            && let Some(p) = pointer.filter(|p| r.contains(*p))
        {
            // The I-beam drops where a press would go to a link (ruling E: the tools that select a
            // link's text show it there).
            let over_link = input.hover.as_ref().is_some_and(|h| matches!(h.owner, Owner::Link { .. }));
            let pointer = TextPointer { page: i, xf, rect: r, tool, resp, ptr: ptr.clone(), pointer: p, over_link, text_drag: input.text_drag };
            text_pointer_input(ui, view, &text, pointer);
        }
    }
}

fn paint_text_marks(ui: &egui::Ui, view: &DocView, text: &PageText, page: usize, xf: PageXform, dragging: bool) {
    let painter = ui.painter();
    let to_screen = |g: [f32; 4]| xf.view_rect(g);
    if let Some(find) = &view.find {
        for (index, (match_page, range)) in find.matches.iter().enumerate() {
            if *match_page != page {
                continue;
            }
            let fill = if find.current == Some(index) {
                Color32::from_rgba_unmultiplied(255, 140, 0, 110)
            } else {
                Color32::from_rgba_unmultiplied(255, 214, 0, 90)
            };
            for line_rect in text.line_rects(range.clone()) {
                painter.rect_filled(to_screen(line_rect).expand(1.0), CornerRadius::same(2), fill);
            }
        }
    }
    if view.all_text {
        for line_rect in text.line_rects(0..text.glyphs.len()) {
            painter.rect_filled(to_screen(line_rect), CornerRadius::same(1), Color32::from_rgba_unmultiplied(0x3A, 0x7B, 0xF0, 70));
        }
    }
    if let Some(selection) = view.selection.filter(|selection| selection.page == page) {
        for line_rect in selection.rects(text) {
            painter.rect_filled(to_screen(line_rect), CornerRadius::same(1), Color32::from_rgba_unmultiplied(0x3A, 0x7B, 0xF0, 70));
        }
        if let Span::Column { rect } = selection.span
            && dragging
        {
            let outline = Stroke::new(1.0, Color32::from_rgb(0x3A, 0x7B, 0xF0));
            painter.rect_stroke(to_screen(rect), CornerRadius::ZERO, outline, egui::StrokeKind::Middle);
        }
    }
}

struct TextPointer<'a> {
    page: usize,
    xf: PageXform,
    rect: Rect,
    tool: QuickTool,
    resp: &'a egui::Response,
    ptr: PagePointer,
    pointer: Pos2,
    /// A press under the pointer would go to a link.
    over_link: bool,
    /// The press's drag is the page's text's ([`PageInput::text_drag`]).
    text_drag: bool,
}

struct TextGesture<'a> {
    page: usize,
    xf: PageXform,
    rect: Rect,
    tool: QuickTool,
    ptr: PagePointer,
    resp: &'a egui::Response,
    pointer: Pos2,
    vx: f32,
    vy: f32,
    over_text: bool,
    over_link: bool,
    text_drag: bool,
    column: bool,
}

fn text_pointer_input(ui: &mut egui::Ui, view: &mut DocView, text: &PageText, pointer: TextPointer<'_>) {
    let TextPointer { page, xf, rect, tool, resp, ptr, pointer: screen_pointer, over_link, text_drag } = pointer;
    let (vx, vy) = xf.screen_to_view(screen_pointer);
    let over_text = text.glyph_at(vx, vy).is_some();
    let column = tool == QuickTool::ColumnSelect || ui.input(|input| input.modifiers.alt);
    let gesture = TextGesture { page, xf, rect, tool, ptr, resp, pointer: screen_pointer, vx, vy, over_text, over_link, text_drag, column };
    update_text_cursor(view, &gesture);
    update_selection_on_press(ui, view, text, &gesture);
    update_dragged_selection(view, text, &gesture);
    finish_text_pointer_gesture(ui, view, text, &gesture);
}

fn update_text_cursor(view: &mut DocView, gesture: &TextGesture<'_>) {
    // A press on a link selects nothing (it is the link's), so no cursor here says it would.
    if gesture.column && !gesture.over_link {
        view.cursor.claim(Layer::Text, egui::CursorIcon::Crosshair);
    } else if gesture.over_text && !gesture.over_link {
        view.cursor.claim(Layer::Text, egui::CursorIcon::Text);
    }
}

fn update_selection_on_press(ui: &egui::Ui, view: &mut DocView, text: &PageText, gesture: &TextGesture<'_>) {
    // A press held by what keeps its drag (a link, a comment that moves) selects no text; one on
    // text markup lets its drag select the text underneath.
    let press_here = gesture.ptr.origin.is_some_and(|origin| gesture.rect.contains(origin));
    if gesture.ptr.pressed().is_some() && press_here && gesture.text_drag {
        let now = ui.input(|input| input.time);
        let (max_delay, max_dist) = ui.ctx().options(|options| (options.input_options.max_double_click_delay, options.input_options.max_click_dist));
        let consecutive = now - view.last_press_time <= max_delay && view.last_press_pos.is_some_and(|pos| pos.distance(gesture.pointer) <= max_dist);
        view.clicks = if consecutive { view.clicks.saturating_add(1) } else { 1 };
        view.last_press_time = now;
        view.last_press_pos = Some(gesture.pointer);
        let clicks = selection_click_count(view.clicks, gesture.tool);
        if clicks >= 2
            && gesture.over_text
            && let Some((first, last)) = text.nearest(gesture.vx, gesture.vy).and_then(|anchor| click_unit(text, clicks, anchor))
        {
            view.selection = Some(Selection::run(gesture.page, first, last));
        }
    }
    if gesture.ptr.drag_started && press_here && gesture.text_drag {
        view.actions_dismissed = None;
        view.selection_drag = true;
        let origin = gesture.ptr.origin.unwrap_or(gesture.pointer);
        let (ox, oy) = gesture.xf.screen_to_view(origin);
        if view.clicks < 2 {
            view.selection = if gesture.column {
                Some(Selection { page: gesture.page, span: Span::Column { rect: [ox, oy, gesture.vx, gesture.vy] } })
            } else {
                text.nearest(ox, oy).map(|anchor| Selection::run(gesture.page, anchor, anchor))
            };
        }
    }
}

fn selection_click_count(clicks: u32, tool: QuickTool) -> u32 {
    if clicks > 2 && tool != QuickTool::Select { 1 } else { clicks }
}

fn update_dragged_selection(view: &mut DocView, text: &PageText, gesture: &TextGesture<'_>) {
    let clicks = selection_click_count(view.clicks, gesture.tool);
    if gesture.resp.dragged_by(egui::PointerButton::Primary)
        && let Some(selection) = view.selection.as_mut().filter(|selection| selection.page == gesture.page)
    {
        match &mut selection.span {
            Span::Run { anchor, head } if clicks >= 2 => {
                if let Some(hit) = text.nearest(gesture.vx, gesture.vy)
                    && let Some((first, last)) = click_unit(text, clicks, *anchor)
                {
                    let (to_first, to_last) = click_unit(text, clicks, hit).unwrap_or((hit, hit));
                    (*anchor, *head) = if to_first < first { (last, to_first) } else { (first, to_last.max(last)) };
                }
            }
            Span::Run { head, .. } => {
                if let Some(hit) = text.nearest(gesture.vx, gesture.vy) {
                    *head = hit;
                }
            }
            Span::Column { rect } => {
                rect[2] = gesture.vx;
                rect[3] = gesture.vy;
            }
        }
    }
    if gesture.resp.drag_stopped()
        && view.selection.is_some_and(|selection| {
            selection.page == gesture.page && matches!(selection.span, Span::Column { .. }) && selection.text(text).is_empty()
        })
    {
        view.selection = None;
    }
}

fn finish_text_pointer_gesture(ui: &egui::Ui, view: &mut DocView, text: &PageText, gesture: &TextGesture<'_>) {
    let release_clicks = if gesture.resp.triple_clicked() {
        view.clicks.max(2).saturating_add(1)
    } else if gesture.resp.double_clicked() {
        view.clicks.max(2)
    } else {
        0
    };
    if release_clicks > 0 {
        view.clicks = release_clicks;
    }
    let clicks = selection_click_count(view.clicks, gesture.tool);
    if release_clicks > 0
        && clicks >= 2
        && gesture.over_text
        && view.selection.is_none()
        && let Some((first, last)) = text.nearest(gesture.vx, gesture.vy).and_then(|anchor| click_unit(text, clicks, anchor))
    {
        view.selection = Some(Selection::run(gesture.page, first, last));
    } else if gesture.resp.clicked() && view.clicks < 2 && gesture.text_drag {
        let shift = ui.input(|input| input.modifiers.shift);
        match (
            view.selection.as_mut().filter(|selection| shift && selection.page == gesture.page).map(|selection| &mut selection.span),
            text.nearest(gesture.vx, gesture.vy),
        ) {
            (Some(Span::Run { head, .. }), Some(hit)) => *head = hit,
            _ => view.selection = None,
        }
    }
}

/// Everything drawn over the page after its text: pictures, comments, content, links, fields, marks
/// and badges.
pub(super) fn page_overlays(ui: &mut egui::Ui, view: &mut DocView, frame: &mut PageFrame<'_>, slot: &PageSlot, input: &PageInput) {
    let PageEnv {
        added,
        allowed,
        calc_order,
        can_modify,
        comments_hidden,
        doc,
        doc_links,
        editing_content,
        form,
        generation,
        info,
        prefs,
        preparing,
        redact_mark,
        table_preview,
        tool,
        unobstructed,
        ..
    } = frame.env;
    let PageSlot { i, r, xf } = *slot;
    let PageFrame { resp, ptr, pointer, .. } = *frame;
    let painter = ui.painter();
    let tool_cx = PageToolContext { ui, resp, xf: &xf, ptr, page: i, doc, info };
    let pcx = comments::PageCx { page: i, xf: &xf, ptr, info, tool, prefs, allowed, hidden: comments_hidden, redact_mark };
    // A press nobody holds picks the picture under it: a link's click follows the link only (ruling
    // 5), though its drag or double-click still lets the picked picture go, as does a drag the page
    // it is over first sees, whichever page it began on.
    let pick_sees =
        matches!(input.press, PagePress::Free | PagePress::Mine(Owner::Link { .. })) || input.press == PagePress::Elsewhere && ptr.drag_started;
    if tool != QuickTool::Select {
        view.pending_pick = None;
    } else if pick_sees && !input.consumed {
        crate::image_pick::page_input(resp, &xf, i, info, doc, ptr, input.press == PagePress::Free, view);
    } else if resp.clicked() || ptr.drag_started {
        // Another owner took the press: an earlier click's picture, still being found, isn't picked.
        view.pending_pick = None;
    }
    crate::image_pick::drop_stale(doc, view);
    crate::image_pick::paint(painter, &xf, i, info, view);
    // A cancelled press ends no selection: no markup or redaction is made from it.
    if !view.press_cancelled {
        comments::page_after_text(resp, &pcx, view);
        if tool == QuickTool::Redact && can_modify {
            crate::redact_ui::after_text(resp, i, info, view);
        }
    }
    if input.area_tool {
        crate::redact_ui::paint(painter, i, view, pointer);
    }
    comments::paint_page(ui, painter, &pcx, view, input.hover.as_ref());
    if editing_content {
        crate::content_ui::paint_page(painter, &xf, i, info, added, view, pointer);
    }
    if tool == QuickTool::EditText {
        crate::object_ui::paint(painter, &xf, i, info, &view.objects);
    }
    if tool == QuickTool::EnhanceCamera {
        crate::camera_ui::paint(painter, &xf, i, view);
    }
    if tool == QuickTool::Link {
        crate::link_ui::paint(painter, &xf, i, info, doc_links, view, pointer);
    }
    if preparing {
        crate::prepare::paint_page(tool_cx, form, view);
        if let Some(preview) = &table_preview {
            crate::field_table_ui::paint_preview(painter, &xf, i, preview);
        }
        if calc_order {
            let selected = view.prepare.selected.as_ref().map(|(name, _)| name.as_str());
            if let Some(field) = crate::calc_order_ui::paint_page(tool_cx, form, &view.calc_order, generation, selected) {
                frame.calc_hover = Some(field);
            }
        }
    } else {
        crate::forms_ui::paint_page(painter, &xf, i, info, form, view, input.hover.as_ref());
        crate::forms_ui::tooltip(resp, form, input.hover.as_ref());
    }

    // Form-field highlight (Acrobat's "Highlight existing fields"); required fields get a
    // red border. Radio buttons and round XFA check boxes are round, and so is theirs (as
    // in Acrobat). Only what can be filled is marked: a read-only widget (its field's, or a
    // radio button's own) is left as it is drawn.
    paint_field_highlights(painter, &xf, info, form.as_slice(), i, view.highlight_fields);
    // Links: the hand and the link's target, which count only for the tools that follow or
    // edit links (`Layer::reached_by`). Tips draw above the palette, so none shows while it is
    // open, even beside it. A click follows the link its press was on, released over it: one that
    // slips off the link first follows nothing, as a button's does.
    if unobstructed
        && resp.clicked()
        && tool == QuickTool::Select
        && !input.consumed
        && let PagePress::Mine(Owner::Link { index }) = input.press
        && input.released_over
        && let Some(l) = info.links.iter().find(|l| l.page == i && l.index == index)
    {
        frame.out.clicked_link = Some(l.target.clone());
    }
    // Comments say what they are with any tool, beside its cursor, as Acrobat's do.
    comments::describe(&pcx, view);
    for (mp, mr, mc) in &view.compare_marks {
        if *mp == i {
            let sr = xf.user_rect(info, i, *mr).expand(1.5);
            painter.rect_filled(sr, CornerRadius::same(2), mc.gamma_multiply(0.28));
        }
    }
    if let Some((fp, fr, t0)) = view.flash
        && fp == i
    {
        let now = ui.input(|inp| inp.time);
        let t0 = if t0 == 0.0 { now } else { t0 };
        view.flash = Some((fp, fr, t0));
        let age = (now - t0) as f32;
        if age < 1.6 {
            let a = ((1.6 - age) / 1.6 * 255.0) as u8;
            let sr = xf.user_rect(info, i, fr).expand(4.0);
            painter.rect_stroke(
                sr,
                CornerRadius::same(3),
                Stroke::new(2.5, Color32::from_rgba_unmultiplied(0x1B, 0x63, 0xE0, a)),
                egui::StrokeKind::Outside,
            );
            ui.ctx().request_repaint();
        } else {
            view.flash = None;
        }
    }
    // Validation badges are viewer chrome over the widget. A recognised named-layer appearance may
    // show the verdict in the display copy too; saves retain its original streams. Badges never
    // enter document streams, exports or either print path. Their tips show where the comments'
    // do (`comments::tips_shown`).
    let tips = comments::tips_shown(tool, view);
    for badge in signature_badges(&doc.signatures, i) {
        let [x0, y0, x1, y1] = badge.rect;
        let sr = xf.user_rect(info, i, [x0 as f32, y0 as f32, x1 as f32, y1 as f32]);
        // The page transform can turn a finite widget into one the screen cannot place; stroking
        // it would put NaN into the mesh.
        if !sr.is_finite() {
            continue;
        }
        let (icon, color) = crate::sign_ui::signature_status_style(badge.status);
        painter.rect_stroke(sr, CornerRadius::same(2), Stroke::new(1.5, color), egui::StrokeKind::Outside);
        if let Some(mark) = verdict_mark_rect(sr, r, mark_anchor(&xf, info, i, badge.rect)) {
            icons::paint(ui, mark, icon, mark.width() * MARK_ICON_SHARE, color);
        }
        // The whole widget answers the pointer, so a field too small for a mark still says its
        // verdict; only where it is on the page, though. Overlapping widgets paint in order, so
        // the topmost badge's verdict is the tip; a link's or a comment's tip wins over any badge.
        if tips && pointer.is_some_and(|p| sr.expand(2.0).intersect(r).contains(p)) {
            view.cursor.describe(Subject::SignatureBadge, crate::i18n::t(badge.summary).to_string());
        }
    }
}

fn paint_field_highlights(painter: &egui::Painter, xf: &PageXform, info: &DocInfo, form: &[pdfcraft_engine::FormField], page: usize, enabled: bool) {
    if !enabled {
        return;
    }
    for field in form.iter() {
        let required = field.has(pdfcraft_engine::field_flags::REQUIRED);
        for widget in field.widgets.iter().filter(|widget| widget.page == Some(page) && !widget.hidden && !widget.read_only) {
            let rect = widget.rect;
            let screen_rect = xf.user_rect(info, page, [rect[0] as f32, rect[1] as f32, rect[2] as f32, rect[3] as f32]);
            let tint = Color32::from_rgba_unmultiplied(0x6E, 0x8E, 0xF5, 48);
            let (width, color) = if required { (2.0, Color32::from_rgb(0xE3, 0x22, 0x22)) } else { (1.0, Color32::from_rgb(0x6E, 0x8E, 0xF5)) };
            if widget.round {
                let radius = screen_rect.width().min(screen_rect.height()) / 2.0;
                painter.circle_filled(screen_rect.center(), radius, tint);
                painter.circle_stroke(screen_rect.center(), radius - width / 2.0, Stroke::new(width, color));
            } else {
                painter.rect_filled(screen_rect, CornerRadius::same(1), tint);
                painter.rect_stroke(screen_rect, CornerRadius::same(1), Stroke::new(width, color), egui::StrokeKind::Inside);
            }
        }
    }
}

/// Escape while the pages hold a press abandons the gesture it drives, whichever tool's: nothing is
/// placed, drawn, moved, resized or opened, and the press is ignored until the button is up. The
/// frame of the release still belongs to the cancelled press.
pub(super) fn cancel_on_escape(ctx: &egui::Context, view: &mut DocView, ptr: &PagePointer) {
    // Over once the button is up and its release seen, or when a new press comes (its release and
    // the next press can share a frame).
    let new_press = matches!(ptr.edges.last(), Some(crate::page_pointer::Edge::Press(_)));
    if view.press_cancelled && (new_press || !ptr.any_down && ptr.edges.is_empty()) {
        view.press_cancelled = false;
    }
    if !view.press_cancelled && ptr.origin.is_some() && ptr.primary_down && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
    {
        // A text selection being dragged goes with its drag: the markup, redaction or Replace
        // Text that its end would make is not made.
        if view.selection_drag {
            view.clear_selection();
        }
        view.cancel_pointer_gestures();
        view.press_cancelled = true;
    }
}
