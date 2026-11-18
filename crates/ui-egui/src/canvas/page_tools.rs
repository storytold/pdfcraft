//! The tool the pages run: one [`CanvasTool`] per quick tool, built each pass over the page session and handed to
//! the tool host after the page parts, so it takes every press no part took. What a tool finished in the pass (an
//! edit, a drawn rectangle, a page to move the camera quadrilateral to) is read from it after the run
//! ([`PageTool::finish`]) and applied to the session ([`ToolFinish::apply`]).

use craft::behavior::{CancelReason, Cursor, PointerButton};
use craft::host::{Host, Wake};
use craft::keymap::Mods;
use craft::pages::ui::{PageTextSource as _, PageTexts, PlacedPage, TextSelectState};
use craft::review::SavedSignature;
use craft::ui::canvas::{
    CanvasTool, HandTool, NavigationOptions, ToolAccess, ToolAction, ToolClaims, ToolContext, ToolKey, ToolPath, ToolPointer, ToolRegion, ZoomTool,
};
use craft::ui::primitives::Draw;
use egui::Pos2;
use pdfcraft_engine::{Document, Edit, FormField};
use pdfcraft_render::DocInfo;

use super::crop::{CropResult, CropTool};
use super::page_parts::{Owner, PagePress};
use super::page_select::{PageSelectTool, PickSlots};
use super::selection::DocTexts;
use super::zoom_snap::SnapshotTool;
use super::{PageSession, PageXform, presenter};
use crate::QuickTool;
use crate::image_pick::PickedImage;
use crate::tools::camera_ui::{CameraQuad, CameraTool};
use crate::tools::comments::{self, CommentsTool};
use crate::tools::edit_text_tool::EditTextTool;
use crate::tools::fill_sign::{FillInput, FillOutcome, FillSignTool, TextBorder};
use crate::tools::link_ui::LinkTool;
use crate::tools::measure_ui::MeasureTool;
use crate::tools::prepare::tool::PrepareTool;
use crate::tools::redact_ui::tool::{RedactResult, RedactTool};
use crate::tools::sign_ui::tool::SignAreaTool;
use crate::tools::stamps_ui::{CustomStamp, StampChoice, StampTool};

/// The Fill & Sign choices the Fill tools read: the text style and the saved signature and initials.
#[derive(Clone, Copy)]
pub(crate) struct FillPrefs<'a> {
    /// The size of new text, in points.
    pub(crate) text_size: f64,
    /// The colour of new text.
    pub(crate) text_color: [f64; 3],
    /// The border drawn around new text.
    pub(crate) text_border: TextBorder,
    /// The saved signature, if any.
    pub(crate) signature: Option<&'a SavedSignature>,
    /// The saved initials, if any.
    pub(crate) initials: Option<&'a SavedSignature>,
}

