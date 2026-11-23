//! What lies on the pages that a press can take: one part per thing a page module offers (a comment, a link, a
//! form field's widget, added content), all registered in the one [`PageParts`] canvas control, so the framework's
//! hit priority ranks them against each other and the press machine holds each press to its release. The tool host
//! shown after it takes every press no part took.

use craft::behavior::{CursorIcon, HitLevel, HitPriority, HitShape, Input, Intent, Metrics, NavMove, PartKey, PartRef, PressPhase, Sense};
use craft::keymap::{Chord, Key, Mods};
use craft::ui::contract::{
    Common, Component, Cx, Focus as _, MeasureCx, NoValue, Outcome, PartsCx, Pointer as _, Props as _, Role, TooltipAt, TooltipSpec, TooltipWhen,
};
use craft::ui::runtime::commands::FireSource;
use craft::ui::runtime::env::Label;
use craft::ui::runtime::focus::KeyUse;
use craft::ui::style::{ComponentStyle, StyleCx};
use egui::{Rect, Response, Vec2};

/// What a part on the pages is: the page module that offered it reads its own presses by it. Each variant names its
/// thing by the numbers the module keeps it by, never by its place in the part list, so a press held across passes
/// keeps naming the same thing while the list changes under it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Owner {
    /// A comment (or a Fill & Sign mark), by its `/Annots` index on its page.
    Comment { index: usize },
    /// A link, by its index in its page's `/Annots`.
    Link { index: usize },
    /// A web or email address written as plain page text (#4207), by its index among the page's.
    TextLink { index: usize },
    /// A form field's widget, by the field's index in the form and the widget's in the field.
    Field { field: usize, widget: usize },
    /// Prepare Form: a field's widget, to select and move.
    PrepareField { field: usize, widget: usize },
    /// Edit text: a paragraph, by its index among the page's.
    Paragraph { block: usize },
    /// Edit text: a picture, by its index among the page's.
    Picture { index: usize },
    /// Edit text: the objects selected together, by index in the selection.
    Objects { index: usize },
    /// Edit text: one side of the frame band of the paragraph open for typing, by the paragraph's index and side (0..4).
    Frame { block: usize, side: usize },
    /// Added text or a picture placed on the page, by its index among the page's.
    Added { index: usize },
    /// The sign tab on an empty signature field a click would sign, by the field's index in the form.
    SignTab { field: usize },
    /// A signed signature's validation badge, by its index on the page: shows the verdict tooltip on hover.
    Badge { index: usize },
}

const COMMENT: PartKey = PartKey::new("comment");
const LINK: PartKey = PartKey::new("link");
const TEXT_LINK: PartKey = PartKey::new("text_link");
const FIELD: PartKey = PartKey::new("field");
const PREPARE_FIELD: PartKey = PartKey::new("prepare_field");
const PARAGRAPH: PartKey = PartKey::new("paragraph");
const PICTURE: PartKey = PartKey::new("picture");
const OBJECTS: PartKey = PartKey::new("objects");
/// The page-part control's identity also lets the view cancel its captured press on leaving.
pub(crate) fn control_id(view_key: u64) -> egui::Id {
    egui::Id::unique(("page-parts", view_key))
}
const FRAME: PartKey = PartKey::new("frame");
const ADDED: PartKey = PartKey::new("added");
const SIGN_TAB: PartKey = PartKey::new("sign_tab");
const BADGE: PartKey = PartKey::new("badge");

/// `high` in the upper 32 bits of a part index and `low` in the lower; `None` when either does not fit, so no two
/// things ever share an index.
fn pack(high: usize, low: usize) -> Option<u64> {
    let (high, low) = (u32::try_from(high).ok()?, u32::try_from(low).ok()?);
    Some((u64::from(high) << 32) | u64::from(low))
}

/// The two halves [`pack`] put in `index`.
fn unpack(index: u64) -> Option<(usize, usize)> {
    let high = usize::try_from(index >> 32).ok()?;
    let low = usize::try_from(index & u64::from(u32::MAX)).ok()?;
    Some((high, low))
}

