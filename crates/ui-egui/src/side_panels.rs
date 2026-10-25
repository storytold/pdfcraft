//! The side panels around the page: the Tools pane, the rail and the open side panel
//! ([`crate::RightPanel`]), drawn with craft-ui task panes ([`craft_ui::panes`]).
//!
//! On the right, outermost first: the rail, the Tools pane when Preferences ▸ Tools panel side is
//! Right, then the side panel, then the page. Panes are docked only; their widths are resizable
//! and saved with the preferences.

use std::ops::RangeInclusive;

use craft_ui::panes::{Edge, PaneContent, PaneHost, PaneLabels, PaneRequest, PaneState, SidePolicy};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{Mode, PdfCraftApp};

/// Default width of the Tools pane.
pub const TOOLS_WIDTH: f32 = 272.0;
/// Default width of the side panel.
pub const SIDE_WIDTH: f32 = 330.0;
const TOOLS_RANGE: RangeInclusive<f32> = 240.0..=520.0;
const SIDE_RANGE: RangeInclusive<f32> = 260.0..=520.0;
/// Version of the saved `side_panels` value.
const VERSION: u64 = 1;
/// Deepest split tree the old layout migration walks; the old renderer allowed far fewer.
const MAX_DEPTH: usize = 32;

/// The two panes [`PaneHost`] draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pane {
    Tools,
    /// The open [`crate::RightPanel`].
    Side,
}

impl Pane {
    pub fn parse(id: &str) -> Option<Self> {
        match id {
            "tools" => Some(Self::Tools),
            "side" => Some(Self::Side),
            _ => None,
        }
    }

    fn range(self) -> RangeInclusive<f32> {
        match self {
            Self::Tools => TOOLS_RANGE,
            Self::Side => SIDE_RANGE,
        }
    }
}

/// Preferences ▸ Tools panel side.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolsSide {
    #[default]
    Left,
    Right,
}

/// The saved side panel layout. Which side panel is open stays in `PdfCraftApp::right` and the
/// Tools pane's open state in `PdfCraftApp::left_open`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SidePanels {
    /// The one setting that places the Tools pane; [`SidePanels::tools_edge`] is its only reader
    /// in the layout.
    pub tools_side: ToolsSide,
    pub tools_width: f32,
    pub side_width: f32,
}

impl Default for SidePanels {
    fn default() -> Self {
        Self { tools_side: ToolsSide::Left, tools_width: TOOLS_WIDTH, side_width: SIDE_WIDTH }
    }
}

/// A width from settings or automation: finite, within the pane's limits.
fn clamp_width(pane: Pane, width: f64) -> Option<f32> {
    let range = pane.range();
    width.is_finite().then(|| (width as f32).clamp(*range.start(), *range.end()))
}

/// What [`restore`] read from the saved preferences.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Restored {
    pub panels: SidePanels,
    /// `None`: not saved; keep the default.
    pub tools_open: Option<bool>,
    /// An old layout had the side panel closed.
    pub side_closed: bool,
}

impl SidePanels {
    pub fn tools_on_right(&self) -> bool {
        self.tools_side == ToolsSide::Right
    }

    fn tools_edge(&self) -> Edge {
        match self.tools_side {
            ToolsSide::Left => Edge::Start,
            ToolsSide::Right => Edge::End,
        }
    }

    pub fn width(&self, pane: Pane) -> f32 {
        match pane {
            Pane::Tools => self.tools_width,
            Pane::Side => self.side_width,
        }
    }

    fn set_width(&mut self, pane: Pane, width: f32) {
        let Some(width) = clamp_width(pane, f64::from(width)) else { return };
        match pane {
            Pane::Tools => self.tools_width = width,
            Pane::Side => self.side_width = width,
        }
    }

    /// The saved value (`side_panels` in the preferences).
    pub fn to_json(&self, tools_open: bool) -> Value {
        json!({
            "version": VERSION,
            "tools_side": self.tools_side,
            "tools_open": tools_open,
            "tools_width": self.tools_width,
            "side_width": self.side_width,
        })
    }
}

