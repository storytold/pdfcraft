//! Cross-viewport drop geometry. Coordinates cross windows in physical desktop pixels,
//! then return to the destination's own UI scale. Only our live native host accepts drops.
use egui::{Pos2, Rect, Vec2, ViewportId};
use pdfcraft_engine::DocId;
use std::collections::HashMap;
#[cfg(target_os = "windows")]
use std::sync::Mutex;

/// Native top-level HWND for each managed viewport, filled by the window host via
/// [`set_viewport_hwnd`] (eframe `ViewportHooks`), never by probing under the pointer.
#[cfg(target_os = "windows")]
static VIEWPORT_HWND: Mutex<HashMap<ViewportId, isize>> = Mutex::new(HashMap::new());

/// Record or clear this viewport's own top-level HWND from the native window host.
#[cfg(target_os = "windows")]
pub fn set_viewport_hwnd(id: ViewportId, hwnd: Option<isize>) {
    let Ok(mut map) = VIEWPORT_HWND.lock() else {
        return;
    };
    match hwnd {
        Some(handle) => {
            map.insert(id, handle);
        }
        None => {
            map.remove(&id);
        }
    }
}

/// The host-recorded HWND for a viewport, if the window is still alive.
#[cfg(target_os = "windows")]
fn host_hwnd(id: ViewportId) -> Option<isize> {
    VIEWPORT_HWND.lock().ok()?.get(&id).copied()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Target {
    Tabs(usize),
    Pages { doc: DocId, at: usize },
    Reject,
}

#[derive(Clone)]
struct Grid {
    doc: DocId,
    rect: Rect,
    cells: Vec<(usize, Rect)>,
    enabled: bool,
}

#[derive(Clone)]
struct Layout {
    origin: Pos2,
    scale: f32,
    content: Rect,
    tabs: Option<(Rect, Vec<(usize, Rect)>)>,
    grids: Vec<Grid>,
    /// Native top-level window under this viewport when last observed on Windows.
    #[cfg(target_os = "windows")]
    hwnd: Option<isize>,
}

impl Layout {
    /// Convert source-local points to physical desktop pixels using its viewport scale.
    fn physical(&self, local: Pos2) -> Pos2 {
        self.origin + local.to_vec2() * self.scale
    }
    /// Convert physical desktop pixels into this destination viewport's logical points.
    fn local(&self, physical: Pos2) -> Pos2 {
        ((physical - self.origin) / self.scale).to_pos2()
    }
    /// Insertion point in the destination's global `views` order for a pointer in the tab strip.
    fn tab_insert_at(tabs: &[(usize, Rect)], p: Pos2) -> usize {
        tabs.iter()
            .find(|(_, r)| p.x <= r.center().x)
            .map(|(index, _)| *index)
            .unwrap_or_else(|| tabs.last().map(|(index, _)| index.saturating_add(1)).unwrap_or(0))
    }
    /// Resolve a tab gap or enabled page grid; reject all other positions inside this window.
    fn target(&self, physical: Pos2) -> Option<Target> {
        let p = self.local(physical);
        if !self.content.contains(p) {
            return None;
        }
        if let Some((rect, tabs)) = &self.tabs
            && rect.contains(p)
        {
            return Some(Target::Tabs(Self::tab_insert_at(tabs, p)));
        }
        if let Some(grid) = self.grids.iter().find(|g| g.enabled && g.rect.contains(p))
            && let Some(at) = crate::canvas::drop_gap(&grid.cells, p)
        {
            return Some(Target::Pages { doc: grid.doc, at });
        }
        Some(Target::Reject)
    }
}

#[derive(Clone, Default)]
struct State {
    layouts: HashMap<ViewportId, Layout>,
    drag: Option<(ViewportId, Pos2)>,
}
/// Use one shared egui context key for cross-viewport drag geometry.
fn key() -> egui::Id {
    egui::Id::unique("pdfcraft-window-drop-state")
}
/// Read the current drag snapshot, defaulting to an empty registry before the first frame.
fn state(ctx: &egui::Context) -> State {
    ctx.data(|d| d.get_temp(key())).unwrap_or_default()
}
/// Mutate and store the shared drag registry within one context-data access.
fn update(ctx: &egui::Context, f: impl FnOnce(&mut State)) {
    ctx.data_mut(|d| {
        let mut s = d.get_temp::<State>(key()).unwrap_or_default();
        f(&mut s);
        d.insert_temp(key(), s);
    });
}

/// Refresh this viewport's geometry only when native drops and finite coordinates are available.
pub(crate) fn begin(ctx: &egui::Context, enabled: bool) {
    let id = ctx.viewport_id();
    let scale = ctx.pixels_per_point();
    let origin = ctx.input(|i| i.viewport().inner_rect.map(|r| r.min));
    let content = ctx.input(|i| i.viewport_rect());
    update(ctx, |s| {
        s.layouts.remove(&id);
        if enabled
            && scale.is_finite()
            && (0.01..=100.0).contains(&scale)
            && let Some(origin) = origin.filter(|p| p.x.is_finite() && p.y.is_finite())
        {
            let origin = (origin.to_vec2() * scale).to_pos2();
            s.layouts.insert(
                id,
                Layout {
                    origin,
                    scale,
                    content,
                    tabs: None,
                    grids: Vec::new(),
                    // Own HWND from the window host — never WindowFromPoint, which names a
                    // covering window when two maximized clients share an origin.
                    #[cfg(target_os = "windows")]
                    hwnd: host_hwnd(id),
                },
            );
        }
    });
}

/// Remove layouts for closed viewports so hidden windows cannot receive future drops.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn retain(ctx: &egui::Context, live: &[ViewportId]) {
    update(ctx, |s| s.layouts.retain(|id, _| live.contains(id)));
}