impl Owner {
    /// The part this owner registers as on `page`. Its key is the variant's; its index packs the page in the upper 32
    /// bits and the variant's own number in the lower, except a field widget's, which is `field << 32 | widget` (a
    /// widget lies on one page, so the pair alone names it). `None` when a number does not fit in 32 bits: that part
    /// is not registered.
    pub(crate) fn part_ref(self, page: usize) -> Option<PartRef> {
        let (key, index) = match self {
            Owner::Comment { index } => (COMMENT, pack(page, index)?),
            Owner::Link { index } => (LINK, pack(page, index)?),
            Owner::TextLink { index } => (TEXT_LINK, pack(page, index)?),
            Owner::Field { field, widget } => (FIELD, pack(field, widget)?),
            Owner::PrepareField { field, widget } => (PREPARE_FIELD, pack(field, widget)?),
            Owner::Paragraph { block } => (PARAGRAPH, pack(page, block)?),
            Owner::Picture { index } => (PICTURE, pack(page, index)?),
            Owner::Objects { index } => (OBJECTS, pack(page, index)?),
            Owner::Frame { block, side } => (FRAME, pack(page, block.checked_shl(2)? | (side & 3))?),
            Owner::Added { index } => (ADDED, pack(page, index)?),
            Owner::SignTab { field } => (SIGN_TAB, pack(page, field)?),
            Owner::Badge { index } => (BADGE, pack(page, index)?),
        };
        Some(key.at(index))
    }

    /// The owner `part` names, with its page where the index carries one ([`part_ref`](Self::part_ref)): `None` for
    /// a field widget's page, which its part does not hold. `None` for a part that is no page part.
    // PLAN: decisions.md gives `Option<(Owner, usize)>`, but a field widget's index is `field << 32 | widget` by the same decision and holds no page, so the page is optional.
    pub(crate) fn from_part(part: PartRef) -> Option<(Owner, Option<usize>)> {
        let (high, low) = unpack(part.index?)?;
        let paged = |owner: Owner| Some((owner, Some(high)));
        match part.key {
            COMMENT => paged(Owner::Comment { index: low }),
            LINK => paged(Owner::Link { index: low }),
            TEXT_LINK => paged(Owner::TextLink { index: low }),
            FIELD => Some((Owner::Field { field: high, widget: low }, None)),
            PREPARE_FIELD => Some((Owner::PrepareField { field: high, widget: low }, None)),
            PARAGRAPH => paged(Owner::Paragraph { block: low }),
            PICTURE => paged(Owner::Picture { index: low }),
            OBJECTS => paged(Owner::Objects { index: low }),
            FRAME => paged(Owner::Frame { block: low >> 2, side: low & 3 }),
            ADDED => paged(Owner::Added { index: low }),
            SIGN_TAB => paged(Owner::SignTab { field: low }),
            BADGE => paged(Owner::Badge { index: low }),
            _ => None,
        }
    }
}

/// The families of page parts, lowest first: the one table that says which outranks which where parts overlap.
/// Families that take a press while a tool is active outrank the page's own text and pictures; handles and the open
/// editor outrank everything on the page body.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum PageLayer {
    /// Edit text's pictures.
    Picture,
    /// Edit text's paragraphs.
    Text,
    /// Links and text links.
    Link,
    /// Comments and Fill & Sign marks.
    Comment,
    /// Form fields' widgets and the sign tabs over them.
    Field,
    /// Movable comment bodies, above filled form fields and below added content.
    MovableComment,
    /// What a tool placed and moves: added content, Prepare's fields, the objects selected together.
    Placed,
    /// Handles on what is selected, and the frame band of the paragraph open for typing.
    Handle,
}

impl PageLayer {
    /// The layer's rank in the framework's hit resolution: a higher `(level, sub)` wins where parts overlap.
    pub(crate) fn priority(self) -> HitPriority {
        let (level, sub) = match self {
            PageLayer::Picture => (HitLevel::Body, 1),
            PageLayer::Text => (HitLevel::Body, 2),
            PageLayer::Link => (HitLevel::Body, 3),
            PageLayer::Comment => (HitLevel::Body, 4),
            PageLayer::Field => (HitLevel::Body, 5),
            PageLayer::MovableComment => (HitLevel::Body, 6),
            PageLayer::Placed => (HitLevel::Body, 7),
            PageLayer::Handle => (HitLevel::Handle, 0),
        };
        HitPriority::new(level, sub)
    }
}

/// One thing on a page a press can take: its owner, its page, its shape in screen points and its layer, with what
/// the pointer shows over it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PagePart {
    /// What the part is.
    pub(crate) owner: Owner,
    /// The page it lies on.
    pub(crate) page: usize,
    /// Its shape, in the screen points of the page body.
    pub(crate) shape: HitShape,
    /// Its family, which ranks it against overlapping parts.
    pub(crate) layer: PageLayer,
    cursor: Option<CursorIcon>,
    tip: Option<(String, TooltipWhen)>,
    tip_below: bool,
    stacking: Option<u32>,
    passes_drag: bool,
    context_only: bool,
    hover_only: bool,
    focusable: bool,
}