/// Reads the side panel layout from saved preferences: `side_panels`, else the old `docking`
/// value, else the defaults. Nothing in it is an error: a malformed or unknown value gives the
/// defaults.
pub fn restore(saved: &Value) -> Restored {
    let defaults = Restored { panels: SidePanels::default(), tools_open: None, side_closed: false };
    if let Some(value) = saved.get("side_panels") {
        if value.get("version").and_then(Value::as_u64) != Some(VERSION) {
            return defaults;
        }
        let mut panels = SidePanels::default();
        if let Some(side) = value.get("tools_side").and_then(|v| serde_json::from_value::<ToolsSide>(v.clone()).ok()) {
            panels.tools_side = side;
        }
        if let Some(width) = value.get("tools_width").and_then(Value::as_f64).and_then(|w| clamp_width(Pane::Tools, w)) {
            panels.tools_width = width;
        }
        if let Some(width) = value.get("side_width").and_then(Value::as_f64).and_then(|w| clamp_width(Pane::Side, w)) {
            panels.side_width = width;
        }
        return Restored { panels, tools_open: value.get("tools_open").and_then(Value::as_bool), side_closed: false };
    }
    let Some(mut restored) = saved.get("docking").and_then(migrate) else { return defaults };
    if let Some(open) = saved.get("docking_left_open").and_then(Value::as_bool) {
        restored.tools_open = Some(open);
    }
    restored
}

/// Where the old docking tree held a panel.
struct Found {
    /// The panel's group held only it: a tab group or stack of its own.
    alone: bool,
    /// The innermost split around the panel's own group.
    split: Option<Split>,
}

struct Split {
    horizontal: bool,
    /// The panel is in the split's second (right or bottom) child.
    second: bool,
    /// The split's fixed size, when it is the panel's side.
    size: Option<f64>,
}

/// The pixel size of `size` (`{"FixedFirst": w}` or `{"FixedSecond": w}`) when it fixes the side
/// the panel is on.
fn fixed_size(size: Option<&Value>, second: bool) -> Option<f64> {
    let key = if second { "FixedSecond" } else { "FixedFirst" };
    size?.get(key)?.as_f64()
}

fn group_ids(node: &Value) -> Option<Vec<&str>> {
    if let Some(tabs) = node.get("Tabs") {
        return Some(tabs.get("panels")?.as_array()?.iter().filter_map(Value::as_str).collect());
    }
    let entries = node.get("Stack")?.get("entries")?.as_array()?;
    Some(entries.iter().filter_map(|entry| entry.get("panel")?.as_str()).collect())
}

fn find(node: &Value, panel: &str, depth: usize) -> Option<Found> {
    if depth > MAX_DEPTH {
        return None;
    }
    if let Some(ids) = group_ids(node) {
        return ids.contains(&panel).then(|| Found { alone: ids.iter().all(|id| *id == panel), split: None });
    }
    let split = node.get("Split")?;
    for (key, second) in [("first", false), ("second", true)] {
        if let Some(mut found) = split.get(key).and_then(|child| find(child, panel, depth + 1)) {
            // A panel sharing its group was placed by its group, not by a split.
            if found.alone && found.split.is_none() {
                let horizontal = split.get("axis").and_then(Value::as_str) == Some("Horizontal");
                found.split = Some(Split { horizontal, second, size: fixed_size(split.get("size"), second) });
            }
            return Some(found);
        }
    }
    None
}

/// The old layout's saved place of a closed panel: whether it was on the right and its width.
fn hidden_place<'a>(old: &'a Value, panel: &str) -> Option<&'a Value> {
    old.get("hidden")?.as_array()?.iter().find_map(|entry| {
        let pair = entry.as_array()?;
        (pair.first()?.as_str()? == panel).then(|| pair.get(1)).flatten()
    })
}

fn floating_contains(old: &Value, panel: &str) -> bool {
    old.get("layout").and_then(|l| l.get("floating")).and_then(Value::as_array).is_some_and(|groups| {
        groups.iter().any(|group| group.get("panels").and_then(Value::as_array).is_some_and(|ids| ids.iter().any(|id| id.as_str() == Some(panel))))
    })
}