/// What the page tools read besides the session: the document, the placement of this pass and the app's choices.
/// One per pass; no tool changes it.
// PLAN: the session is not a field (pdf-canvas-w8 assumed `session` here): forms-w5 decided `FillSignTool::new(cx:
// &PageToolCx, input: FillInput<'a>, ..)` with `&'a mut` session fields in `FillInput`, which cannot be borrowed while a
// cx holding the whole `&mut PageSession` is lent; `page_tool` takes the session beside it (pdf-canvas-w8 passes it).
// PLAN: pdf-canvas-w8's literal also fills `sizes`, `can_crop`, `presenting`, `navigation`, `wake` and the stamp and
// Fill & Sign choices (`author`, `date_text`, `fill`, `by_line`, `custom_stamp`), which the arms read as production did.
#[derive(Clone, Copy)]
pub(crate) struct PageToolCx<'a> {
    /// The open document.
    pub(crate) doc: &'a Document,
    /// Its page facts.
    pub(crate) info: &'a DocInfo,
    /// Its form fields.
    pub(crate) form: &'a [FormField],
    /// The pages placed this pass.
    pub(crate) pages: &'a [PlacedPage],
    /// Every page's size in view space, by page index, unturned by the view (`DocView::page_sizes`).
    // PLAN: pdf-canvas-w8 passes `&view.page_sizes`; `PageXform::of` and the text layers need each page's size.
    pub(crate) sizes: &'a [(f32, f32)],
    /// The view's rotation this pass, in degrees.
    pub(crate) rotation: u16,
    /// The document allows comments.
    pub(crate) allowed: bool,
    /// The document allows changes.
    pub(crate) can_modify: bool,
    /// The document allows its pages' boxes to change.
    pub(crate) can_crop: bool,
    /// Prepare a form is open.
    pub(crate) preparing: bool,
    /// Full screen shows the pages: the presenter's keys reach it through the running tool.
    pub(crate) presenting: bool,
    /// The measuring units.
    pub(crate) units: crate::units::Unit,
    /// The user's navigation choices, which the Hand and Zoom tools read.
    pub(crate) navigation: NavigationOptions,
    /// The app's Host, which the tools that run background jobs build their queues from (L-135).
    pub(crate) host: &'a Host,
    /// The app's one wake source (L-168), for those queues.
    // PLAN: pdf-canvas-w8 passes `&app.wake` (hub-w1's field); its `PageToolCx` literal has no `wake`.
    pub(crate) wake: &'a Wake,
    /// The comment author, written on stamps and Fill & Sign marks.
    pub(crate) author: &'a str,
    /// The date as Fill & Sign's date field writes it, or why it cannot.
    pub(crate) date_text: &'a Result<String, String>,
    /// The Fill & Sign choices.
    pub(crate) fill: FillPrefs<'a>,
    /// The by-line of a dynamic stamp.
    pub(crate) by_line: &'a str,
    /// The custom stamp chosen, and the line filled in for it (#1741).
    pub(crate) custom_stamp: Option<(&'a CustomStamp, Option<&'a str>)>,
}

/// What the page tool borrows for a pass besides the session: the text layers and page transforms it builds from
/// the session and the placement. The caller keeps it beside the tool for the pass and never reads it.
#[derive(Default)]
pub(crate) struct ToolBench<'a> {
    texts: Option<DocTexts<'a>>,
    xforms: Vec<(usize, PageXform)>,
    map: Option<comments::PageMap<'a>>,
}