impl PagePart {
    /// `owner` on `page`, hit by `shape`, ranked by `layer`. It takes clicks, drags and secondary clicks, and shows the
    /// default cursor and no tip.
    pub(crate) fn new(owner: Owner, page: usize, shape: impl Into<HitShape>, layer: PageLayer) -> Self {
        PagePart {
            owner,
            page,
            shape: shape.into(),
            layer,
            cursor: None,
            tip: None,
            tip_below: false,
            stacking: None,
            passes_drag: false,
            context_only: false,
            hover_only: false,
            focusable: false,
        }
    }

    /// Where the part stacks among the parts of its layer, the higher on top, in place of its place in the list: for
    /// parts listed in another order than they overlap (form fields go in tab order, while the earlier field in the
    /// form takes a press where two overlap).
    #[must_use]
    pub(crate) fn stacked(mut self, order: u32) -> Self {
        self.stacking = Some(order);
        self
    }

    /// The part takes only secondary clicks: a primary press goes to what lies below (a highlight over a link, whose
    /// click follows the link while its right-click opens the highlight's menu).
    #[must_use]
    pub(crate) fn context_only(mut self) -> Self {
        self.context_only = true;
        self
    }

    /// The cursor over the part.
    #[must_use]
    pub(crate) fn cursor(mut self, cursor: CursorIcon) -> Self {
        self.cursor = Some(cursor);
        self
    }

    /// The tip stands below the part, clear of its text, as production's link target does, not beside the pointer.
    #[must_use]
    pub(crate) fn tip_below(mut self) -> Self {
        self.tip_below = true;
        self
    }

    /// The tip shown beside the pointer over the part, after the hover delay.
    #[must_use]
    pub(crate) fn tip(mut self, tip: impl Into<String>) -> Self {
        self.tip = Some((tip.into(), TooltipWhen::Hover));
        self
    }

    /// The tip shown beside the pointer as soon as it is over the part: what a comment says or where a link goes,
    /// which the page states at once (a form field's own tooltip waits for the hover delay, as other tooltips do).
    #[must_use]
    pub(crate) fn tip_now(mut self, tip: impl Into<String>) -> Self {
        self.tip = Some((tip.into(), TooltipWhen::Now));
        self
    }

    /// The part only clicks: a drag that starts on it goes to what lies below (text markup selects the text under it,
    /// a link with the Link tool lets a new link be drawn over it).
    #[must_use]
    pub(crate) fn passes_drag(mut self) -> Self {
        self.passes_drag = true;
        self
    }

    /// The part is a Tab stop, in the order the parts are given (a form's fields in the PDF's tab order).
    #[must_use]
    pub(crate) fn focusable(mut self) -> Self {
        self.focusable = true;
        self
    }

    /// The part senses hover alone (for tips that let presses go straight to the tool beneath).
    #[must_use]
    pub(crate) fn hover_only(mut self) -> Self {
        self.hover_only = true;
        self
    }

    /// What the part senses: a click, a secondary click and a drag of its own, or a click that passes its drag on, or
    /// a secondary click alone, or hover alone.
    fn sense(&self) -> Sense {
        if self.hover_only {
            return Sense::HOVER;
        }
        if self.context_only {
            return Sense::CONTEXT_CLICK;
        }
        let drag = if self.passes_drag { Sense::PASSES_DRAG } else { Sense::DRAG };
        Sense::CLICK | Sense::CONTEXT_CLICK | drag
    }
}

/// The Tab stop of `owner` among `parts`: the part focus moves to for it.
pub(crate) fn focus_stop(parts: &[PagePart], owner: Owner) -> Option<PartRef> {
    parts.iter().filter(|part| part.focusable && part.owner == owner).find_map(|part| part.owner.part_ref(part.page))
}

/// A press on a page part in this pass, as the press machine reports it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PagePress {
    /// What was pressed.
    pub(crate) owner: Owner,
    /// The page it lies on.
    pub(crate) page: usize,
    /// The press's phase in this pass, in screen points; a click's count is its start's `clicks`.
    pub(crate) phase: PressPhase,
    /// The modifiers held: readers test `cmd` (Cmd on macOS, Ctrl elsewhere) and `shift`.
    pub(crate) mods: Mods,
}

/// The style of the page parts: they paint nothing.
#[derive(Clone, Debug, PartialEq, ComponentStyle)]
pub(crate) struct PagePartsStyle {
    #[style(metrics)]
    metrics: Metrics,
}

