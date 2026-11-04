//! The cursor the pages show, and the tip beside it, set once a frame.
//!
//! A page module claims the cursor for what it has under the pointer, on a [`Layer`], and never
//! sets egui's cursor itself: where each module set it, the last write of a frame won, so the
//! order of the per-page chain decided the cursor and not what a press there does. A link's hand
//! showed over every tool though only the Select tool follows links, and the I-beam of the text
//! under a comment hid that a drag there moves the comment. `canvas::document_area` sets the
//! cursor once, from the claim on the highest layer the active tool reaches. The tip beside it is
//! that claim's own (a link's target, so only where a press acts on the link: the Select tool
//! follows it, the Link tool edits it), or else what lies under the pointer says (a comment, or a
//! signature's validation badge), with any tool, as Acrobat shows a comment's tooltip. A test in
//! `page_pointer/tests.rs` keeps page modules from setting the cursor.

use egui::CursorIcon;

use crate::QuickTool;

/// What a claim is for, from the bottom up: a claim on a higher layer wins.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Layer {
    /// The page under the active tool: the crosshair it places or draws with, its I-beam or hand.
    ToolArea,
    /// The page's text, to select; Edit text's paragraphs, and its pictures beneath them.
    Text,
    /// Links.
    Link,
    /// Text markup (highlights, underlines…), which lies on the text: under a form field.
    Comment,
    /// Form fields.
    Field,
    /// What a drag moves whole: notes, shapes, signatures and Fill & Sign's marks, added text and
    /// pictures, the objects selected together, Prepare's fields.
    Placed,
    /// The grips of the selected item: resize, rewrap, rotate and crop handles, paragraph edges,
    /// the frame of the paragraph open for typing.
    Handle,
    /// An open editor: a paragraph being typed in, a picture being cropped.
    Editor,
    /// A drag in progress: what is dragged keeps its cursor until the release.
    Drag,
}

/// Every layer, bottom first.
const LAYERS: [Layer; 9] =
    [Layer::ToolArea, Layer::Text, Layer::Link, Layer::Comment, Layer::Field, Layer::Placed, Layer::Handle, Layer::Editor, Layer::Drag];

impl Layer {
    /// Whether a press with `tool` acts on what lies on this layer, so that the layer's claims
    /// count. Only the Select tool follows links (the Link tool edits them) and selects text
    /// markup: every other tool draws, places or selects text over them. The other layers are
    /// claimed only by the modules of the tools that act on them (each module's hit test filters
    /// by tool), so they count for every tool.
    pub(crate) fn reached_by(self, tool: QuickTool) -> bool {
        match self {
            Layer::Link => matches!(tool, QuickTool::Select | QuickTool::Link),
            Layer::Comment => tool == QuickTool::Select,
            Layer::ToolArea | Layer::Text | Layer::Field | Layer::Placed | Layer::Handle | Layer::Editor | Layer::Drag => true,
        }
    }
}

/// A claim on the cursor: the icon, and the tip that goes with it (a link's target).
#[derive(Clone, Debug, PartialEq)]
struct Claim {
    icon: CursorIcon,
    tip: Option<String>,
}

/// What a description tells about, from the least up: a higher one shows over a lower one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Subject {
    /// A signature's validation badge: viewer chrome over the signer's own artwork.
    SignatureBadge,
    /// A comment: who wrote it, and what it says.
    Comment,
}

/// What the page modules claim for the cursor this frame. `canvas::document_area` sets the
/// cursor from it once, after the pages ([`PageCursor::resolve`]).
#[derive(Debug, Default)]
pub(crate) struct PageCursor {
    /// The last claim on each layer, bottom layer first.
    claims: [Option<Claim>; LAYERS.len()],
    /// What lies under the pointer says ([`PageCursor::describe`]).
    description: Option<(Subject, String)>,
}

impl PageCursor {
    /// Shows `icon` while the pointer is over what lies on `layer`, unless the active tool
    /// reaches a higher layer that is claimed too.
    pub(crate) fn claim(&mut self, layer: Layer, icon: CursorIcon) {
        self.claim_with_tip(layer, icon, None);
    }

