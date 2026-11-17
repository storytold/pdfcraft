//! Bookmarks sidebar: virtualized single-line projection, persistent UI expansion/selection,
//! Scroll-Spy tracking, multi-select caravan drag-and-drop, and focus-scoped shortcuts.

use std::collections::{HashMap, HashSet};

use pdfcraft_engine::{DocId, OutlinePlacementKind};
use pdfcraft_render::{DestView, LinkTarget};

use super::{CornerRadius, DocInfo, Nav, OutlineItem, Rect, Sense, Tokens, icons, pos2, theme, vec2};
use crate::canvas::DocView;
use crate::page_keys::PageKeys;

mod drag;

/// Fixed virtualized row height matching the existing panel snapshot layout metrics.
pub const ROW_HEIGHT: f32 = 28.0;

/// Stable UI key for preserving expand/collapse and selection state across edits and undo/redo
/// without dirtying COS or rewriting `/Count`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum BookmarkKey {
    Obj((u32, u16)),
    Path(Vec<usize>),
}

impl BookmarkKey {
    pub fn for_item(item: &OutlineItem, path: &[usize]) -> Self {
        item.obj.map(Self::Obj).unwrap_or_else(|| Self::Path(path.to_vec()))
    }
}

/// Active multi-root drag payload persisted on [`BookmarkPanelState`] across frames.
#[derive(Clone, Debug, Default)]
pub struct BookmarkDragState {
    pub roots: Vec<BookmarkKey>,
    pub folded_descendants: usize,
    pub lead_title: String,
    pub dwell: Option<(BookmarkKey, egui::Pos2, f64)>,
}

/// Per-document UI state for the Bookmarks panel. Never dirties COS on expand/collapse.
#[derive(Clone, Debug, Default)]
pub struct BookmarkPanelState {
    pub expanded_keys: HashMap<BookmarkKey, bool>,
    pub selected_keys: HashSet<BookmarkKey>,
    pub focused_key: Option<BookmarkKey>,
    pub drag: Option<BookmarkDragState>,
}

impl BookmarkPanelState {
    pub fn select_only(&mut self, key: BookmarkKey) {
        self.selected_keys.clear();
        self.selected_keys.insert(key.clone());
        self.focused_key = Some(key);
    }

    pub fn set_expanded(&mut self, key: BookmarkKey, expanded: bool) {
        self.expanded_keys.insert(key, expanded);
    }
}

/// What the user asked to do with bookmarks (applied after the panel is drawn).
#[derive(Clone, Debug, PartialEq)]
pub enum BmAction {
    /// New "Untitled" bookmark for the current page, after focus or in page order (`Ctrl+B` / `+`).
    New,
    StartRename(Vec<usize>),
    Rename(Vec<usize>, String),
    SetToCurrentPage(Vec<usize>),
    SetDestinationToCurrentView(Vec<usize>),
    /// Make the title bold or italic or not (`None` keeps that one).
    SetStyle(Vec<usize>, Option<bool>, Option<bool>),
    /// Colour the title; black is the default.
    SetColor(Vec<usize>, [f64; 3]),
    Delete(Vec<usize>),
    DeleteBatch(Vec<Vec<usize>>),
    MoveUp(Vec<usize>),
    MoveDown(Vec<usize>),
    /// Make it the last child of the bookmark above it.
    Indent(Vec<usize>),
    /// Move it out to follow its parent.
    Outdent(Vec<usize>),
    /// Atomic batch caravan move triggered by drag-and-drop.
    MoveBatch {
        paths: Vec<Vec<usize>>,
        target: Vec<usize>,
        placement: OutlinePlacementKind,
        expand_parent: Option<BookmarkKey>,
    },
    Copy(Vec<usize>),
    CopyBatch(Vec<Vec<usize>>),
    CopySelected,
    Cut(Vec<usize>),
    CutBatch(Vec<Vec<usize>>),
    CutSelected,
    Paste {
        parent: Vec<usize>,
        index: usize,
    },
    PasteAtFocus,
    FromStructure,
    FromTextStyles,
    OpenAutoGenerate,
    OpenRegexNest,
    OpenShiftPages(Vec<Vec<usize>>),
}

#[derive(Clone, Debug)]
pub(super) struct ProjectedBookmark {
    pub key: BookmarkKey,
    pub path: Vec<usize>,
    pub title: String,
    pub page: Option<usize>,
    pub view: DestView,
    pub action: Option<LinkTarget>,
    pub color: Option<[f32; 3]>,
    pub italic: bool,
    pub bold: bool,
    /// 1-based depth (`1` = top level).
    pub depth: u32,
    pub has_children: bool,
    pub is_expanded: bool,
    pub euler_in: u32,
    pub euler_out: u32,
    pub descendant_count: usize,
    pub parent_idx: Option<usize>,
    pub sibling_idx: usize,
    pub siblings_count: usize,
}

pub(super) struct ProjectedTree {
    pub nodes: Vec<ProjectedBookmark>,
    pub visible_rows: Vec<usize>,
    pub visible_pos_of_node: Vec<Option<usize>>,
}

impl ProjectedTree {
    /// Normalize and prune `selected_keys` into disjoint pre-order root indices in `self.nodes`.
    pub fn pruned_roots_from_keys(&self, keys: &HashSet<BookmarkKey>) -> Vec<usize> {
        let mut selected: Vec<usize> = self.nodes.iter().enumerate().filter_map(|(idx, n)| keys.contains(&n.key).then_some(idx)).collect();
        selected.sort_unstable_by_key(|&idx| self.nodes.get(idx).map_or(u32::MAX, |n| n.euler_in));
        let mut pruned = Vec::with_capacity(selected.len());
        let mut last_interval: Option<(u32, u32)> = None;
        for idx in selected {
            let Some(n) = self.nodes.get(idx) else { continue };
            if let Some((lin, lout)) = last_interval
                && n.euler_in >= lin
                && n.euler_in <= lout
            {
                continue;
            }
            pruned.push(idx);
            last_interval = Some((n.euler_in, n.euler_out));
        }
        pruned
    }