const TAB: Chord = Chord::plain(Key::Tab);
const SHIFT_TAB: Chord = Chord::new(Mods::SHIFT, Key::Tab);

/// What the page parts report this pass.
#[derive(Debug)]
pub(crate) struct PagePartsOutput {
    /// The control's response.
    pub(crate) response: Response,
    /// The press on a part, in the phase this pass reports.
    pub(crate) press: Option<PagePress>,
    /// A secondary click beside an ongoing press on a part.
    pub(crate) context: Option<egui::Pos2>,
    /// A key the focused part was given: a form field's toggle, its arrows, Enter or typed text.
    pub(crate) key: Option<(Owner, Intent)>,
    /// Tab (`true`) or Shift+Tab (`false`) pressed while a focusable field part held focus.
    pub(crate) tab: Option<(Owner, bool)>,
    /// The part under the pointer this pass, and its page.
    pub(crate) hovered: Option<(Owner, usize)>,
    /// The part focus arrived on this pass (Tab, a focus request or a click): a form field opens on it.
    pub(crate) focused: Option<(Owner, usize)>,
}

impl Outcome for PagePartsOutput {
    type Value = NoValue;

    fn response(&self) -> &Response {
        &self.response
    }
}

/// The parts of the pages, in paint order: each family appends its own after what it paints over, so where parts of
/// one layer overlap the one drawn on top takes the press. A canvas control that paints nothing: show it first in the
/// page body, before the tool host, over the body's rect.
#[derive(Component)]
#[component(canvas_control, sample = PageParts::sample)]
pub(crate) struct PageParts<'a> {
    #[common]
    common: Common,
    parts: &'a [PagePart],
    focus: Option<Owner>,
    report: Report,
}

/// What `update` read, for `finish`.
#[derive(Debug, Default)]
struct Report {
    press: Option<PagePress>,
    context: Option<egui::Pos2>,
    key: Option<(Owner, Intent)>,
    tab: Option<(Owner, bool)>,
    hovered: Option<(Owner, usize)>,
    focused: Option<(Owner, usize)>,
}

impl<'a> PageParts<'a> {
    /// The parts `parts`, given in paint order.
    pub(crate) fn new(parts: &'a [PagePart]) -> Self {
        PageParts { common: Common::new(), parts, focus: None, report: Report::default() }.label(Label::from(tl!("Page content")))
    }

    /// Moves the focus to `owner`'s part in this pass, when it is a Tab stop and the pages hold the focus.
    #[must_use]
    pub(crate) fn focus(mut self, owner: Option<Owner>) -> Self {
        self.focus = owner;
        self
    }

    /// No parts, for the component suite.
    fn sample() -> Self {
        PageParts::new(&[])
    }

    /// The part `part` names, among this pass's parts.
    fn part_named(&self, part: PartRef) -> Option<&PagePart> {
        let (owner, page) = Owner::from_part(part)?;
        self.parts.iter().find(|p| p.owner == owner && page.is_none_or(|page| p.page == page))
    }

    /// The key the focused part was given this pass: an activation by the keyboard, Enter, an arrow or typed text, the
    /// first of them; a field reads one key per pass.
    fn field_key(cx: &mut Cx<'_, Self>, part: PartRef) -> Option<Intent> {
        if cx.part_activated(part) == Some(FireSource::Key) {
            return Some(Intent::Activate);
        }
        let intents = cx.intents();
        if intents.take(Intent::Cancel) {
            return Some(Intent::Cancel);
        }
        if intents.take(Intent::Submit) {
            return Some(Intent::Submit);
        }
        if let Some((mv, mods)) = intents.take_move_where(|mv, _| matches!(mv, NavMove::Up | NavMove::Down | NavMove::Back | NavMove::Forward)) {
            return Some(Intent::Move(mv, mods));
        }
        intents.take_text().map(Intent::Text)
    }
}