// WindowFromPoint names the top-level native window under the pointer. Accept a layout only when
// that root HWND equals the viewport's host-recorded handle — never by shared client origin.
/// Locate the visible native top-level window under a bounded physical pointer position.
#[cfg(target_os = "windows")]
fn top_window(p: Pos2) -> Option<isize> {
    if !p.x.is_finite() || !p.y.is_finite() || p.x.abs() > 1_000_000.0 || p.y.abs() > 1_000_000.0 {
        return None;
    }
    let hwnd = winsafe::HWND::WindowFromPoint(winsafe::POINT { x: p.x as i32, y: p.y as i32 })?;
    let hwnd = hwnd.GetAncestor(winsafe::co::GA::ROOT).unwrap_or(hwnd);
    Some(hwnd.ptr() as isize)
}

/// Whether this layout is the native window under the pointer (host HWND, not client origin).
#[cfg(target_os = "windows")]
fn accepts_native(layout: &Layout, hwnd: isize) -> bool {
    layout.hwnd == Some(hwnd)
}

/// Select a foreign viewport under the pointer, excluding obscured windows on Windows.
fn find(s: &State, source: ViewportId, physical: Pos2) -> Option<(ViewportId, Target)> {
    #[cfg(target_os = "windows")]
    let hwnd = top_window(physical)?;
    s.layouts.iter().filter(|(id, _)| **id != source).find_map(|(id, layout)| {
        #[cfg(target_os = "windows")]
        if !accepts_native(layout, hwnd) {
            return None;
        }
        layout.target(physical).map(|target| (*id, target))
    })
}

/// Resolve the current local pointer against other managed windows using physical coordinates.
pub(crate) fn foreign_target(ctx: &egui::Context, local: Pos2) -> Option<(ViewportId, Target)> {
    let s = state(ctx);
    let from = ctx.viewport_id();
    find(&s, from, s.layouts.get(&from)?.physical(local))
}

/// A queued tear-off stores absolute logical coordinates in its source viewport's scale.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn destination(ctx: &egui::Context, from: ViewportId, absolute: Pos2) -> Option<(ViewportId, Target)> {
    let s = state(ctx);
    let scale = s.layouts.get(&from)?.scale;
    find(&s, from, (absolute.to_vec2() * scale).to_pos2())
}

/// Publish or clear the source drag and repaint destinations to update insertion indicators.
pub(crate) fn drag(ctx: &egui::Context, local: Option<Pos2>) {
    let from = ctx.viewport_id();
    update(ctx, |s| s.drag = local.and_then(|p| s.layouts.get(&from).map(|l| (from, l.physical(p)))));
    for id in state(ctx).layouts.keys() {
        ctx.request_repaint_of(*id);
    }
}

/// Return a drop preview only to the destination viewport currently under the drag.
fn hover(ctx: &egui::Context) -> Option<(Pos2, Target)> {
    let s = state(ctx);
    let (source, physical) = s.drag?;
    let (id, target) = find(&s, source, physical)?;
    (id == ctx.viewport_id()).then(|| (s.layouts.get(&id).map_or(Pos2::ZERO, |l| l.local(physical)), target))
}