    pub fn visible_keys(&self) -> Vec<BookmarkKey> {
        self.visible_rows.iter().filter_map(|&idx| self.nodes.get(idx).map(|n| n.key.clone())).collect()
    }
}

/// Matching titles plus their ancestors, keeping document paths rather than filtered indexes.
/// `query` is folded with [`pdfcraft_render::text::fold_case`], as titles are here.
pub(super) fn outline_matches(items: &[OutlineItem], query: &str) -> HashSet<Vec<usize>> {
    let mut matches = HashSet::new();
    if query.is_empty() {
        return matches;
    }
    let mut pending: Vec<_> = items.iter().enumerate().map(|(i, item)| (vec![i], item)).collect();
    while let Some((path, item)) = pending.pop() {
        if pdfcraft_render::text::fold_case(&item.title).contains(query) {
            let mut ancestor = path.clone();
            while !ancestor.is_empty() {
                matches.insert(ancestor.clone());
                ancestor.pop();
            }
        }
        for (i, child) in item.children.iter().enumerate() {
            let mut child_path = path.clone();
            child_path.push(i);
            pending.push((child_path, child));
        }
    }
    matches
}

/// Project `outline` into an iterative flat Euler-indexed list with persistent UI expansion.
pub(super) fn project_outline(
    outline: &[OutlineItem],
    state: &mut BookmarkPanelState,
    expand_override: Option<usize>,
    matches: &HashSet<Vec<usize>>,
    searching: bool,
) -> ProjectedTree {
    struct Frame<'a> {
        items: &'a [OutlineItem],
        next_child: usize,
        parent_idx: Option<usize>,
        parent_path: Vec<usize>,
        depth: u32,
        node_idx: Option<usize>,
        ancestors_open: bool,
        folded_by_drag: bool,
    }

    let drag_roots: HashSet<BookmarkKey> = state.drag.as_ref().map(|d| d.roots.iter().cloned().collect()).unwrap_or_default();

    let mut nodes: Vec<ProjectedBookmark> = Vec::new();
    let mut visible_rows: Vec<usize> = Vec::new();
    let mut timer: u32 = 0;

    let mut stack = vec![Frame {
        items: outline,
        next_child: 0,
        parent_idx: None,
        parent_path: Vec::new(),
        depth: 1,
        node_idx: None,
        ancestors_open: true,
        folded_by_drag: false,
    }];

    while let Some(frame) = stack.last_mut() {
        if frame.next_child < frame.items.len() {
            let sibling_idx = frame.next_child;
            let siblings_count = frame.items.len();
            let Some(item) = frame.items.get(sibling_idx) else {
                frame.next_child = siblings_count;
                continue;
            };
            frame.next_child = sibling_idx.saturating_add(1);

            let mut path = frame.parent_path.clone();
            path.push(sibling_idx);
            let key = BookmarkKey::for_item(item, &path);
            let depth = frame.depth;
            let has_children = !item.children.is_empty();

            if let Some(levels) = expand_override {
                state.expanded_keys.insert(key.clone(), (depth as usize) <= levels);
            }
            let is_expanded = if has_children { *state.expanded_keys.entry(key.clone()).or_insert(item.open) } else { false };

            let row_visible = if searching { matches.contains(&path) } else { frame.ancestors_open && !frame.folded_by_drag };

            let my_idx = nodes.len();
            let euler_in = timer;
            timer = timer.saturating_add(1);

            let is_drag_root = !searching && drag_roots.contains(&key);
            let child_folded = frame.folded_by_drag || is_drag_root;
            let child_ancestors_open = frame.ancestors_open && is_expanded;

            nodes.push(ProjectedBookmark {
                key,
                path: path.clone(),
                title: item.title.clone(),
                page: item.page,
                view: item.view,
                action: item.action.clone(),
                color: item.color,
                italic: item.italic,
                bold: item.bold,
                depth,
                has_children,
                is_expanded,
                euler_in,
                euler_out: euler_in,
                descendant_count: 0,
                parent_idx: frame.parent_idx,
                sibling_idx,
                siblings_count,
            });
            if row_visible {
                visible_rows.push(my_idx);
            }

            if has_children {
                stack.push(Frame {
                    items: &item.children,
                    next_child: 0,
                    parent_idx: Some(my_idx),
                    parent_path: path,
                    depth: depth.saturating_add(1),
                    node_idx: Some(my_idx),
                    ancestors_open: child_ancestors_open,
                    folded_by_drag: child_folded,
                });
            } else if let Some(n) = nodes.get_mut(my_idx) {
                n.euler_out = timer.saturating_sub(1);
            }
        } else if let Some(done) = stack.pop()
            && let Some(node_idx) = done.node_idx
            && let Some(n) = nodes.get_mut(node_idx)
        {
            let euler_out = timer.saturating_sub(1);
            n.euler_out = euler_out;
            n.descendant_count = usize::try_from(euler_out.saturating_sub(n.euler_in)).unwrap_or(0);
        }
    }

    let mut visible_pos_of_node = vec![None; nodes.len()];
    for (vis_pos, &node_idx) in visible_rows.iter().enumerate() {
        if let Some(slot) = visible_pos_of_node.get_mut(node_idx) {
            *slot = Some(vis_pos);
        }
    }

    ProjectedTree { nodes, visible_rows, visible_pos_of_node }
}

/// Find the deepest visible node corresponding to the reader's top viewport position.
pub(super) fn active_scroll_spy_row(info: &DocInfo, view: &DocView, proj: &ProjectedTree) -> Option<usize> {
    let mut targets: Vec<(usize, f32, u32, usize)> = proj
        .nodes
        .iter()
        .enumerate()
        .filter_map(|(idx, n)| n.page.map(|p| (p, pdfcraft_engine::view_top_fraction(info, p, &n.view, view.rotation), n.euler_in, idx)))
        .collect();
    let (cur_page, cur_frac) = view.current_top_position(info);
    active_scroll_spy_row_from_targets(cur_page, cur_frac, proj, &mut targets)
}