impl Component for PageParts<'_> {
    const ROLE: Role = Role::GROUP;
    const OVERLAYS: bool = false;
    const KEYS: KeyUse = KeyUse::of(Input::Pointer).with_moves();
    type Style = PagePartsStyle;
    type Output = PagePartsOutput;

    fn style(_: &StyleCx<'_>, _: PagePartsStyleKey) -> PagePartsStyle {
        PagePartsStyle { metrics: Metrics::new() }
    }

    fn measure(&mut self, _: &MeasureCx<'_, Self>) -> Vec2 {
        Vec2::ZERO
    }

    /// One part per page part, in the order given, each with that order as its stacking order. A family gives one
    /// shape per owner and page; a second shape of the same owner on the same page is not registered, so one press
    /// never names two parts.
    fn parts(&mut self, _: Rect, cx: &mut PartsCx<'_, Self>) {
        let mut declared: Vec<PartRef> = Vec::with_capacity(self.parts.len());
        for (order, part) in self.parts.iter().enumerate() {
            let Some(at) = part.owner.part_ref(part.page) else { continue };
            let Ok(order) = u32::try_from(order) else { break };
            if declared.contains(&at) {
                continue;
            }
            declared.push(at);
            let mut builder =
                cx.part(at, part.shape.clone()).sense(part.sense()).priority(part.layer.priority()).stacking(part.stacking.unwrap_or(order));
            if part.layer == PageLayer::MovableComment && part.passes_drag {
                // A movable comment's drag belongs to its canvas tool even over a draggable field.
                builder = builder.drag_to_root();
            }
            if let Some(cursor) = part.cursor {
                builder = builder.cursor(cursor);
            }
            if let Some((tip, when)) = &part.tip {
                let at = if part.tip_below { TooltipAt::Control } else { TooltipAt::Pointer };
                builder = builder.tooltip(TooltipSpec::new(Label::from(tip.clone())).at(at).when(*when));
            }
            if part.focusable {
                builder.focusable();
            } else {
                builder.pointer_only("the comments, links and fields panels and the selected object's commands reach page content");
            }
        }
    }

    /// Reads the press, the hover, the focused part's key and the focus that arrived. The press machine reports a
    /// press's phases one per pass, so a second pass of a frame reads only its own phase.
    fn update(&mut self, cx: &mut Cx<'_, Self>) {
        // A press on a Tab stop moves the key focus to it, so the keys go on from the field clicked (#2223); else the
        // focus follows the session's moves by key (a radio group's arrows, Tab from a box) while the pages hold it,
        // unless it arrived on a part in this pass (Tab, a request), which the session follows instead.
        let arrived = cx.focus_arrived();
        let pressed_stop = self.parts.iter().filter(|part| part.focusable).find_map(|part| {
            let at = part.owner.part_ref(part.page)?;
            cx.part_if_declared(at).is_some_and(|result| matches!(result.press.phase, PressPhase::Down { .. })).then_some(at)
        });
        if let Some(at) = pressed_stop {
            cx.focus_part(at);
        } else if let Some(owner) = self.focus
            && arrived.is_none()
            && cx.is_focused()
            && let Some(at) = focus_stop(self.parts, owner)
        {
            cx.focus_part(at);
        }
        for part in self.parts {
            let Some(at) = part.owner.part_ref(part.page) else { continue };
            let Some(result) = cx.part_if_declared(at) else { continue };
            let (press, hovered) = (result.press, result.hovered);
            if self.report.context.is_none() {
                self.report.context = press.context;
            }
            if hovered && self.report.hovered.is_none() {
                self.report.hovered = Some((part.owner, part.page));
            }
            if press.phase.is_press() && self.report.press.is_none() {
                self.report.press = Some(PagePress { owner: part.owner, page: part.page, phase: press.phase, mods: press.mods });
            }
        }
        let field_owner = cx
            .active_part()
            .and_then(|active| self.part_named(active).filter(|part| part.focusable).map(|part| (Some(active), part.owner)))
            .or_else(|| {
                (cx.is_focused() || cx.active_part().is_some())
                    .then_some(self.focus)
                    .flatten()
                    .filter(|owner| matches!(owner, Owner::Field { .. }))
                    .map(|owner| (cx.active_part(), owner))
            });
        if let Some((active, owner)) = field_owner {
            cx.claim(TAB);
            cx.claim(SHIFT_TAB);
            if cx.take_claimed(TAB) {
                self.report.tab = Some((owner, true));
            } else if cx.take_claimed(SHIFT_TAB) {
                self.report.tab = Some((owner, false));
            } else if let Some(active) = active
                && let Some(intent) = Self::field_key(cx, active)
            {
                self.report.key = Some((owner, intent));
            }
        }
        if let Some(arrived) = cx.focus_arrived() {
            self.report.focused = self.part_named(arrived).map(|part| (part.owner, part.page));
        }
    }

    fn finish(self, cx: Cx<'_, Self>) -> PagePartsOutput {
        let Report { press, context, key, tab, hovered, focused } = self.report;
        PagePartsOutput { response: cx.response.clone(), press, context, key, tab, hovered, focused }
    }
}