/// Register tab insertion gaps and paint the active destination indicator within the strip.
pub(crate) fn tabs(ui: &egui::Ui, rect: Rect, slots: &[(usize, Rect)]) {
    let ctx = ui.ctx();
    let viewport = ctx.viewport_id();
    update(ctx, |s| {
        if let Some(l) = s.layouts.get_mut(&viewport) {
            l.tabs = Some((rect, slots.to_vec()));
        }
    });
    if let Some((_, Target::Tabs(at))) = hover(ctx) {
        let x = slots
            .iter()
            .find(|(index, _)| *index == at)
            .map(|(_, r)| r.left())
            .or_else(|| slots.last().map(|(_, r)| r.right()))
            .unwrap_or(rect.left());
        let color = crate::theme::Tokens::get(ctx).accent;
        ui.painter()
            .with_clip_rect(rect)
            .line_segment([egui::pos2(x, rect.top() + 3.0), egui::pos2(x, rect.bottom() - 3.0)], egui::Stroke::new(3.0, color));
    }
}

/// Register an Organize grid and preview insertion without mutating either document.
pub(crate) fn grid(ui: &egui::Ui, doc: DocId, rect: Rect, cells: &[(usize, Rect)], enabled: bool) {
    let ctx = ui.ctx();
    let viewport = ctx.viewport_id();
    update(ctx, |s| {
        if let Some(l) = s.layouts.get_mut(&viewport) {
            l.grids.push(Grid { doc, rect, cells: cells.to_vec(), enabled });
        }
    });
    if let Some((p, Target::Pages { doc: hover_doc, at, .. })) = hover(ctx)
        && hover_doc == doc
        && rect.contains(p)
    {
        let painter = ui.painter().with_clip_rect(rect);
        let color = crate::theme::Tokens::get(ctx).accent;
        let next = cells.iter().find(|(i, _)| *i == at);
        if let Some((_, r)) = next.or(cells.last()) {
            let x = if next.is_some() { r.left() + 3.0 } else { r.right() - 3.0 };
            painter.line_segment([egui::pos2(x, r.top() + 10.0), egui::pos2(x, r.bottom() - 10.0)], egui::Stroke::new(3.0, color));
        }
        let badge = p + Vec2::splat(16.0);
        painter.circle_filled(badge, 9.0, color);
        let ink = egui::Stroke::new(1.5, egui::Color32::WHITE);
        painter.line_segment([badge - Vec2::new(4.0, 0.0), badge + Vec2::new(4.0, 0.0)], ink);
        painter.line_segment([badge - Vec2::new(0.0, 4.0), badge + Vec2::new(0.0, 4.0)], ink);
    }
}