fn active_scroll_spy_row_from_targets(
    cur_page: usize,
    cur_frac: f32,
    proj: &ProjectedTree,
    targets: &mut [(usize, f32, u32, usize)],
) -> Option<usize> {
    if targets.is_empty() {
        return None;
    }
    targets.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.total_cmp(&b.1)).then(a.2.cmp(&b.2)));
    let pos = targets.partition_point(|&(p, f, _, _)| p < cur_page || (p == cur_page && f <= cur_frac));
    let mut active = targets.get(pos.checked_sub(1)?).map(|t| t.3)?;
    let mut guard = 0usize;
    while guard <= proj.nodes.len() {
        if proj.visible_pos_of_node.get(active).copied().flatten().is_some() {
            return Some(active);
        }
        active = proj.nodes.get(active)?.parent_idx?;
        guard = guard.saturating_add(1);
    }
    None
}

pub(super) struct BookmarksPanelInput<'a> {
    pub doc_id: DocId,
    pub info: &'a DocInfo,
    pub view: &'a mut DocView,
    pub modal: bool,
    pub keys: PageKeys,
    pub editable: bool,
    pub can_paste: bool,
    pub expand_override: Option<usize>,
    pub matches: &'a HashSet<Vec<usize>>,
    pub searching: bool,
    pub rename: &'a mut Option<(Vec<usize>, String)>,
    pub nav: &'a mut Option<Nav>,
    pub action: &'a mut Option<BmAction>,
}

/// Render the virtualized, drag-and-drop Bookmarks panel inside its own `ScrollArea`.
pub(super) fn show_panel(ui: &mut egui::Ui, t: &Tokens, input: BookmarksPanelInput<'_>) {
    let BookmarksPanelInput { doc_id, info, view, modal, keys, editable, can_paste, expand_override, matches, searching, rename, nav, action } =
        input;

    if ui.ctx().egui_wants_keyboard_input() || rename.is_some() || modal {
        view.bookmarks_focused = false;
    }
    let panel_clip = ui.clip_rect();
    let owns_keyboard = view.bookmarks_focused && !ui.ctx().egui_wants_keyboard_input() && rename.is_none() && !modal;

    let proj = project_outline(&info.outline, &mut view.bookmarks_ui, expand_override, matches, searching);
    let scroll_spy_node = active_scroll_spy_row(info, view, &proj);

    // Handle Escape / Delete while focused (or pointer inside panel without modal/text input).
    let keys_active = !modal && rename.is_none() && !ui.ctx().egui_wants_keyboard_input() && (owns_keyboard || ui.rect_contains_pointer(panel_clip));
    if keys_active && keys.take(ui.ctx(), egui::Key::Escape) {
        view.bookmarks_ui.selected_keys.clear();
        view.bookmarks_ui.drag = None;
    }
    if keys_active
        && editable
        && !view.bookmarks_ui.selected_keys.is_empty()
        && (keys.take(ui.ctx(), egui::Key::Delete) || keys.take(ui.ctx(), egui::Key::Backspace))
    {
        let roots = proj.pruned_roots_from_keys(&view.bookmarks_ui.selected_keys);
        let paths: Vec<Vec<usize>> = roots.into_iter().filter_map(|idx| proj.nodes.get(idx).map(|n| n.path.clone())).collect();
        if !paths.is_empty() {
            *action = Some(BmAction::DeleteBatch(paths));
        }
    }

    let drag_root_indices: Vec<usize> = view
        .bookmarks_ui
        .drag
        .as_ref()
        .map(|d| {
            let set: HashSet<BookmarkKey> = d.roots.iter().cloned().collect();
            proj.pruned_roots_from_keys(&set)
        })
        .unwrap_or_default();

    let total_rows = proj.visible_rows.len();
    let dragging_now = view.bookmarks_ui.drag.is_some() && editable && !searching;
    let mut evaluated_drop: Option<drag::EvaluatedDrop> = None;
    let mut clicked_row: Option<(usize, usize)> = None;
    let mut toggle_expand_key: Option<(BookmarkKey, bool)> = None;
    let mut gained_focus = false;

    egui::ScrollArea::vertical().id_salt(("bookmarks_scroll", doc_id.0)).auto_shrink([false, false]).show_rows(
        ui,
        ROW_HEIGHT,
        total_rows,
        |ui, row_range| {
            if info.outline.is_empty() {
                super::right::empty(ui, t, "bookmark", "This document has no bookmarks.");
            } else if searching && matches.is_empty() {
                ui.label(tl!("No matches."));
            }

            let mut row_rects: Vec<(usize, usize, Rect)> = Vec::with_capacity(row_range.len());
            let drag_root_set: HashSet<usize> = drag_root_indices.iter().copied().collect();

            for vis_pos in row_range {
                let Some(&node_idx) = proj.visible_rows.get(vis_pos) else { continue };
                let Some(node) = proj.nodes.get(node_idx) else { continue };
                let is_dragged_root = drag_root_set.contains(&node_idx);

                let rect = draw_bookmark_row(RowDrawArgs {
                    ui,
                    t,
                    info,
                    node,
                    node_idx,
                    vis_pos,
                    view_state: &view.bookmarks_ui,
                    editable,
                    searching,
                    can_paste,
                    current_page: view.current,
                    is_scroll_spy: scroll_spy_node == Some(node_idx),
                    is_dragged_root,
                    proj: &proj,
                    rename,
                    nav,
                    action,
                    gained_focus: &mut gained_focus,
                    clicked_row: &mut clicked_row,
                    toggle_expand: &mut toggle_expand_key,
                });
                row_rects.push((vis_pos, node_idx, rect));
            }

            if info.outline_more {
                ui.label(tl!("Some bookmarks are not shown here: the panel lists a limited number, nested a limited depth."));
            }

            if dragging_now {
                evaluated_drop = drag::process_active_drag(ui, t, ui.clip_rect(), &row_rects, &proj, &mut view.bookmarks_ui, &drag_root_indices);
            }
        },
    );

    finalize_drag_start(ui, view);

    if toggle_expand_key.is_some() || clicked_row.is_some() || gained_focus {
        view.bookmarks_focused = true;
    }

    if let Some((key, next_open)) = toggle_expand_key {
        view.bookmarks_ui.set_expanded(key, next_open);
    }

    if let Some((vis_pos, node_idx)) = clicked_row {
        apply_row_selection_click(ui, &proj, &mut view.bookmarks_ui, vis_pos, node_idx);
    }

    // Complete or cancel active drag when primary button releases.
    if view.bookmarks_ui.drag.is_some() {
        let still_down = ui.input(|i| i.pointer.primary_down());
        if !still_down {
            if let Some(eval) = evaluated_drop
                && eval.valid
                && !drag_root_indices.is_empty()
            {
                let (target_idx, placement) = match eval.kind {
                    drag::DropTargetKind::Line { target_node_idx, placement, .. } => (target_node_idx, placement),
                    drag::DropTargetKind::IntoRow { target_node_idx, .. } => (target_node_idx, OutlinePlacementKind::LastChild),
                };
                if let Some(target_node) = proj.nodes.get(target_idx) {
                    let paths: Vec<Vec<usize>> = drag_root_indices.iter().filter_map(|&idx| proj.nodes.get(idx).map(|n| n.path.clone())).collect();
                    if !paths.is_empty() {
                        *action =
                            Some(BmAction::MoveBatch { paths, target: target_node.path.clone(), placement, expand_parent: eval.expanded_parent_key });
                    }
                }
            }
            view.bookmarks_ui.drag = None;
        }
    }
}

