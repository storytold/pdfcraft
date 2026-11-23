//! 2D Spatial Drag Hit-Tester, caravan drop resolver, dwell auto-expand, edge
//! auto-scroll, and cursor ghost rendering for the Bookmarks sidebar.

use std::time::Duration;

use pdfcraft_engine::OutlinePlacementKind;

use super::super::{Color32, CornerRadius, Rect, Stroke, Tokens, pos2, vec2};
use super::{BookmarkDragState, BookmarkKey, BookmarkPanelState, ProjectedTree};

pub(super) const INDENT_STEP: f32 = 16.0;
const BASE_PAD: f32 = 4.0;
const EDGE_SCROLL_ZONE: f32 = 36.0;
const MAX_EDGE_SCROLL_STEP: f32 = 12.0;
const DWELL_EXPAND_SECS: f64 = 0.45;
const DWELL_MAX_MOVE_PX: f32 = 4.0;

#[derive(Clone, Debug, PartialEq)]
pub(super) enum DropTargetKind {
    /// Horizontal gap at `y` targeting `depth` (1-based).
    Line { y: f32, depth: u32, target_node_idx: usize, placement: OutlinePlacementKind },
    /// Directly inside `target_node_idx` (Center 50% band).
    IntoRow { target_node_idx: usize, rect: Rect },
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct EvaluatedDrop {
    pub kind: DropTargetKind,
    pub valid: bool,
    pub expanded_parent_key: Option<BookmarkKey>,
}

/// Evaluate the live drag pointer against rendered visible row boxes and `scroll_rect`.
pub(super) fn evaluate_drop_hit(
    pointer: egui::Pos2,
    scroll_rect: Rect,
    row_rects: &[(usize, usize, Rect)],
    proj: &ProjectedTree,
    drag_roots: &[usize],
) -> Option<EvaluatedDrop> {
    if row_rects.is_empty() {
        return None;
    }

    // Horizontal guard with slight slop so dragging just beside the rail edge remains fluid.
    if pointer.x < scroll_rect.left() - 24.0 || pointer.x > scroll_rect.right() + 24.0 {
        return None;
    }

    let d_raw = (((pointer.x - scroll_rect.left() - BASE_PAD).max(0.0) / INDENT_STEP).floor() as u32).saturating_add(1);

    for &(vis_pos, node_idx, rect) in row_rects {
        if pointer.y < rect.top() || pointer.y > rect.bottom() {
            continue;
        }
        let rel = (pointer.y - rect.top()) / rect.height().max(1.0);
        if rel < 0.25 {
            let upper = vis_pos.checked_sub(1).and_then(|vp| proj.visible_rows.get(vp).copied());
            return resolve_gap(upper, Some(node_idx), rect.top(), d_raw, proj, drag_roots);
        }
        if rel <= 0.75 {
            let valid = !is_self_or_descendant(proj, drag_roots, node_idx);
            let expanded_parent_key = proj.nodes.get(node_idx).map(|n| n.key.clone());
            return Some(EvaluatedDrop { kind: DropTargetKind::IntoRow { target_node_idx: node_idx, rect }, valid, expanded_parent_key });
        }
        let lower = proj.visible_rows.get(vis_pos.saturating_add(1)).copied();
        return resolve_gap(Some(node_idx), lower, rect.bottom(), d_raw, proj, drag_roots);
    }

    // Blank space below the last rendered row at the end of the outline tree.
    if let Some(&(last_vis, last_node, last_rect)) = row_rects.last()
        && pointer.y > last_rect.bottom()
        && pointer.y <= scroll_rect.bottom() + 8.0
        && last_vis.saturating_add(1) == proj.visible_rows.len()
    {
        return resolve_gap(Some(last_node), None, last_rect.bottom(), d_raw, proj, drag_roots);
    }

    // Above the first rendered row when scrolled at top.
    if let Some(&(first_vis, first_node, first_rect)) = row_rects.first()
        && pointer.y < first_rect.top()
        && pointer.y >= scroll_rect.top() - 8.0
        && first_vis == 0
    {
        return resolve_gap(None, Some(first_node), first_rect.top(), d_raw, proj, drag_roots);
    }

    None
}

fn resolve_gap(upper: Option<usize>, lower: Option<usize>, y: f32, d_raw: u32, proj: &ProjectedTree, drag_roots: &[usize]) -> Option<EvaluatedDrop> {
    let (target_node_idx, placement, depth) = match (upper, lower) {
        (None, Some(l)) => (l, OutlinePlacementKind::Before, 1),
        (Some(u), Some(l)) => {
            let u_node = proj.nodes.get(u)?;
            let l_node = proj.nodes.get(l)?;
            let d_u = u_node.depth.max(1);
            let d_l = l_node.depth.max(1);
            let (d_min, d_max) = if d_u < d_l { (d_l, d_l) } else { (d_l, d_u.saturating_add(1)) };
            let d_target = d_raw.clamp(d_min, d_max);
            if d_target == d_u.saturating_add(1) {
                if u_node.has_children && u_node.is_expanded && d_u < d_l {
                    (l, OutlinePlacementKind::Before, d_target)
                } else {
                    (u, OutlinePlacementKind::FirstChild, d_target)
                }
            } else {
                let anchor = ancestor_at_depth(proj, u, d_target)?;
                (anchor, OutlinePlacementKind::After, d_target)
            }
        }
        (Some(u), None) => {
            let u_node = proj.nodes.get(u)?;
            let d_u = u_node.depth.max(1);
            let d_target = d_raw.clamp(1, d_u.saturating_add(1));
            if d_target == d_u.saturating_add(1) {
                (u, OutlinePlacementKind::LastChild, d_target)
            } else {
                let anchor = ancestor_at_depth(proj, u, d_target)?;
                (anchor, OutlinePlacementKind::After, d_target)
            }
        }
        (None, None) => return None,
    };

    let valid = !is_self_or_descendant(proj, drag_roots, target_node_idx);
    let expanded_parent_key = match placement {
        OutlinePlacementKind::FirstChild | OutlinePlacementKind::LastChild => proj.nodes.get(target_node_idx).map(|n| n.key.clone()),
        OutlinePlacementKind::Before | OutlinePlacementKind::After => {
            proj.nodes.get(target_node_idx).and_then(|n| n.parent_idx).and_then(|pi| proj.nodes.get(pi)).map(|p| p.key.clone())
        }
    };

    Some(EvaluatedDrop { kind: DropTargetKind::Line { y, depth, target_node_idx, placement }, valid, expanded_parent_key })
}

fn ancestor_at_depth(proj: &ProjectedTree, mut node_idx: usize, target_depth: u32) -> Option<usize> {
    let mut guard = 0usize;
    while let Some(node) = proj.nodes.get(node_idx) {
        if node.depth <= target_depth || guard > proj.nodes.len() {
            return Some(node_idx);
        }
        guard = guard.saturating_add(1);
        node_idx = node.parent_idx?;
    }
    None
}

fn is_self_or_descendant(proj: &ProjectedTree, drag_roots: &[usize], candidate_idx: usize) -> bool {
    let Some(cand) = proj.nodes.get(candidate_idx) else { return true };
    drag_roots.iter().any(|&r_idx| proj.nodes.get(r_idx).is_some_and(|rn| cand.euler_in >= rn.euler_in && cand.euler_in <= rn.euler_out))
}

/// Update the 450 ms / < 4 px dwell auto-expand timer for collapsed parent rows.
pub(super) fn update_dwell_auto_expand(
    state: &mut BookmarkPanelState,
    evaluated: Option<&EvaluatedDrop>,
    proj: &ProjectedTree,
    pointer: egui::Pos2,
    now: f64,
) -> bool {
    if let Some(eval) = evaluated
        && eval.valid
        && let DropTargetKind::IntoRow { target_node_idx, .. } = eval.kind
        && let Some(node) = proj.nodes.get(target_node_idx)
        && node.has_children
        && !node.is_expanded
        && let Some(drag) = state.drag.as_mut()
    {
        match &mut drag.dwell {
            Some((key, anchor, started)) if *key == node.key => {
                if (pointer - *anchor).length() >= DWELL_MAX_MOVE_PX {
                    *anchor = pointer;
                    *started = now;
                } else if now - *started >= DWELL_EXPAND_SECS {
                    state.expanded_keys.insert(node.key.clone(), true);
                    drag.dwell = None;
                }
            }
            _ => {
                drag.dwell = Some((node.key.clone(), pointer, now));
            }
        }
        true
    } else {
        if let Some(drag) = state.drag.as_mut() {
            drag.dwell = None;
        }
        false
    }
}

/// Update dwell-to-expand timer on collapsed parent rows during center 50% hover, compute
/// proportional edge auto-scroll delta, and paint indicator + ghost pill.
pub(super) fn process_active_drag(
    ui: &mut egui::Ui,
    t: &Tokens,
    scroll_rect: Rect,
    row_rects: &[(usize, usize, Rect)],
    proj: &ProjectedTree,
    state: &mut BookmarkPanelState,
    drag_roots: &[usize],
) -> Option<EvaluatedDrop> {
    let pointer = ui.input(|i| i.pointer.hover_pos())?;
    let now = ui.input(|i| i.time);

    // Proportional edge auto-scroll when dragging inside top/bottom 36 px zones.
    if pointer.x >= scroll_rect.left() - 24.0 && pointer.x <= scroll_rect.right() + 24.0 {
        let mut dy = 0.0_f32;
        if pointer.y >= scroll_rect.top() && pointer.y < scroll_rect.top() + EDGE_SCROLL_ZONE {
            let ratio = ((scroll_rect.top() + EDGE_SCROLL_ZONE - pointer.y) / EDGE_SCROLL_ZONE).clamp(0.0, 1.0);
            dy = ratio * MAX_EDGE_SCROLL_STEP;
        } else if pointer.y <= scroll_rect.bottom() && pointer.y > scroll_rect.bottom() - EDGE_SCROLL_ZONE {
            let ratio = ((pointer.y - (scroll_rect.bottom() - EDGE_SCROLL_ZONE)) / EDGE_SCROLL_ZONE).clamp(0.0, 1.0);
            dy = -ratio * MAX_EDGE_SCROLL_STEP;
        }
        if dy.abs() > 0.05 {
            ui.scroll_with_delta(vec2(0.0, dy));
            ui.ctx().request_repaint();
        }
    }

    let evaluated = evaluate_drop_hit(pointer, scroll_rect, row_rects, proj, drag_roots);

    // 450 ms dwell auto-expand (< 4 px cursor drift) on a collapsed parent in Center 50%.
    if update_dwell_auto_expand(state, evaluated.as_ref(), proj, pointer, now) {
        ui.ctx().request_repaint_after(Duration::from_millis(50));
    }

    // Paint visual drop feedback on foreground/panel painter.
    if let Some(ref eval) = evaluated
        && eval.valid
    {
        let painter = ui.painter();
        match eval.kind {
            DropTargetKind::Line { y, depth, .. } => {
                let x_start = (scroll_rect.left() + BASE_PAD + (depth.saturating_sub(1) as f32) * INDENT_STEP).min(scroll_rect.right() - 16.0);
                let x_end = (scroll_rect.right() - 6.0).max(x_start + 8.0);
                painter.circle_filled(pos2(x_start, y), 4.0, t.accent);
                painter.line_segment([pos2(x_start, y), pos2(x_end, y)], Stroke::new(2.0, t.accent));
            }
            DropTargetKind::IntoRow { rect, .. } => {
                let highlight = t.accent.gamma_multiply(0.14);
                painter.rect_filled(rect.shrink(1.0), CornerRadius::same(4), highlight);
                painter.rect_stroke(rect.shrink(1.0), CornerRadius::same(4), Stroke::new(1.5, t.accent), egui::StrokeKind::Inside);
            }
        }
    }

    // Paint floating cursor ghost pill.
    if let Some(ref drag) = state.drag {
        paint_ghost_pill(ui.ctx(), t, pointer, drag);
    }

    evaluated
}

fn paint_ghost_pill(ctx: &egui::Context, t: &Tokens, pointer: egui::Pos2, drag: &BookmarkDragState) {
    let badge = if drag.roots.len() > 1 {
        let m = drag.roots.len();
        format!("{m} bookmarks")
    } else if drag.folded_descendants > 0 {
        let title: String = drag.lead_title.chars().take(24).collect();
        let ellipsis = if drag.lead_title.chars().count() > 24 { "…" } else { "" };
        format!("{title}{ellipsis} (+{} children)", drag.folded_descendants)
    } else {
        let title: String = drag.lead_title.chars().take(28).collect();
        let ellipsis = if drag.lead_title.chars().count() > 28 { "…" } else { "" };
        format!("{title}{ellipsis}")
    };

    let layer = egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("bookmark_drag_ghost"));
    let painter = ctx.layer_painter(layer);
    let galley = ctx.fonts_mut(|f| f.layout_no_wrap(badge, crate::theme::medium(12.0), Color32::WHITE));
    let size = galley.size() + vec2(16.0, 10.0);
    let rect = Rect::from_min_size(pointer + vec2(14.0, 14.0), size);
    painter.rect_filled(rect, CornerRadius::same(6), t.accent);
    painter.galley(rect.left_center() + vec2(8.0, -galley.size().y * 0.5), galley, Color32::WHITE);
}