    /// As [`Self::claim`], with `tip` beside the pointer where the claim wins.
    pub(crate) fn claim_with_tip(&mut self, layer: Layer, icon: CursorIcon, tip: Option<String>) {
        if let Some(slot) = self.claims.get_mut(layer as usize) {
            *slot = Some(Claim { icon, tip });
        }
    }

    /// Says what lies under the pointer without claiming the cursor: the tip shows with any tool,
    /// beside that tool's cursor, as Acrobat shows a comment's tooltip, unless the winning claim
    /// has a tip of its own. The description of the highest subject stands, and the last of
    /// equals (the topmost of overlapping badges).
    pub(crate) fn describe(&mut self, subject: Subject, text: String) {
        if self.description.as_ref().is_none_or(|(held, _)| *held <= subject) {
            self.description = Some((subject, text));
        }
    }

    /// The claim on the highest layer `tool` reaches.
    fn winner(claims: [Option<Claim>; LAYERS.len()], tool: QuickTool) -> Option<Claim> {
        LAYERS.into_iter().zip(claims).rev().find_map(|(layer, claim)| claim.filter(|_| layer.reached_by(tool)))
    }

    /// Sets the cursor, once, to the icon of the claim on the highest layer `tool` reaches, and
    /// returns the tip: that claim's own, or else the description. With no such claim the cursor
    /// is left as it is. The claims are spent: the next frame starts with none.
    pub(crate) fn resolve(&mut self, ctx: &egui::Context, tool: QuickTool, pointer: Option<egui::Pos2>) -> Option<String> {
        self.resolve_with(ctx, tool, pointer, soft_cursor())
    }

    /// [`PageCursor::resolve`] with the high-contrast cursor `soft` on or off. With it on, a text or crosshair
    /// cursor over the page is drawn by the app, at `pointer`, and the system's is hidden.
    fn resolve_with(&mut self, ctx: &egui::Context, tool: QuickTool, pointer: Option<egui::Pos2>, soft: bool) -> Option<String> {
        let PageCursor { claims, description } = std::mem::take(self);
        let winner = Self::winner(claims, tool);
        if let Some(w) = &winner {
            match pointer.filter(|_| soft && matches!(w.icon, CursorIcon::Text | CursorIcon::Crosshair)) {
                Some(at) => {
                    ctx.set_cursor_icon(CursorIcon::None);
                    paint_soft_cursor(ctx, w.icon, at);
                }
                None => ctx.set_cursor_icon(w.icon),
            }
        }
        winner.and_then(|w| w.tip).or(description.map(|(_, text)| text))
    }
}

/// Whether the app draws its own text and crosshair cursors (`PDFCRAFT_SOFT_CURSOR` set, and not `0`). The system
/// draws them thin and, on some Windows drivers, white or not at all, so over a white page they can't be seen
/// (#5738, #6113, #6114); an opt-in way to tell whether that is the cause.
fn soft_cursor() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("PDFCRAFT_SOFT_CURSOR").is_some_and(|v| v != "0"))
}

/// Half the length of a soft cursor's arm, in points.
const SOFT_ARM: f32 = 9.0;
/// How far an I-beam's serifs reach to each side, in points.
const SOFT_SERIF: f32 = 3.0;

/// The line segments of the soft cursor for `icon`, centred on `at`: an I-beam with serifs, or a plus.
fn soft_cursor_segments(icon: CursorIcon, at: egui::Pos2) -> Vec<[egui::Pos2; 2]> {
    let (x, y) = (at.x, at.y);
    match icon {
        CursorIcon::Text => vec![
            [egui::pos2(x, y - SOFT_ARM), egui::pos2(x, y + SOFT_ARM)],
            [egui::pos2(x - SOFT_SERIF, y - SOFT_ARM), egui::pos2(x + SOFT_SERIF, y - SOFT_ARM)],
            [egui::pos2(x - SOFT_SERIF, y + SOFT_ARM), egui::pos2(x + SOFT_SERIF, y + SOFT_ARM)],
        ],
        _ => vec![[egui::pos2(x - SOFT_ARM, y), egui::pos2(x + SOFT_ARM, y)], [egui::pos2(x, y - SOFT_ARM), egui::pos2(x, y + SOFT_ARM)]],
    }
}