struct RowDrawArgs<'a> {
    ui: &'a mut egui::Ui,
    t: &'a Tokens,
    info: &'a DocInfo,
    node: &'a ProjectedBookmark,
    node_idx: usize,
    vis_pos: usize,
    view_state: &'a BookmarkPanelState,
    editable: bool,
    searching: bool,
    can_paste: bool,
    current_page: usize,
    is_scroll_spy: bool,
    is_dragged_root: bool,
    proj: &'a ProjectedTree,
    rename: &'a mut Option<(Vec<usize>, String)>,
    nav: &'a mut Option<Nav>,
    action: &'a mut Option<BmAction>,
    gained_focus: &'a mut bool,
    clicked_row: &'a mut Option<(usize, usize)>,
    toggle_expand: &'a mut Option<(BookmarkKey, bool)>,
}

fn draw_bookmark_row(args: RowDrawArgs<'_>) -> Rect {
    let RowDrawArgs {
        ui,
        t,
        info,
        node,
        node_idx,
        vis_pos,
        view_state,
        editable,
        searching,
        can_paste,
        current_page,
        is_scroll_spy,
        is_dragged_root,
        proj,
        rename,
        nav,
        action,
        gained_focus,
        clicked_row,
        toggle_expand,
    } = args;

    let depth_zero = node.depth.saturating_sub(1);
    let indent = depth_zero as f32 * drag::INDENT_STEP;
    let id = ui.scope_id().with(("outline", &node.path));

    if let Some((rpath, text)) = rename.as_mut()
        && rpath.as_slice() == node.path.as_slice()
    {
        let mut commit = None;
        let mut cancel = false;
        let (row_rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), Sense::hover());
        let field_rect =
            Rect::from_min_max(pos2(row_rect.left() + indent + 20.0, row_rect.top() + 2.0), pos2(row_rect.right() - 4.0, row_rect.bottom() - 2.0));
        ui.scope_builder(egui::UiBuilder::new().max_rect(field_rect), |ui| {
            crate::widgets::begin_dialog(ui);
            let resp = rename_field(ui, text, id.with("rename"));
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                cancel = true;
            } else if resp.lost_focus() {
                commit = Some(text.trim().to_string());
            }
        });
        if cancel {
            *rename = None;
        } else if let Some(title) = commit {
            *rename = None;
            if !title.is_empty() && title != node.title {
                *action = Some(BmAction::Rename(node.path.clone(), title));
            }
        }
        return row_rect;
    }

    let selected = view_state.selected_keys.contains(&node.key);
    let sense = if editable && !searching { Sense::click_and_drag() } else { Sense::click() };
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_HEIGHT), sense);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &node.title));

    if selected {
        ui.painter().rect_filled(rect, CornerRadius::same(6), t.accent.gamma_multiply(0.14));
    }
    theme::Press::track(ui, &resp).wash(ui, rect, 6, selected);

    if is_scroll_spy {
        let bar = Rect::from_min_size(pos2(rect.left() + 1.0, rect.top() + 4.0), vec2(3.0, ROW_HEIGHT - 8.0));
        ui.painter().rect_filled(bar, CornerRadius::same(1), t.accent);
    }

    let x0 = rect.left() + indent;
    let mut tri_clicked = false;
    if node.has_children {
        let tri = Rect::from_min_size(pos2(x0, rect.top() + 6.0), vec2(16.0, 16.0));
        let tint = if is_dragged_root { t.text_muted.gamma_multiply(0.45) } else { t.text_muted };
        icons::paint(ui, tri, if node.is_expanded { "chevron-down" } else { "chevron-right" }, 14.0, tint);
        if !searching && ui.interact(tri, id.with("tri"), Sense::click()).clicked() {
            tri_clicked = true;
            *toggle_expand = Some((node.key.clone(), !node.is_expanded));
        }
    }

    let unlinked = node.page.is_none() && node.action.is_none();
    let label_font = theme::regular(11.0);
    let label_galley = node.page.and_then(|p| info.pages.get(p)).map(|page| {
        ui.fonts_mut(|f| {
            let text =
                ellipsized_prefix(&page.label, |s| f.layout_no_wrap(s.to_owned(), label_font.clone(), t.text_faint).size().x <= LABEL_COLUMN_MAX);
            f.layout_no_wrap(text, label_font, t.text_faint)
        })
    });
    let label_w = label_galley.as_ref().map_or(0.0, |g| g.size().x);
    let icon_w = if unlinked { 16.0 } else { 0.0 };
    let wrap_w = (rect.width() - indent - 20.0 - label_w - icon_w - 12.0).max(24.0);

    let mut fmt = title_format(node, depth_zero as usize, t);
    if is_dragged_root {
        fmt.color = fmt.color.gamma_multiply(0.45);
    }
    let mut job = egui::text::LayoutJob::single_section(node.title.clone(), fmt);
    job.wrap = egui::text::TextWrapping::truncate_at_width(wrap_w);
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    let text_elided = galley.elided;
    let text_y = rect.top() + (ROW_HEIGHT - galley.size().y) * 0.5;
    ui.painter().galley(pos2(x0 + 20.0, text_y), galley, t.text);

    if let Some(g) = label_galley {
        let pos = pos2(rect.right() - 6.0 - g.size().x, rect.top() + (ROW_HEIGHT - g.size().y) * 0.5);
        ui.painter().galley(pos, g, t.text_faint);
    } else if unlinked {
        let icon_rect = Rect::from_min_size(pos2(rect.right() - 18.0, rect.top() + 8.0), vec2(12.0, 12.0));
        icons::paint(ui, icon_rect, "circle-dashed", 12.0, t.text_faint);
    }

    if !tri_clicked && theme::hand(resp.clone()).clicked() {
        *gained_focus = true;
        *clicked_row = Some((vis_pos, node_idx));
        let multi_mod = ui.input(|i| i.modifiers.command || i.modifiers.shift);
        if !multi_mod {
            match (node.page, &node.action) {
                (Some(p), _) => *nav = Some(Nav::Dest(p, node.view)),
                (None, Some(act)) => *nav = Some(Nav::Action(act.clone())),
                (None, None) => {}
            }
        }
    }

    if resp.double_clicked() && editable {
        *action = Some(BmAction::StartRename(node.path.clone()));
    }

    // Start multi-select or single drag on primary drag start when not searching.
    if editable && !searching && resp.drag_started_by(egui::PointerButton::Primary) {
        *gained_focus = true;
        let drag_roots_idx = if view_state.selected_keys.contains(&node.key) && view_state.selected_keys.len() > 1 {
            proj.pruned_roots_from_keys(&view_state.selected_keys)
        } else {
            vec![node_idx]
        };
        let roots: Vec<BookmarkKey> = drag_roots_idx.iter().filter_map(|&idx| proj.nodes.get(idx).map(|n| n.key.clone())).collect();
        let folded_descendants: usize = drag_roots_idx.iter().filter_map(|&idx| proj.nodes.get(idx).map(|n| n.descendant_count)).sum();
        if !roots.is_empty() {
            *clicked_row = None;
            ui.data_mut(|d| {
                d.insert_temp(
                    egui::Id::unique("bookmark_drag_start_request"),
                    BookmarkDragState { roots, folded_descendants, lead_title: node.title.clone(), dwell: None },
                )
            });
        }
    }

    let resp = if text_elided { resp.on_hover_text(&node.title) } else { resp };

    if editable {
        draw_context_menu(&resp, node, view_state, proj, can_paste, current_page, action, gained_focus);
    }

    rect
}