/// Physical geometry for opt-in UI control clients coordinating drags across viewports.
pub(crate) fn inspect(ctx: &egui::Context) -> serde_json::Value {
    let s = state(ctx);
    let rect = |r: Rect| [r.min.x, r.min.y, r.max.x, r.max.y];
    serde_json::Value::Array(
        s.layouts
            .iter()
            .map(|(id, l)| {
                serde_json::json!({
                    "viewport":format!("{id:?}"), "current":*id==ctx.viewport_id(),
                    "origin_pixels":[l.origin.x,l.origin.y], "pixels_per_point":l.scale,
                    "content":rect(l.content), "tabs":l.tabs.as_ref().map(|(r,_)|rect(*r)),
                    "organize":l.grids.iter().map(|g|serde_json::json!({"doc":g.doc.0,"rect":rect(g.rect),"enabled":g.enabled})).collect::<Vec<_>>(),
                })
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Build synthetic tab and page geometry at a requested DPI scale.
    fn layout(scale: f32) -> Layout {
        Layout {
            origin: egui::pos2(-900.0, 200.0),
            scale,
            content: Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0)),
            tabs: Some((
                Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 38.0)),
                vec![
                    (0, Rect::from_min_max(egui::pos2(40.0, 0.0), egui::pos2(200.0, 38.0))),
                    (1, Rect::from_min_max(egui::pos2(200.0, 0.0), egui::pos2(360.0, 38.0))),
                ],
            )),
            grids: vec![Grid {
                doc: DocId(7),
                rect: Rect::from_min_max(egui::pos2(150.0, 100.0), egui::pos2(800.0, 600.0)),
                enabled: true,
                cells: vec![
                    (5, Rect::from_min_max(egui::pos2(200.0, 150.0), egui::pos2(400.0, 400.0))),
                    (6, Rect::from_min_max(egui::pos2(400.0, 150.0), egui::pos2(600.0, 400.0))),
                ],
            }],
            #[cfg(target_os = "windows")]
            hwnd: None,
        }
    }
    /// Cross-scale coordinate conversion retains the intended tab gap and page insertion position.
    #[test]
    fn window_drop_geometry_preserves_tab_gaps_and_page_positions_across_dpi() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let l = layout(scale);
            for (point, target) in [
                (egui::pos2(50.0, 20.0), Target::Tabs(0)),
                (egui::pos2(190.0, 20.0), Target::Tabs(1)),
                (egui::pos2(700.0, 20.0), Target::Tabs(2)),
                (egui::pos2(230.0, 220.0), Target::Pages { doc: DocId(7), at: 5 }),
                (egui::pos2(380.0, 220.0), Target::Pages { doc: DocId(7), at: 6 }),
                (egui::pos2(700.0, 500.0), Target::Pages { doc: DocId(7), at: 7 }),
                (egui::pos2(60.0, 250.0), Target::Reject),
            ] {
                assert_eq!(l.target(l.physical(point)), Some(target));
            }
            assert_eq!(l.target(l.physical(egui::pos2(900.0, 200.0))), None);
            assert_eq!(l.target(egui::pos2(f32::NAN, 200.0)), None);
        }
    }
    /// Disabled or absent Organize grids reject drops rather than copying pages.
    #[test]
    fn window_drop_does_not_insert_into_disabled_or_closed_organizer() {
        let mut l = layout(1.5);
        let p = l.physical(egui::pos2(300.0, 250.0));
        l.grids[0].enabled = false;
        assert_eq!(l.target(p), Some(Target::Reject));
        l.grids.clear();
        assert_eq!(l.target(p), Some(Target::Reject));
    }

    /// Tab gaps use each visible tab's global view index, not its position in the strip alone.
    #[test]
    fn window_drop_tab_gaps_use_global_view_indices() {
        let mut l = layout(1.0);
        l.tabs = Some((
            Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 38.0)),
            vec![
                (1, Rect::from_min_max(egui::pos2(40.0, 0.0), egui::pos2(200.0, 38.0))),
                (3, Rect::from_min_max(egui::pos2(200.0, 0.0), egui::pos2(360.0, 38.0))),
            ],
        ));
        assert_eq!(l.target(l.physical(egui::pos2(50.0, 20.0))), Some(Target::Tabs(1)));
        assert_eq!(l.target(l.physical(egui::pos2(190.0, 20.0))), Some(Target::Tabs(3)));
        assert_eq!(l.target(l.physical(egui::pos2(700.0, 20.0))), Some(Target::Tabs(4)));
    }

    /// Each Organize pane keeps its own grid, so a drop picks the pane under the pointer.
    #[test]
    fn window_drop_keeps_each_organize_pane_grid() {
        let mut l = layout(1.0);
        l.grids = vec![
            Grid {
                doc: DocId(1),
                rect: Rect::from_min_max(egui::pos2(0.0, 100.0), egui::pos2(400.0, 600.0)),
                enabled: true,
                cells: vec![(0, Rect::from_min_max(egui::pos2(40.0, 150.0), egui::pos2(180.0, 400.0)))],
            },
            Grid {
                doc: DocId(2),
                rect: Rect::from_min_max(egui::pos2(400.0, 100.0), egui::pos2(800.0, 600.0)),
                enabled: true,
                cells: vec![(0, Rect::from_min_max(egui::pos2(440.0, 150.0), egui::pos2(580.0, 400.0)))],
            },
        ];
        assert_eq!(l.target(l.physical(egui::pos2(80.0, 220.0))), Some(Target::Pages { doc: DocId(1), at: 0 }));
        assert_eq!(l.target(l.physical(egui::pos2(480.0, 220.0))), Some(Target::Pages { doc: DocId(2), at: 0 }));
        l.grids[0].enabled = false;
        assert_eq!(l.target(l.physical(egui::pos2(80.0, 220.0))), Some(Target::Reject));
    }

    /// Native identity is the host HWND alone: a matching client origin must not claim another window.
    #[cfg(target_os = "windows")]
    #[test]
    fn window_drop_accepts_only_the_host_recorded_hwnd() {
        let mut covered = layout(1.0);
        let mut covering = layout(1.0);
        covered.origin = egui::pos2(0.0, 0.0);
        covering.origin = egui::pos2(0.0, 0.0);
        covered.hwnd = Some(0x1111);
        covering.hwnd = Some(0x2222);
        assert!(accepts_native(&covering, 0x2222));
        assert!(!accepts_native(&covered, 0x2222), "a covered window must not match the covering HWND");
        assert!(!accepts_native(&covered, 0x2222));
        covered.hwnd = None;
        assert!(!accepts_native(&covered, 0x2222), "origin alone is not window identity");
        set_viewport_hwnd(ViewportId::ROOT, Some(0x3333));
        assert_eq!(host_hwnd(ViewportId::ROOT), Some(0x3333));
        set_viewport_hwnd(ViewportId::ROOT, None);
        assert_eq!(host_hwnd(ViewportId::ROOT), None);
    }
}