/// A panel of the old layout: on the right edge, open, and its width where the layout stated it
/// in points. A floating or tabbed panel counts as on the left, as the old Tools side reading did.
fn old_panel(old: &Value, panel: &str) -> (bool, bool, Option<f64>) {
    let root = old.get("layout").and_then(|l| l.get("root"));
    if let Some(found) = root.and_then(|root| find(root, panel, 0)) {
        return match found.split {
            Some(split) if split.horizontal => (split.second, true, split.size),
            _ => (false, true, None),
        };
    }
    if floating_contains(old, panel) {
        return (false, true, None);
    }
    let Some(place) = hidden_place(old, panel) else { return (false, false, None) };
    let right = place.get("placement").and_then(|p| p.get("Split")).and_then(Value::as_str) == Some("Right");
    let left = place.get("placement").and_then(|p| p.get("Split")).and_then(Value::as_str) == Some("Left");
    let size = if right || left { fixed_size(place.get("size"), right) } else { None };
    (right, false, size)
}

/// Reads the old docking layout (craft-ui's `Workspace`) as plain JSON. `None` when it has no
/// document view, which the old layout always held.
fn migrate(old: &Value) -> Option<Restored> {
    find(old.get("layout")?.get("root")?, "canvas", 0)?;
    let mut panels = SidePanels::default();
    let (tools_right, tools_open, tools_width) = old_panel(old, "tools");
    let (_, side_open, side_width) = old_panel(old, "inspector");
    if tools_right {
        panels.tools_side = ToolsSide::Right;
    }
    if let Some(width) = tools_width.and_then(|w| clamp_width(Pane::Tools, w)) {
        panels.tools_width = width;
    }
    if let Some(width) = side_width.and_then(|w| clamp_width(Pane::Side, w)) {
        panels.side_width = width;
    }
    Some(Restored { panels, tools_open: Some(tools_open), side_closed: !side_open })
}

/// What `ui.panel` and `ui.state` report.
pub fn state(app: &PdfCraftApp) -> Value {
    let panels = &app.side_panels;
    json!({
        "tools_side": panels.tools_side,
        "tools_open": app.left_open,
        "tools_width": panels.tools_width,
        "side_open": app.right.is_some(),
        "side_panel": app.right.map(|r| format!("{r:?}")),
        "side_width": panels.side_width,
    })
}

/// `ui.panel {panel, open?, width?}` and `ui.dock.reset` (`reset`).
pub fn command(app: &mut PdfCraftApp, reset: bool, params: &Value) -> Result<Value, String> {
    if reset {
        app.side_panels = SidePanels::default();
        app.left_open = true;
        return Ok(json!({ "side_panels": state(app) }));
    }
    let pane = Pane::parse(params.get("panel").and_then(Value::as_str).ok_or("panel must be \"tools\" or \"side\"")?)
        .ok_or("panel must be \"tools\" or \"side\"")?;
    let open = match params.get("open") {
        None | Some(Value::Null) => None,
        Some(value) => Some(value.as_bool().ok_or("open must be a boolean")?),
    };
    let width = match params.get("width") {
        None | Some(Value::Null) => None,
        Some(value) => Some(value.as_f64().and_then(|w| clamp_width(pane, w)).ok_or("width must be a finite number")?),
    };
    if let Some(width) = width {
        app.side_panels.set_width(pane, width);
    }
    match (pane, open) {
        (Pane::Tools, Some(open)) => app.left_open = open,
        (Pane::Side, Some(true)) if app.right.is_none() => app.toggle_side_panel(),
        (Pane::Side, Some(false)) if app.right.is_some() => app.choose_right_panel(None),
        _ => {}
    }
    Ok(json!({ "side_panels": state(app) }))
}

struct Content<'a> {
    app: &'a mut PdfCraftApp,
}

impl PaneContent<Pane> for Content<'_> {
    fn title(&self, pane: &Pane) -> String {
        crate::i18n::t(match pane {
            Pane::Tools => "Tools",
            Pane::Side => "Inspector",
        })
        .to_owned()
    }

    fn body(&mut self, ui: &mut egui::Ui, pane: &Pane) {
        match pane {
            Pane::Tools => crate::panels::left_panel(self.app, ui),
            Pane::Side => {
                ui.data_mut(|data| data.insert_temp(egui::Id::new("pdfcraft-inspector-body"), ui.max_rect()));
                crate::panels::right_panel(self.app, ui);
            }
        }
    }

    fn default_state(&self, pane: &Pane) -> PaneState {
        state_of(&self.app.side_panels, *pane)
    }

    fn size_range(&self, pane: &Pane) -> RangeInclusive<f32> {
        pane.range()
    }

    fn movable(&self, _pane: &Pane) -> bool {
        false
    }

    fn title_bar(&self, _pane: &Pane) -> bool {
        // Both panels draw their own header and Close.
        false
    }
}