#[allow(clippy::too_many_arguments)]
fn draw_context_menu(
    resp: &egui::Response,
    node: &ProjectedBookmark,
    view_state: &BookmarkPanelState,
    proj: &ProjectedTree,
    can_paste: bool,
    current_page: usize,
    action: &mut Option<BmAction>,
    gained_focus: &mut bool,
) {
    let mut selected_paths: Vec<Vec<usize>> = if view_state.selected_keys.contains(&node.key) && view_state.selected_keys.len() > 1 {
        proj.pruned_roots_from_keys(&view_state.selected_keys).into_iter().filter_map(|idx| proj.nodes.get(idx).map(|n| n.path.clone())).collect()
    } else {
        vec![node.path.clone()]
    };
    if selected_paths.is_empty() {
        selected_paths.push(node.path.clone());
    }
    let multi = selected_paths.len() > 1;
    let depth = node.depth.saturating_sub(1) as usize;
    let i = node.sibling_idx;
    let siblings = node.siblings_count;

    if resp.secondary_clicked() {
        *gained_focus = true;
    }

    resp.context_menu(|ui| {
        let mut pick = |ui: &mut egui::Ui, label: &str, enabled: bool, a: BmAction| {
            if ui.add_enabled(enabled, egui::Button::new(tl!(label))).clicked() {
                *action = Some(a);
                ui.close();
            }
        };
        pick(ui, tl!("Rename"), !multi, BmAction::StartRename(node.path.clone()));
        let set_page = crate::i18n::fmt(tl!("Set to current page ({n})"), &[("n", &(current_page + 1).to_string())]);
        pick(ui, &set_page, !multi, BmAction::SetToCurrentPage(node.path.clone()));
        pick(ui, tl!("Set destination to current view"), !multi, BmAction::SetDestinationToCurrentView(node.path.clone()));

        let mut styled = None;
        ui.menu_button(tl!("Style"), |ui| {
            let (mut bold, mut italic) = (node.bold, node.italic);
            if ui.add(crate::widgets::checkbox(&mut bold, tl!("Bold"))).clicked() {
                styled = Some(BmAction::SetStyle(node.path.clone(), Some(bold), None));
            }
            if ui.add(crate::widgets::checkbox(&mut italic, tl!("Italic"))).clicked() {
                styled = Some(BmAction::SetStyle(node.path.clone(), None, Some(italic)));
            }
        });
        ui.menu_button(tl!("Colour"), |ui| {
            let current = node.color.map_or([0.0; 3], |c| c.map(f64::from));
            if let Some(c) = crate::comments::swatch_grid(ui, Some(current)) {
                styled = Some(BmAction::SetColor(node.path.clone(), c));
            }
        });
        if styled.is_some() {
            ui.close();
        }

        ui.separator();
        pick(ui, tl!("Move up"), !multi && i > 0, BmAction::MoveUp(node.path.clone()));
        pick(ui, tl!("Move down"), !multi && i + 1 < siblings, BmAction::MoveDown(node.path.clone()));
        pick(ui, tl!("Indent"), !multi && i > 0, BmAction::Indent(node.path.clone()));
        pick(ui, tl!("Outdent"), !multi && depth > 0, BmAction::Outdent(node.path.clone()));
        ui.separator();
        match selected_paths.as_slice() {
            [single] => {
                pick(ui, tl!("Cut"), true, BmAction::Cut(single.clone()));
                pick(ui, tl!("Copy"), true, BmAction::Copy(single.clone()));
            }
            _ => {
                pick(ui, tl!("Cut"), true, BmAction::CutBatch(selected_paths.clone()));
                pick(ui, tl!("Copy"), true, BmAction::CopyBatch(selected_paths.clone()));
            }
        }
        let parent = node.path.get(..depth).unwrap_or_default().to_vec();
        pick(ui, tl!("Paste"), can_paste, BmAction::Paste { parent, index: i.saturating_add(1) });
        ui.separator();
        pick(ui, tl!("Shift page numbers…"), true, BmAction::OpenShiftPages(selected_paths.clone()));
        match selected_paths.as_slice() {
            [single] => pick(ui, tl!("Delete"), true, BmAction::Delete(single.clone())),
            _ => pick(ui, tl!("Delete"), true, BmAction::DeleteBatch(selected_paths)),
        }
        if let Some(a) = styled {
            *action = Some(a);
        }
    });
}