/// Draws the soft cursor over everything: a black line with a white halo, which shows on a white page and on a
/// dark one.
fn paint_soft_cursor(ctx: &egui::Context, icon: CursorIcon, at: egui::Pos2) {
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("pdfcraft_soft_cursor")));
    let segments = soft_cursor_segments(icon, at);
    for segment in &segments {
        painter.line_segment(*segment, egui::Stroke::new(3.5, egui::Color32::WHITE));
    }
    for segment in &segments {
        painter.line_segment(*segment, egui::Stroke::new(1.5, egui::Color32::BLACK));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::comments::CommentTool;
    use crate::fill_sign::FillTool;
    use crate::prepare::FieldTool;

    /// One of each tool. The match stops compiling when a tool is added, so that it gets a row
    /// in [`Layer::reached_by`]'s tests.
    fn tools() -> Vec<QuickTool> {
        let tools = vec![
            QuickTool::Select,
            QuickTool::Hand,
            QuickTool::Comment(CommentTool::Rectangle),
            QuickTool::Comment(CommentTool::Highlight),
            QuickTool::Measure(crate::measure_ui::Tool::Distance),
            QuickTool::Crop,
            QuickTool::EnhanceCamera,
            QuickTool::Fill(FillTool::Signature),
            QuickTool::Field(FieldTool::Text),
            QuickTool::Redact,
            QuickTool::AddText,
            QuickTool::EditText,
            QuickTool::Stamp(pdfcraft_engine::StampKind::DynApproved),
            QuickTool::CustomStamp(0),
            QuickTool::Link,
            QuickTool::SignArea { certify: false },
            QuickTool::MarqueeZoom,
            QuickTool::Snapshot,
            QuickTool::ColumnSelect,
        ];
        for tool in &tools {
            match tool {
                QuickTool::Select
                | QuickTool::Hand
                | QuickTool::Comment(_)
                | QuickTool::Measure(_)
                | QuickTool::Crop
                | QuickTool::EnhanceCamera
                | QuickTool::Fill(_)
                | QuickTool::Field(_)
                | QuickTool::Redact
                | QuickTool::AddText
                | QuickTool::EditText
                | QuickTool::Stamp(_)
                | QuickTool::CustomStamp(_)
                | QuickTool::Link
                | QuickTool::SignArea { .. }
                | QuickTool::MarqueeZoom
                | QuickTool::Snapshot
                | QuickTool::ColumnSelect => {}
            }
        }
        tools
    }

    #[test]
    fn layers_are_listed_bottom_first_in_their_order() {
        for (i, layer) in LAYERS.into_iter().enumerate() {
            assert_eq!(layer as usize, i, "{layer:?}");
        }
        assert!(LAYERS.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn links_count_for_the_select_and_link_tools_only() {
        for tool in tools() {
            assert_eq!(Layer::Link.reached_by(tool), matches!(tool, QuickTool::Select | QuickTool::Link), "{tool:?}");
        }
    }

    #[test]
    fn comments_count_for_the_select_tool_only() {
        for tool in tools() {
            assert_eq!(Layer::Comment.reached_by(tool), tool == QuickTool::Select, "{tool:?}");
        }
    }

    #[test]
    fn the_other_layers_count_for_every_tool() {
        for tool in tools() {
            for layer in [Layer::ToolArea, Layer::Text, Layer::Field, Layer::Placed, Layer::Handle, Layer::Editor, Layer::Drag] {
                assert!(layer.reached_by(tool), "{layer:?} for {tool:?}");
            }
        }
    }

    fn claims(claims: &[(Layer, CursorIcon, Option<&str>)]) -> PageCursor {
        let mut cursor = PageCursor::default();
        for (layer, icon, tip) in claims {
            cursor.claim_with_tip(*layer, *icon, tip.map(str::to_string));
        }
        cursor
    }

    fn winner(cursor: PageCursor, tool: QuickTool) -> Option<Claim> {
        PageCursor::winner(cursor.claims, tool)
    }

    #[test]
    fn the_highest_layer_the_tool_reaches_wins_with_its_tip() {
        let on_text_under_a_link = [(Layer::Text, CursorIcon::Text, None), (Layer::Link, CursorIcon::PointingHand, Some("https://example.org/"))];
        let select = winner(claims(&on_text_under_a_link), QuickTool::Select);
        assert_eq!(select, Some(Claim { icon: CursorIcon::PointingHand, tip: Some("https://example.org/".into()) }));
        // A markup tool selects the text there, and the link's tip goes with its hand.
        let highlight = winner(claims(&on_text_under_a_link), QuickTool::Comment(CommentTool::Highlight));
        assert_eq!(highlight, Some(Claim { icon: CursorIcon::Text, tip: None }));
        // A tool reaching nothing claimed leaves the cursor alone.
        assert_eq!(winner(claims(&[(Layer::Link, CursorIcon::PointingHand, None)]), QuickTool::Hand), None);
    }

    #[test]
    fn a_higher_claim_without_a_tip_hides_a_lower_ones_tip() {
        let field_over_a_link = [(Layer::Link, CursorIcon::PointingHand, Some("Go to page 2")), (Layer::Field, CursorIcon::Text, None)];
        assert_eq!(winner(claims(&field_over_a_link), QuickTool::Select), Some(Claim { icon: CursorIcon::Text, tip: None }));
    }

    #[test]
    fn the_last_claim_on_a_layer_wins() {
        let two_links = [(Layer::Link, CursorIcon::PointingHand, Some("first")), (Layer::Link, CursorIcon::PointingHand, Some("second"))];
        assert_eq!(winner(claims(&two_links), QuickTool::Select).and_then(|c| c.tip).as_deref(), Some("second"));
    }

    /// Runs one frame that sets the cursor to `before`, if any, then resolves `claims` for
    /// `tool`. Returns the frame's cursor and the tip.
    fn frame(ctx: &egui::Context, before: Option<CursorIcon>, claims: &mut PageCursor, tool: QuickTool) -> (CursorIcon, Option<String>) {
        let mut tip = None;
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            if let Some(icon) = before {
                ui.ctx().set_cursor_icon(icon);
            }
            tip = claims.resolve_with(ui.ctx(), tool, None, false);
        });
        // An input-only test has no renderer to take the font texture.
        output.textures_delta.clear();
        (output.platform_output.cursor_icon, tip)
    }

    #[test]
    fn resolve_sets_the_winners_cursor_and_spends_the_claims() {
        let ctx = egui::Context::default();
        let on_a_link = || claims(&[(Layer::ToolArea, CursorIcon::Crosshair, None), (Layer::Link, CursorIcon::PointingHand, Some("tip"))]);
        assert_eq!(frame(&ctx, None, &mut on_a_link(), QuickTool::Select), (CursorIcon::PointingHand, Some("tip".into())));
        let stamp = QuickTool::Stamp(pdfcraft_engine::StampKind::DynApproved);
        assert_eq!(frame(&ctx, None, &mut on_a_link(), stamp), (CursorIcon::Crosshair, None), "the stamp's crosshair, and no link tip");
        // Nothing the tool reaches: the cursor stays as the frame set it.
        let mut only_a_link = claims(&[(Layer::Link, CursorIcon::PointingHand, Some("tip"))]);
        only_a_link.describe(Subject::Comment, "Ana".into());
        assert_eq!(frame(&ctx, Some(CursorIcon::Help), &mut only_a_link, QuickTool::Hand), (CursorIcon::Help, Some("Ana".into())));
        // Resolving spends the claims, reached or not, and the description.
        assert_eq!(only_a_link.description, None);
        assert_eq!(winner(only_a_link, QuickTool::Select), None);
    }

    #[test]
    fn a_description_shows_with_any_tool_unless_the_winner_has_a_tip_of_its_own() {
        let ctx = egui::Context::default();
        let stamp = QuickTool::Stamp(pdfcraft_engine::StampKind::DynApproved);
        let link = (Layer::Link, CursorIcon::PointingHand, Some("https://example.org/"));
        let described = |claimed: &[(Layer, CursorIcon, Option<&str>)]| {
            let mut cursor = claims(claimed);
            cursor.describe(Subject::Comment, "Ana\nCheck this".into());
            cursor
        };
        let comment = Some("Ana\nCheck this".to_string());
        // A comment under the stamp's crosshair: the tool's cursor, and what the comment says.
        let crosshair = (Layer::ToolArea, CursorIcon::Crosshair, None);
        assert_eq!(frame(&ctx, None, &mut described(&[crosshair]), stamp), (CursorIcon::Crosshair, comment.clone()));
        // A link the tool follows speaks for itself; one it doesn't keeps its tip to itself.
        assert_eq!(
            frame(&ctx, None, &mut described(&[crosshair, link]), QuickTool::Select),
            (CursorIcon::PointingHand, Some("https://example.org/".into()))
        );
        assert_eq!(frame(&ctx, None, &mut described(&[crosshair, link]), stamp), (CursorIcon::Crosshair, comment.clone()));
        // A winner without a tip of its own, such as the Select tool's comment claim.
        let picked_up = (Layer::Comment, CursorIcon::Move, None);
        assert_eq!(frame(&ctx, None, &mut described(&[link, picked_up]), QuickTool::Select), (CursorIcon::Move, comment));
    }

    #[test]
    fn a_comment_says_more_than_a_signature_badge_and_the_last_badge_stands() {
        let badge_then_comment = {
            let mut cursor = PageCursor::default();
            cursor.describe(Subject::SignatureBadge, "Signature is valid".into());
            cursor.describe(Subject::Comment, "Ana".into());
            cursor
        };
        let comment_then_badges = {
            let mut cursor = PageCursor::default();
            cursor.describe(Subject::Comment, "Ana".into());
            cursor.describe(Subject::SignatureBadge, "Signature is valid".into());
            cursor
        };
        let two_badges = {
            let mut cursor = PageCursor::default();
            cursor.describe(Subject::SignatureBadge, "Signature is valid".into());
            cursor.describe(Subject::SignatureBadge, "Signature is invalid".into());
            cursor
        };
        let said = |cursor: PageCursor| cursor.description.map(|(_, text)| text);
        assert_eq!(said(badge_then_comment).as_deref(), Some("Ana"));
        assert_eq!(said(comment_then_badges).as_deref(), Some("Ana"), "whichever came first");
        assert_eq!(said(two_badges).as_deref(), Some("Signature is invalid"), "the badge painted last is on top");
    }

    /// One frame with `claims` for the Select tool, the pointer at (50, 60), and the soft cursor `soft`: the system cursor
    /// it ends on, and how many shapes were painted.
    fn soft_frame(icon: CursorIcon, pointer: Option<egui::Pos2>, soft: bool) -> (CursorIcon, usize) {
        let ctx = egui::Context::default();
        let mut cursor = claims(&[(Layer::ToolArea, icon, None)]);
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            cursor.resolve_with(ui.ctx(), QuickTool::Select, pointer, soft);
        });
        let shapes = output.shapes.len();
        output.textures_delta.clear();
        (output.platform_output.cursor_icon, shapes)
    }

    /// The soft cursor replaces the system's text and crosshair cursors only when it is on and the pointer is known:
    /// the system's cursor is hidden and a halo and a line are drawn for each segment. Every other cursor stays the system's.
    #[test]
    fn the_soft_cursor_replaces_the_text_and_crosshair_cursors_when_on() {
        let at = Some(egui::pos2(50.0, 60.0));
        assert_eq!(soft_frame(CursorIcon::Text, at, true), (CursorIcon::None, 6), "an I-beam is three segments, drawn twice");
        assert_eq!(soft_frame(CursorIcon::Crosshair, at, true), (CursorIcon::None, 4), "a plus is two segments, drawn twice");
        assert_eq!(soft_frame(CursorIcon::Text, at, false), (CursorIcon::Text, 0), "off: the system's");
        assert_eq!(soft_frame(CursorIcon::Text, None, true), (CursorIcon::Text, 0), "no pointer to draw at: the system's");
        assert_eq!(soft_frame(CursorIcon::PointingHand, at, true), (CursorIcon::PointingHand, 0), "a hand is not one of the two");
    }

    #[test]
    fn the_soft_cursor_is_centred_on_the_pointer() {
        let at = egui::pos2(50.0, 60.0);
        for icon in [CursorIcon::Text, CursorIcon::Crosshair] {
            let points: Vec<egui::Pos2> = soft_cursor_segments(icon, at).into_iter().flatten().collect();
            let (min, max) =
                points.iter().fold((egui::pos2(f32::MAX, f32::MAX), egui::pos2(f32::MIN, f32::MIN)), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
            assert_eq!(((min.x + max.x) / 2.0, (min.y + max.y) / 2.0), (at.x, at.y), "{icon:?}");
        }
    }
}