fn state_of(panels: &SidePanels, pane: Pane) -> PaneState {
    let edge = match pane {
        Pane::Tools => panels.tools_edge(),
        Pane::Side => Edge::End,
    };
    PaneState::docked(edge, panels.width(pane))
}

/// The panes that show this frame, outermost first on a shared edge.
fn open_panes(app: &PdfCraftApp) -> Vec<Pane> {
    let mut open = Vec::new();
    // Read mode hides the tools, not the navigation panes.
    if app.left_open && app.mode != Mode::Read {
        open.push(Pane::Tools);
    }
    // Home and Combine files have no side panel; the choice is kept for the next document.
    if app.active.is_some() && app.right.is_some() {
        open.push(Pane::Side);
    }
    open
}

/// Draws the rail and the open panes. Call after the top bars and before the central panel.
pub fn show(app: &mut PdfCraftApp, ui: &mut egui::Ui) {
    if let Some(panel) = app.right {
        app.last_right = panel;
    }
    // The rail first: it is the outermost panel on the right.
    if app.active.is_some() {
        crate::chrome::right_rail(app, ui);
    }
    // The expanded All tools view covers the panes.
    if app.mode == Mode::AllTools && app.all_tools_full {
        return;
    }
    let open = open_panes(app);
    if open.is_empty() {
        return;
    }
    let t = crate::theme::Tokens::get(ui.ctx());
    let labels = PaneLabels { resize: crate::i18n::t("Resize panels").to_owned(), ..PaneLabels::default() };
    let panels = app.side_panels;
    // Columns: both panes may share the right edge, Tools outermost, as `open` lists them.
    let output = PaneHost::new("pdfcraft-side-panels")
        .policy(SidePolicy::Columns)
        .f6(false)
        .labels(labels)
        .frame(egui::Frame::NONE.fill(t.panel))
        .body_margin(egui::Margin::ZERO)
        .show(ui, &open, |pane| Some(state_of(&panels, *pane)), &mut Content { app });
    for request in output.requests {
        match request {
            PaneRequest::Resize(pane, width) => app.side_panels.set_width(pane, width),
            PaneRequest::Close(Pane::Tools) => app.left_open = false,
            PaneRequest::Close(Pane::Side) => app.choose_right_panel(None),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tabs(panels: &[&str]) -> Value {
        json!({"Tabs": {"panels": panels, "active": 0}})
    }

    fn split(size: Value, first: Value, second: Value) -> Value {
        json!({"Split": {"axis": "Horizontal", "size": size, "first": first, "second": second}})
    }

    fn workspace(root: Value, floating: Value, hidden: Value) -> Value {
        json!({"docking": {"layout": {"root": root, "floating": floating}, "hidden": hidden}})
    }

    /// The old default layout, as craft-ui 5c874d5 serialized it.
    fn docked(tools: f64, inspector: f64) -> Value {
        workspace(
            split(json!({"FixedFirst": tools}), tabs(&["tools"]), split(json!({"FixedSecond": inspector}), tabs(&["canvas"]), tabs(&["inspector"]))),
            json!([]),
            json!([]),
        )
    }

    #[test]
    fn docked_default_keeps_widths() {
        let restored = restore(&docked(300.0, 400.0));
        assert_eq!(restored.panels, SidePanels { tools_side: ToolsSide::Left, tools_width: 300.0, side_width: 400.0 });
        assert_eq!((restored.tools_open, restored.side_closed), (Some(true), false));
    }

    #[test]
    fn tools_on_the_right_moves_right() {
        // `set_tools_on_right` put Tools to the right of the Inspector.
        let old = workspace(
            split(json!({"FixedSecond": 330.0}), tabs(&["canvas"]), split(json!({"FixedSecond": 290.0}), tabs(&["inspector"]), tabs(&["tools"]))),
            json!([]),
            json!([]),
        );
        let restored = restore(&old);
        assert_eq!(restored.panels.tools_side, ToolsSide::Right);
        assert_eq!(restored.panels.tools_width, 290.0);
        // The Inspector's own split fixes Tools' width, not its.
        assert_eq!(restored.panels.side_width, SIDE_WIDTH);
    }

    #[test]
    fn floated_tools_docks_left_and_stays_open() {
        let old = workspace(
            split(json!({"FixedSecond": 360.0}), tabs(&["canvas"]), tabs(&["inspector"])),
            json!([{"panels": ["tools"], "active": 0, "rect": [10.0, 20.0, 300.0, 400.0]}]),
            json!([]),
        );
        let restored = restore(&old);
        assert_eq!(restored.panels, SidePanels { tools_side: ToolsSide::Left, tools_width: TOOLS_WIDTH, side_width: 360.0 });
        assert_eq!((restored.tools_open, restored.side_closed), (Some(true), false));
    }

    #[test]
    fn tools_tabbed_with_the_inspector_docks_both() {
        let old = workspace(split(json!({"FixedSecond": 350.0}), tabs(&["canvas"]), tabs(&["inspector", "tools"])), json!([]), json!([]));
        let restored = restore(&old);
        assert_eq!(restored.panels.tools_side, ToolsSide::Left);
        assert_eq!((restored.panels.tools_width, restored.panels.side_width), (TOOLS_WIDTH, SIDE_WIDTH));
        assert_eq!((restored.tools_open, restored.side_closed), (Some(true), false));
    }

    #[test]
    fn a_hidden_inspector_stays_closed_and_hidden_tools_keeps_its_side() {
        let old = workspace(
            tabs(&["canvas"]),
            json!([]),
            json!([
                ["inspector", {"floating": null, "anchor": "canvas", "placement": {"Split": "Right"}, "size": {"FixedSecond": 380.0}, "stack": null, "sibling": ["canvas"]}],
                ["tools", {"floating": null, "anchor": "canvas", "placement": {"Split": "Right"}, "size": {"FixedSecond": 250.0}, "stack": null, "sibling": ["canvas"]}]
            ]),
        );
        let restored = restore(&old);
        assert_eq!(restored.panels, SidePanels { tools_side: ToolsSide::Right, tools_width: 250.0, side_width: 380.0 });
        assert_eq!((restored.tools_open, restored.side_closed), (Some(false), true));
    }

    #[test]
    fn malformed_values_give_the_defaults() {
        let defaults = Restored { panels: SidePanels::default(), tools_open: None, side_closed: false };
        for saved in [
            json!({"docking": "not a layout"}),
            json!({"docking": {"layout": [1, 2]}}),
            json!({"docking": {"layout": {"root": {"Split": {"axis": 3, "size": {"FixedFirst": "wide"}, "first": null}}}}}),
            json!({"side_panels": {"version": 2, "tools_side": "right"}}),
            json!({"side_panels": "right"}),
        ] {
            let restored = restore(&saved);
            assert_eq!(restored, defaults, "{saved}");
        }
        // Non-finite and out-of-range widths fall back or clamp; an unknown side keeps Left.
        let restored = restore(&json!({"side_panels": {"version": 1, "tools_side": "up", "tools_width": 9000.0, "side_width": "x"}}));
        assert_eq!(restored.panels, SidePanels { tools_side: ToolsSide::Left, tools_width: 520.0, side_width: SIDE_WIDTH });
        // Deep trees are cut off rather than walked.
        let mut deep = tabs(&["tools"]);
        for _ in 0..200 {
            deep = split(json!({"Ratio": 0.5}), tabs(&["canvas"]), deep);
        }
        assert_eq!(restore(&workspace(deep, json!([]), json!([]))).panels.tools_side, ToolsSide::Left);
    }

    #[test]
    fn saved_value_round_trips() {
        let panels = SidePanels { tools_side: ToolsSide::Right, tools_width: 300.0, side_width: 410.0 };
        let restored = restore(&json!({ "side_panels": panels.to_json(false) }));
        assert_eq!(restored, Restored { panels, tools_open: Some(false), side_closed: false });
    }
}