fn apply_row_selection_click(ui: &egui::Ui, proj: &ProjectedTree, state: &mut BookmarkPanelState, vis_pos: usize, node_idx: usize) {
    let Some(node) = proj.nodes.get(node_idx) else { return };
    let (cmd, shift) = ui.input(|i| (i.modifiers.command, i.modifiers.shift));
    if shift
        && let Some(anchor_key) = state.focused_key.as_ref()
        && let Some(anchor_idx) = proj.nodes.iter().position(|n| &n.key == anchor_key)
        && let Some(anchor_vis) = proj.visible_pos_of_node.get(anchor_idx).copied().flatten()
    {
        let (lo, hi) = if anchor_vis <= vis_pos { (anchor_vis, vis_pos) } else { (vis_pos, anchor_vis) };
        state.selected_keys.clear();
        for vp in lo..=hi {
            if let Some(&ni) = proj.visible_rows.get(vp)
                && let Some(n) = proj.nodes.get(ni)
            {
                state.selected_keys.insert(n.key.clone());
            }
        }
    } else if cmd {
        if !state.selected_keys.remove(&node.key) {
            state.selected_keys.insert(node.key.clone());
        }
        state.focused_key = Some(node.key.clone());
    } else {
        state.select_only(node.key.clone());
    }
}

/// Apply any drag-start queued during row drawing onto `view.bookmarks_ui`.
pub(super) fn finalize_drag_start(ui: &egui::Ui, view: &mut DocView) {
    if let Some(drag_start) = ui.data_mut(|d| d.remove_temp::<BookmarkDragState>(egui::Id::unique("bookmark_drag_start_request"))) {
        if drag_start.roots.len() == 1
            && let Some(first) = drag_start.roots.first()
            && !view.bookmarks_ui.selected_keys.contains(first)
        {
            view.bookmarks_ui.select_only(first.clone());
        }
        view.bookmarks_ui.drag = Some(drag_start);
    }
}

/// The destination page label gets its own right-aligned column in a bookmark row; drawing
/// tools put whole section titles in /PageLabels, so the column is bounded (#124).
const LABEL_COLUMN_MAX: f32 = 72.0;

/// More characters than ever fit [`LABEL_COLUMN_MAX`] at the label size.
const LABEL_MEASURE_CHARS: usize = 64;

/// The longest head of `text` that `fits`, plus an ellipsis when the whole string doesn't.
pub(crate) fn ellipsized_prefix(text: &str, mut fits: impl FnMut(&str) -> bool) -> String {
    let mut s: String = text.chars().take(LABEL_MEASURE_CHARS).collect();
    if s.len() == text.len() && fits(text) {
        return text.to_owned();
    }
    loop {
        s.pop();
        let candidate = format!("{s}\u{2026}");
        if s.is_empty() || fits(&candidate) {
            return candidate;
        }
    }
}

/// The inline rename field (#5529). On the frame it takes focus it selects the whole title.
fn rename_field(ui: &mut egui::Ui, text: &mut String, id: egui::Id) -> egui::Response {
    let first_frame = !ui.memory(|m| m.has_focus(id));
    let resp = ui.add(crate::widgets::line(text).desired_width(ui.available_width() - 8.0).id(id));
    if !resp.has_focus() && !resp.lost_focus() {
        if first_frame {
            let mut state = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
            let all = egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(text.chars().count()));
            state.cursor.set_char_range(Some(all));
            state.store(ui.ctx(), id);
        }
        resp.request_focus();
    }
    resp
}

/// How a bookmark's title is drawn: in its own style and colour (ISO 32000-2 §12.3.3, Tables 151
/// and 152), bold in the heavier face and italic slanted, its colour kept readable on the panel.
/// Top-level bookmarks without a style keep the medium face.
fn title_format(item: &ProjectedBookmark, depth: usize, t: &Tokens) -> egui::TextFormat {
    let font_id = match (item.bold, depth) {
        (true, _) => theme::semibold(13.0),
        (false, 0) => theme::medium(13.0),
        (false, _) => theme::regular(13.0),
    };
    let color = item.color.map_or(t.text, |rgb| {
        let [r, g, b] = rgb.map(|c| (c * 255.0).round() as u8);
        theme::readable(egui::Color32::from_rgb(r, g, b), t.panel, t.text)
    });
    egui::TextFormat { font_id, color, italics: item.italic, ..Default::default() }
}

/// Helper for `app/keys.rs` `Ctrl+A` in Bookmarks panel: returns all visible keys when searching or not.
pub(crate) fn visible_bookmark_keys(outline: &[OutlineItem], state: &mut BookmarkPanelState, query: &str) -> Vec<BookmarkKey> {
    let matches = outline_matches(outline, query);
    project_outline(outline, state, None, &matches, !query.is_empty()).visible_keys()
}

/// Helper to resolve pruned selected paths from `BookmarkPanelState`.
pub(crate) fn selected_pruned_paths(outline: &[OutlineItem], state: &mut BookmarkPanelState) -> Vec<Vec<usize>> {
    let proj = project_outline(outline, state, None, &HashSet::new(), false);
    let roots = proj.pruned_roots_from_keys(&state.selected_keys);
    roots.into_iter().filter_map(|idx| proj.nodes.get(idx).map(|n| n.path.clone())).collect()
}