/// The tool a pass runs: the Select tool's text selection, a view tool, or a tool a page module writes.
enum Arm<'a> {
    /// Select and Column Select: the text selection and the picture pick.
    Select(PageSelectTool<'a>),
    /// The Hand tool.
    Hand(HandTool),
    /// Marquee Zoom: the framework's Zoom tool without scrubbing, so a drag draws the rectangle.
    MarqueeZoom(ZoomTool),
    /// A tool whose result is all in the session it borrows.
    Area(Box<dyn CanvasTool + 'a>),
    /// The camera quadrilateral, moved to the page a click went to.
    Camera(CameraTool<'a>),
    /// A measuring tool, whose finished measurement is an edit.
    Measure(MeasureTool<'a>),
    /// A stamp, whose placement is an edit.
    Stamp(StampTool<'a>),
    /// Redact: a text selection or a box, finished into marks.
    Redact(RedactTool<'a>),
    /// The signature rectangle of Digitally sign.
    SignArea(SignAreaTool<'a>),
    /// Snapshot: the area to copy.
    Snapshot(SnapshotTool),
    /// Crop: the rectangle to crop the page to, or a double click for Set Page Boxes.
    Crop(CropTool),
}

/// The page tool of a pass, handed to the tool host: every [`CanvasTool`] method forwards to the arm.
pub(crate) struct PageTool<'a> {
    arm: Arm<'a>,
    /// The quick tool the pages see (after the placing rule).
    tool: QuickTool,
    /// The quick tool selects text ([`tool_selects_text`]).
    selects_text: bool,
    /// The tool host ended a gesture with a release in this pass.
    released: bool,
    /// Full screen: the presenter's keys the tool took for it in this pass, applied after the run.
    presenting: bool,
    presenter_keys: Vec<ToolKey>,
}

/// What the page tool finished in a pass. The fields the page body reads are public; the rest are applied to the
/// session by [`apply`](Self::apply).
#[derive(Default)]
pub(crate) struct ToolFinish {
    /// The running tool is done and the app goes back to the Select tool (a stamp placed, a crop committed).
    pub(crate) back_to_select: bool,
    /// A Snapshot area drawn: (page, view-space rect).
    pub(crate) snapshot: Option<(usize, [f32; 4])>,
    /// The Crop tool's double click: open Set Page Boxes on that page.
    pub(crate) open_boxes: Option<usize>,
    /// The Crop tool's drawn rectangle, which the page body turns into the crop edit.
    pub(crate) crop: Option<CropResult>,
    /// Fill & Sign asked for a signature (`false`) or initials (`true`) before it can place one.
    pub(crate) open_signature: Option<bool>,
    /// Fill & Sign refused a click, and why.
    pub(crate) refused: Option<String>,
    edit: Option<Edit>,
    camera_page: Option<usize>,
    sign_drawn: Option<(usize, [f64; 4])>,
    redaction: Option<RedactResult>,
    presenter_keys: Vec<ToolKey>,
    /// The quick tool, when it selects text.
    text_tool: Option<QuickTool>,
    text_finished: bool,
}

/// Whether copied comments hang on the pointer: they take the next click whatever tool is on.
fn placing(session: &PageSession) -> bool {
    session.comments.paste.is_some()
}

/// The tool the pages see: the Select tool while copied comments hang on the pointer, until the bundle is placed or
/// let go, so no form field, added content or field being prepared takes the click either; else `chosen`.
pub(crate) fn seen_tool(session: &PageSession, chosen: QuickTool) -> QuickTool {
    if placing(session) { QuickTool::Select } else { chosen }
}

/// Text selection runs for the Select tool, for the markup tools (highlight…) and Replace Text, for Redact and for
/// Column Select. Every tool is listed, so a new tool must say whether it selects text.
pub(crate) fn tool_selects_text(tool: QuickTool, preparing: bool) -> bool {
    match tool {
        QuickTool::Comment(t) => t.markup().is_some() || t == comments::CommentTool::ReplaceText,
        QuickTool::Select => !preparing,
        QuickTool::Redact | QuickTool::ColumnSelect => true,
        QuickTool::Hand
        | QuickTool::Measure(_)
        | QuickTool::Crop
        | QuickTool::EnhanceCamera
        | QuickTool::Fill(_)
        | QuickTool::Field(_)
        | QuickTool::AddText
        | QuickTool::EditText
        | QuickTool::Stamp(_)
        | QuickTool::CustomStamp(_)
        | QuickTool::Link
        | QuickTool::SignArea { .. }
        | QuickTool::MarqueeZoom
        | QuickTool::Snapshot => false,
    }
}

/// The Select tool's text selection and picture pick over the session's text, borrowing the session's fields one by
/// one so the comments tool can borrow its own beside it.
fn select_tool<'a>(
    cx: &PageToolCx<'a>,
    text: &'a mut TextSelectState,
    texts: &'a dyn PageTexts,
    pending_pick: &'a mut Option<(usize, [f64; 2], u64)>,
    picked_image: &'a mut Option<PickedImage>,
    xforms: &'a [(usize, PageXform)],
) -> PageSelectTool<'a> {
    PageSelectTool::new(cx.pages, texts, text, cx.info, PickSlots::new(pending_pick, picked_image), xforms, cx.doc.edit_generation())
}

/// The page tool for `chosen` this pass. The placing rule applies first ([`seen_tool`]); Fill & Sign leaves its
/// click's outcome in `fill`, which the caller applies after the run with [`ToolFinish::apply`].
// PLAN: the sheet's `page_tool(tool, cx)` cannot lend the per-pass text layers and transforms the tools borrow (they
// would borrow locals of this function) nor Fill & Sign's outcome slot (forms-w5 decided `&'a mut Option<FillOutcome>`);
// pdf-canvas-w8 owns `bench` and `fill` beside the call, and reads `fill` after the tool is dropped.
pub(crate) fn page_tool<'a>(
    chosen: QuickTool,
    cx: PageToolCx<'a>,
    session: &'a mut PageSession,
    bench: &'a mut ToolBench<'a>,
    fill: &'a mut Option<FillOutcome>,
) -> PageTool<'a> {
    let tool = seen_tool(session, chosen);
    let ToolBench { texts: texts_slot, xforms, map: map_slot } = bench;
    let sizes = cx.sizes;
    let size = move |page: usize| sizes.get(page).copied().unwrap_or_default();
    *xforms = cx.pages.iter().map(|placed| (placed.page, PageXform::of(placed, cx.rotation, size(placed.page)))).collect();
    let xforms: &'a [(usize, PageXform)] = xforms;
    let map: &'a comments::PageMap<'a> = map_slot.insert(comments::PageMap::new(cx.info, cx.pages, cx.rotation));
    let selects_text = tool_selects_text(tool, cx.preparing);
    let arm = match tool {
        // Prepare a form under the Select tool: the selection rectangle and the moves of fields are Prepare's.
        QuickTool::Select if cx.preparing => Arm::Area(Box::new(PrepareTool::new(None, cx.can_modify, cx.form, session, cx.info, xforms))),
        QuickTool::Select | QuickTool::ColumnSelect | QuickTool::Comment(_) | QuickTool::Redact => {
            let texts: &'a DocTexts<'a> = texts_slot.insert(DocTexts::new(&session.texts, &session.text_failed, cx.rotation, cx.sizes));
            let select = select_tool(&cx, &mut session.text, texts, &mut session.pending_pick, &mut session.picked_image, xforms);
            match tool {
                QuickTool::ColumnSelect => Arm::Select(select.column(true)),
                QuickTool::Redact => {
                    let pages = cx.pages;
                    let over_text = move |at: Pos2| {
                        pages
                            .iter()
                            .find(|placed| placed.rect.contains(at))
                            .is_some_and(|placed| texts.app(placed.page).is_some_and(|layer| layer.glyph_box_at(placed.to_page(at)).is_some()))
                    };
                    Arm::Redact(RedactTool::new(select, cx.pages, cx.rotation, cx.info, over_text))
                }
                // Select runs the comments tool too: a hanging paste is placed by its press, and a press no comment
                // part took goes on to the text.
                _ => {
                    let view = &mut session.comments;
                    let map = comments::PageMap::new(cx.info, cx.pages, cx.rotation);
                    let annotate = CommentsTool::new(&mut view.tool, &mut view.paste, map, &mut view.tool_done);
                    let annotate = if selects_text { annotate.text(select) } else { annotate };
                    Arm::Area(Box::new(annotate))
                }
            }
        }
        QuickTool::Hand => Arm::Hand(HandTool::new(cx.navigation)),
        QuickTool::MarqueeZoom => {
            let mut options = cx.navigation;
            options.scrubby_zoom = false;
            Arm::MarqueeZoom(ZoomTool::new(options))
        }
        QuickTool::Measure(t) => {
            Arm::Measure(MeasureTool::new(&mut session.measure, cx.doc, map, cx.info, t, cx.units, cx.author, cx.host, cx.wake.clone()))
        }
        QuickTool::Crop => Arm::Crop(CropTool::new(cx.can_crop, cx.pages, cx.rotation, size)),
        QuickTool::EnhanceCamera => Arm::Camera(CameraTool::new(cx.can_modify, cx.pages)),
        QuickTool::Fill(fill_tool) => {
            let input = FillInput {
                tool: fill_tool,
                text_size: cx.fill.text_size,
                text_color: cx.fill.text_color,
                text_border: cx.fill.text_border,
                signature: cx.fill.signature,
                initials: cx.fill.initials,
                author: cx.author,
                date_text: cx.date_text,
                fill_text: &mut session.fill_text,
                comments: &mut session.comments,
            };
            Arm::Area(Box::new(FillSignTool::new(&cx, input, fill)))
        }
        QuickTool::Field(field_tool) => Arm::Area(Box::new(PrepareTool::new(Some(field_tool), cx.can_modify, cx.form, session, cx.info, xforms))),
        // PLAN: assumed `AddTextTool::new(session, doc, info)` in `content_ui` (pdf-edit-w7 gives no argument list).
        QuickTool::AddText => Arm::Area(Box::new(crate::tools::content_ui::AddTextTool::new(session, cx.doc, cx.info))),
        // PLAN: pdf-edit-w10's `EditTextTool::new(view, doc, info)` takes no placed pages, which the review asked to hand over.
        QuickTool::EditText => Arm::Area(Box::new(EditTextTool::new(session, cx.doc, cx.info))),
        QuickTool::Stamp(kind) => {
            Arm::Stamp(StampTool::new(StampChoice::Builtin(kind), cx.by_line.to_owned(), None, cx.author.to_owned(), cx.pages, cx.info, map))
        }
        QuickTool::CustomStamp(_) => match cx.custom_stamp {
            Some((stamp, line)) => Arm::Stamp(StampTool::new(
                StampChoice::Custom(stamp.clone()),
                cx.by_line.to_owned(),
                line.map(str::to_owned),
                cx.author.to_owned(),
                cx.pages,
                cx.info,
                map,
            )),
            // The library no longer holds the stamp: no click places one, and a drag pans as the Hand tool's.
            None => Arm::Hand(HandTool::new(cx.navigation)),
        },
        // PLAN: assumed `LinkTool::new(session, doc, info, can_modify)` (pdf-edit-w11 gives `new(view, ..)`).
        QuickTool::Link => Arm::Area(Box::new(LinkTool::new(session, cx.doc, cx.info, cx.can_modify))),
        QuickTool::SignArea { certify } => Arm::SignArea(SignAreaTool::new(certify, cx.pages, cx.rotation, cx.info)),
        QuickTool::Snapshot => Arm::Snapshot(SnapshotTool::new(cx.pages, cx.rotation, size)),
    };
    PageTool { arm, tool, selects_text, released: false, presenting: cx.presenting, presenter_keys: Vec::new() }
}

impl<'a> PageTool<'a> {
    fn tool(&self) -> &(dyn CanvasTool + 'a) {
        match &self.arm {
            Arm::Select(t) => t,
            Arm::Hand(t) => t,
            Arm::MarqueeZoom(t) => t,
            Arm::Area(t) => &**t,
            Arm::Camera(t) => t,
            Arm::Measure(t) => t,
            Arm::Stamp(t) => t,
            Arm::Redact(t) => t,
            Arm::SignArea(t) => t,
            Arm::Snapshot(t) => t,
            Arm::Crop(t) => t,
        }
    }

    fn tool_mut(&mut self) -> &mut (dyn CanvasTool + 'a) {
        match &mut self.arm {
            Arm::Select(t) => t,
            Arm::Hand(t) => t,
            Arm::MarqueeZoom(t) => t,
            Arm::Area(t) => &mut **t,
            Arm::Camera(t) => t,
            Arm::Measure(t) => t,
            Arm::Stamp(t) => t,
            Arm::Redact(t) => t,
            Arm::SignArea(t) => t,
            Arm::Snapshot(t) => t,
            Arm::Crop(t) => t,
        }
    }

    /// What the tool finished in this pass, read after the tool host ran it; the tool's borrows of the session end
    /// here.
    // PLAN: assumed result readers `MeasureTool::take_edit`, `StampTool::take_placed`, `RedactTool::finished`,
    // `SignAreaTool::finished` (the owning sheets name the results, not every reader's name).
    pub(crate) fn finish(self) -> ToolFinish {
        let mut finish = ToolFinish {
            presenter_keys: self.presenter_keys,
            text_tool: Some(self.tool).filter(|_| self.selects_text),
            text_finished: self.released && self.selects_text,
            ..ToolFinish::default()
        };
        match self.arm {
            Arm::Camera(tool) => finish.camera_page = tool.moved_to(),
            Arm::Measure(mut tool) => finish.edit = tool.take_edit(),
            Arm::Stamp(mut tool) => {
                finish.edit = tool.placed();
                finish.back_to_select = finish.edit.is_some();
            }
            Arm::Redact(mut tool) => finish.redaction = tool.finished(),
            Arm::SignArea(mut tool) => finish.sign_drawn = tool.finished(),
            Arm::Snapshot(mut tool) => finish.snapshot = tool.take_result(),
            Arm::Crop(mut tool) => {
                finish.crop = tool.take_result();
                finish.back_to_select = finish.crop.is_some();
                finish.open_boxes = tool.take_open_boxes();
            }
            Arm::Select(_) | Arm::Hand(_) | Arm::MarqueeZoom(_) | Arm::Area(_) => {}
        }
        finish
    }
}

impl ToolFinish {
    /// Applies what the tool finished to the session: its edit, the camera quadrilateral's new page, the signature
    /// rectangle, the redaction marks, Fill & Sign's `fill` outcome and the presenter's keys; and, under a tool that
    /// selects text, the comments' follow-up of the selection on every placed page (comments-w8 decision: every pass,
    /// with `finished` only in the pass of the release).
    pub(crate) fn apply(&mut self, fill: Option<FillOutcome>, session: &mut PageSession, cx: &PageToolCx<'_>) {
        if let Some(edit) = self.edit.take() {
            session.pending_edit = Some(edit);
        }
        if let Some(page) = self.camera_page.take()
            && let Some(placed) = cx.pages.iter().find(|placed| placed.page == page)
        {
            // PLAN: assumed `camera_ui::page_points` made `pub(crate)` (pdf-print-ocr-w10), the one rule for a placed
            // page's size in view points.
            let (w, h) = crate::tools::camera_ui::page_points(placed);
            session.camera = Some(CameraQuad::inset(page, w, h));
        }
        if let Some(drawn) = self.sign_drawn.take() {
            session.sign.drawn = Some(drawn);
        }
        match self.redaction.take() {
            Some(RedactResult::Marks(marks, page)) => session.pending_redaction = Some((QuickTool::Redact, marks, page)),
            Some(RedactResult::Refused) => session.redaction_refused = true,
            None => {}
        }
        match fill {
            Some(FillOutcome::Edit(edit)) => session.pending_edit = Some(*edit),
            // PLAN: forms-w5's `FillOutcome::Signature(Box<Edit>)` carries no page, which `fill_signature_page` needs
            // (production set the clicked page); assumed `Signature(Box<Edit>, usize)`.
            Some(FillOutcome::Signature(edit, page)) => {
                session.pending_edit = Some(*edit);
                session.fill_signature_page = Some(page);
            }
            Some(FillOutcome::CreateSignature) => self.open_signature = Some(false),
            Some(FillOutcome::CreateInitials) => self.open_signature = Some(true),
            Some(FillOutcome::Refused(why)) => self.refused = Some(why),
            None => {}
        }
        for key in self.presenter_keys.drain(..) {
            // PLAN: assumed `Presenter::key(&mut self, &ToolKey)` (pdf-canvas-w18 names the keys it takes).
            session.presenter.key(&key);
        }
        if let Some(tool @ QuickTool::Comment(_)) = self.text_tool {
            for placed in cx.pages {
                // PLAN: assumed `comments::PageCx::of(page, info)` for the per-page cx of `page_after_text` (pdf-comments-w8
                // decided `page_after_text(cx: &PageCx, tool, finished, view)` and pdf-comments-w7 rewrites `PageCx`).
                let page_cx = comments::PageCx::of(placed.page, cx.info);
                comments::page_after_text(&page_cx, tool, self.text_finished, session);
            }
        }
    }
}

/// A press on an Edit text part: with Shift or Cmd (Ctrl off macOS) held it adds to or takes from the objects
/// selected together first, whatever lies under it; then the objects and the paragraph frame take their own; then
/// each family its own.
// PLAN: pdf-canvas-w8's `route_press` calls this for the Edit text owners under `QuickTool::EditText`; assumed
// `object_ui::modified_press(..) -> bool` (taken) and `objects_press(session, press)` (pdf-edit-w10 decisions).
pub(crate) fn edit_text_press(session: &mut PageSession, doc: &Document, info: &DocInfo, press: &PagePress) {
    let Mods { cmd, shift, .. } = press.mods;
    if (shift || cmd) && crate::tools::object_ui::modified_press(session, doc, info, press) {
        return;
    }
    match press.owner {
        Owner::Objects | Owner::Frame { .. } => crate::tools::object_ui::objects_press(session, press),
        Owner::Paragraph { .. } => crate::tools::edit_text_ui::paragraph_press(session, doc, press),
        Owner::Added { .. } => crate::tools::content_ui::added_press(session, doc, press),
        Owner::Picture { .. } => crate::tools::edit_text_ui::picture_press(session, doc, press),
        Owner::Comment { .. }
        | Owner::Link { .. }
        | Owner::TextLink { .. }
        | Owner::Field { .. }
        | Owner::PrepareField { .. }
        | Owner::SignTab { .. } => {}
    }
}

impl CanvasTool for PageTool<'_> {
    fn each_pass(&mut self, cx: &mut ToolContext<'_>) {
        self.released = false;
        self.tool_mut().each_pass(cx);
    }

    fn press(&mut self, pointer: &ToolPointer, cx: &mut ToolContext<'_>) -> ToolAction {
        self.tool_mut().press(pointer, cx)
    }

    fn drag(&mut self, pointer: &ToolPointer, cx: &mut ToolContext<'_>) {
        self.tool_mut().drag(pointer, cx);
    }

    /// Forwards, and notes the release: a text selection ends in this pass ([`ToolFinish::apply`]).
    fn release(&mut self, pointer: &ToolPointer, cx: &mut ToolContext<'_>) {
        self.released = true;
        self.tool_mut().release(pointer, cx);
    }

    fn hover(&mut self, pointer: &ToolPointer, cx: &mut ToolContext<'_>) {
        self.tool_mut().hover(pointer, cx);
    }

    fn cancel(&mut self, reason: CancelReason, cx: &mut ToolContext<'_>) {
        self.tool_mut().cancel(reason, cx);
    }

    /// The tool's claims; in full screen the presenter's page keys too.
    fn claims(&self) -> ToolClaims {
        let claims = self.tool().claims();
        if self.presenting { claims.keys(presenter::PRESENT_KEYS) } else { claims }
    }

    fn regions(&self) -> Vec<ToolRegion> {
        self.tool().regions()
    }

    /// In full screen the presenter's keys and Enter are the presenter's, applied after the run; every other key is
    /// the tool's.
    fn key(&mut self, key: &ToolKey, cx: &mut ToolContext<'_>) -> bool {
        if self.presenting
            && match key {
                ToolKey::Kept(chord) => presenter::PRESENT_KEYS.contains(chord),
                ToolKey::Submit => true,
                _ => false,
            }
        {
            self.presenter_keys.push(key.clone());
            return true;
        }
        self.tool_mut().key(key, cx)
    }

    fn paint(&self, draw: &Draw<'_>, cx: &ToolContext<'_>) {
        self.tool().paint(draw, cx);
    }

    fn cursor(&self, cx: &ToolContext<'_>) -> Cursor {
        self.tool().cursor(cx)
    }

    fn takes(&self, button: PointerButton, mods: Mods) -> bool {
        self.tool().takes(button, mods)
    }

    fn autoscrolls(&self) -> bool {
        self.tool().autoscrolls()
    }

    fn autoscrolls_on_hover(&self) -> bool {
        self.tool().autoscrolls_on_hover()
    }

    fn view_only(&self) -> bool {
        self.tool().view_only()
    }

    fn takes_every_move(&self) -> bool {
        self.tool().takes_every_move()
    }

    fn access(&self, cx: &ToolContext<'_>) -> Option<ToolAccess> {
        self.tool().access(cx)
    }

    fn command_form(&self) -> ToolPath {
        self.tool().command_form()
    }
}