/// Helper to resolve focused bookmark's path in `outline`.
pub(crate) fn focused_path(outline: &[OutlineItem], state: &mut BookmarkPanelState) -> Option<Vec<usize>> {
    let key = state.focused_key.as_ref()?.clone();
    let proj = project_outline(outline, state, None, &HashSet::new(), false);
    proj.nodes.into_iter().find(|n| n.key == key).map(|n| n.path)
}

#[cfg(test)]
mod label_column {
    use super::ellipsized_prefix;

    #[test]
    fn short_labels_are_not_touched() {
        for label in ["3", "iv", "1-1", "A-201"] {
            assert_eq!(ellipsized_prefix(label, |s| s.len() <= 10), label);
        }
    }

    #[test]
    fn long_labels_are_cut_to_their_column_with_an_ellipsis() {
        let label = "Floor Plans (001 Floor Plans): GROUND FLOOR PLAN";
        let shown = ellipsized_prefix(label, |s| s.len() <= 20);
        assert!(shown.ends_with('\u{2026}'), "{shown:?}");
        assert!(shown.len() <= 20, "{shown:?}");
        assert!(label.starts_with(shown.trim_end_matches('\u{2026}')), "{shown:?}");
    }

    #[test]
    fn multibyte_labels_are_cut_on_char_boundaries() {
        let label = "図面（一階平面図）：配置図";
        let shown = ellipsized_prefix(label, |s| s.len() <= 12);
        assert!(shown.ends_with('\u{2026}'), "{shown:?}");
        assert!(shown.len() <= 12, "{shown:?}");
        assert!(label.starts_with(shown.trim_end_matches('\u{2026}')), "{shown:?}");
    }

    #[test]
    fn the_loop_ends_when_nothing_fits() {
        assert_eq!(ellipsized_prefix("Floor Plans", |_| false), "\u{2026}");
    }

    #[test]
    fn a_huge_label_is_measured_a_bounded_number_of_times() {
        let label = "x".repeat(1_000_000);
        let mut calls = 0;
        let shown = ellipsized_prefix(&label, |s| {
            calls += 1;
            s.len() <= 12
        });
        assert!(calls <= super::LABEL_MEASURE_CHARS + 1, "{calls} measurements");
        assert!(shown.ends_with('\u{2026}') && label.starts_with(shown.trim_end_matches('\u{2026}')), "{shown:?}");
    }
}

#[cfg(test)]
mod bookmark_filter {
    use super::drag::{DropTargetKind, evaluate_drop_hit};
    use super::{BookmarkKey, BookmarkPanelState, ProjectedBookmark, ROW_HEIGHT, outline_matches, project_outline, title_format};
    use crate::theme::{self, Tokens};
    use egui::{Rect, pos2, vec2};
    use pdfcraft_engine::OutlinePlacementKind;
    use pdfcraft_render::OutlineItem;

    fn proj_node(color: Option<[f32; 3]>, italic: bool, bold: bool) -> ProjectedBookmark {
        ProjectedBookmark {
            key: BookmarkKey::Path(vec![0]),
            path: vec![0],
            title: "Styled".into(),
            page: Some(0),
            view: pdfcraft_render::DestView::Top,
            action: None,
            color,
            italic,
            bold,
            depth: 1,
            has_children: false,
            is_expanded: false,
            euler_in: 0,
            euler_out: 0,
            descendant_count: 0,
            parent_idx: None,
            sibling_idx: 0,
            siblings_count: 1,
        }
    }

    #[test]
    fn bookmark_titles_take_their_style_and_colour() {
        let t = Tokens::for_kind(crate::ThemeKind::Light);
        let plain = proj_node(None, false, false);
        assert_eq!(title_format(&plain, 0, &t).font_id, theme::medium(13.0));
        assert_eq!(title_format(&plain, 1, &t).font_id, theme::regular(13.0));
        assert_eq!(title_format(&plain, 1, &t).color, t.text);
        assert!(!title_format(&plain, 1, &t).italics);
        let styled = proj_node(Some([1.0, 0.0, 0.0]), true, true);
        let f = title_format(&styled, 1, &t);
        assert_eq!(f.font_id, theme::semibold(13.0));
        assert!(f.italics);
        assert_eq!(f.color, egui::Color32::from_rgb(255, 0, 0));
        let half = proj_node(Some([0.0, 0.5, 0.0]), false, false);
        assert_eq!(title_format(&half, 1, &t).color, egui::Color32::from_rgb(0, 128, 0));
    }

    fn item(title: &str, children: Vec<OutlineItem>) -> OutlineItem {
        OutlineItem {
            obj: None,
            title: title.into(),
            page: Some(0),
            view: pdfcraft_render::DestView::Top,
            action: None,
            children,
            open: true,
            color: None,
            italic: false,
            bold: false,
        }
    }

    #[test]
    fn titles_match_in_any_case_with_turkish_i() {
        let outline = [item("İSTANBUL ŞUBESİ", vec![item("Kış dönemi", vec![])]), item("Ankara", vec![])];
        let fold = pdfcraft_render::text::fold_case;
        assert_eq!(outline_matches(&outline, &fold("istanbul")).len(), 1);
        assert_eq!(outline_matches(&outline, &fold("İSTANBUL")).len(), 1);
        assert_eq!(outline_matches(&outline, &fold("şubesi")).len(), 1);
        assert_eq!(outline_matches(&outline, &fold("KIŞ")).len(), 2);
        assert_eq!(outline_matches(&outline, &fold("ankara")).len(), 1);
    }

    #[test]
    fn drop_hit_test_handles_top_center_bottom_and_x_axis_outdent() {
        let outline = [item("Ch1", vec![item("Sec1", vec![item("Sub1", vec![])])]), item("Ch2", vec![])];
        let mut state = BookmarkPanelState::default();
        let proj = project_outline(&outline, &mut state, None, &std::collections::HashSet::new(), false);
        let scroll = Rect::from_min_size(pos2(0.0, 0.0), vec2(260.0, 400.0));
        let row_rects: Vec<_> = proj
            .visible_rows
            .iter()
            .enumerate()
            .map(|(vp, &ni)| (vp, ni, Rect::from_min_size(pos2(0.0, vp as f32 * ROW_HEIGHT), vec2(260.0, ROW_HEIGHT))))
            .collect();

        // Top 25% of first visible row (row 0) -> Before(Ch1) at depth 1
        let top0 = evaluate_drop_hit(pos2(40.0, 2.0), scroll, &row_rects, &proj, &[3]).unwrap();
        assert!(top0.valid);
        assert_eq!(top0.kind, DropTargetKind::Line { y: 0.0, depth: 1, target_node_idx: 0, placement: OutlinePlacementKind::Before });

        // Center 50% of Ch2 (row 3, y = 84..112) -> IntoRow(Ch2)
        let center_ch2 = evaluate_drop_hit(pos2(40.0, 98.0), scroll, &row_rects, &proj, &[0]).unwrap();
        assert!(center_ch2.valid);
        assert!(matches!(center_ch2.kind, DropTargetKind::IntoRow { target_node_idx: 3, .. }));

        // Dropping Ch1 onto its own child Sub1 is invalid
        let self_drop = evaluate_drop_hit(pos2(40.0, 2.0 * ROW_HEIGHT + 14.0), scroll, &row_rects, &proj, &[0]).unwrap();
        assert!(!self_drop.valid);

        // Bottom band / blank space below last row (Ch2 depth 1 -> Sub1 after moving? test gap after Sub1 vs Ch2!)
        let blank_below = evaluate_drop_hit(pos2(40.0, 150.0), scroll, &row_rects, &proj, &[0]).unwrap();
        assert!(blank_below.valid);
        assert_eq!(
            blank_below.kind,
            DropTargetKind::Line { y: 4.0 * ROW_HEIGHT, depth: 2, target_node_idx: 3, placement: OutlinePlacementKind::LastChild }
        );
        // Far left in blank space -> Level 1 After(Ch2)
        let blank_left = evaluate_drop_hit(pos2(6.0, 150.0), scroll, &row_rects, &proj, &[0]).unwrap();
        assert_eq!(
            blank_left.kind,
            DropTargetKind::Line { y: 4.0 * ROW_HEIGHT, depth: 1, target_node_idx: 3, placement: OutlinePlacementKind::After }
        );
    }

    #[test]
    fn scroll_spy_highlights_collapsed_ancestor_and_resolves_ties() {
        let mut p1 = item("Part 1", vec![item("Sub A", vec![]), item("Sub B", vec![])]);
        p1.open = false;
        let outline = [p1, item("Appendix", vec![])];
        let mut state = BookmarkPanelState::default();
        let proj = project_outline(&outline, &mut state, None, &std::collections::HashSet::new(), false);
        // Collapsed: visible_rows has Part 1 (idx 0) and Appendix (idx 3).
        assert_eq!(proj.visible_rows, vec![0, 3]);
        // Suppose Sub A is at page 1 frac 0.0, Sub B is at page 2 frac 0.5, Appendix is non-monotonic at page 0.
        let mut targets = vec![
            (0, 0.0, proj.nodes[0].euler_in, 0),
            (1, 0.0, proj.nodes[1].euler_in, 1),
            (2, 0.5, proj.nodes[2].euler_in, 2),
            (0, 0.0, proj.nodes[3].euler_in, 3),
        ];
        // Reader at page 2 frac 0.6 -> Sub B (idx 2) -> collapsed parent Part 1 (idx 0).
        assert_eq!(super::active_scroll_spy_row_from_targets(2, 0.6, &proj, &mut targets), Some(0));
        // Reader at page 0 frac 0.1 -> tie at (0, 0.0) picks Appendix (later euler_in, idx 3).
        assert_eq!(super::active_scroll_spy_row_from_targets(0, 0.1, &proj, &mut targets), Some(3));
    }

    #[test]
    fn dwell_timer_auto_expands_after_threshold_when_still() {
        let mut ch1 = item("Ch1", vec![item("Sec1", vec![])]);
        ch1.open = false;
        let outline = [ch1, item("Ch2", vec![])];
        let mut state = BookmarkPanelState {
            drag: Some(super::BookmarkDragState {
                roots: vec![BookmarkKey::Path(vec![1])],
                folded_descendants: 0,
                lead_title: "Ch2".into(),
                dwell: None,
            }),
            ..Default::default()
        };
        let proj = project_outline(&outline, &mut state, None, &std::collections::HashSet::new(), false);
        let scroll = Rect::from_min_size(pos2(0.0, 0.0), vec2(260.0, 400.0));
        let row_rects = vec![
            (0, 0, Rect::from_min_size(pos2(0.0, 0.0), vec2(260.0, ROW_HEIGHT))),
            (1, 1, Rect::from_min_size(pos2(0.0, ROW_HEIGHT), vec2(260.0, ROW_HEIGHT))),
        ];
        let p0 = pos2(60.0, 14.0);
        let eval = evaluate_drop_hit(p0, scroll, &row_rects, &proj, &[2]).unwrap();
        super::drag::update_dwell_auto_expand(&mut state, Some(&eval), &proj, p0, 10.0);
        super::drag::update_dwell_auto_expand(&mut state, Some(&eval), &proj, pos2(62.0, 14.0), 10.2);
        assert_ne!(state.expanded_keys.get(&proj.nodes[0].key), Some(&true));
        // >= 4px cursor move resets dwell timer.
        super::drag::update_dwell_auto_expand(&mut state, Some(&eval), &proj, pos2(70.0, 14.0), 10.4);
        super::drag::update_dwell_auto_expand(&mut state, Some(&eval), &proj, pos2(70.0, 14.0), 10.6);
        assert_ne!(state.expanded_keys.get(&proj.nodes[0].key), Some(&true));
        // After 0.45s without moving >= 4px, collapsed parent expands.
        super::drag::update_dwell_auto_expand(&mut state, Some(&eval), &proj, pos2(71.0, 14.0), 10.9);
        assert_eq!(state.expanded_keys.get(&proj.nodes[0].key), Some(&true));
    }
}
