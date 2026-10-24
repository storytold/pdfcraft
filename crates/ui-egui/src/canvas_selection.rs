//! Canvas selection and zoom-aware navigation engine for PDF Craft (L7).
//!
//! Provides granular bounding box detection, click-to-select mechanics for text lines
//! and page elements, selection handles, Delete-key removal, and zoom-aware navigation.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use egui::{Color32, CornerRadius, CursorIcon, Key, Pos2, Rect, Stroke, vec2};
use pdfcraft_engine::{Edit, ImageEdit, LineSegment};
use pdfcraft_render::DocInfo;

use crate::canvas::{DocView, Fit, PageXform};

/// Primary selection accent color (Acrobat-style blue).
pub const ACCENT_BLUE: Color32 = Color32::from_rgb(0x14, 0x73, 0xE6);

/// Hover accent (amber) — shown when the pointer is over an element but it
/// is not selected. ACCENT_BLUE is used for the selected state.
pub const ACCENT_YELLOW: Color32 = Color32::from_rgb(0xF5, 0xA6, 0x23);

/// The kind of selectable canvas element.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectableKind {
    /// A single line of text on a page.
    TextLine,
    /// A grouped paragraph / text block.
    TextBlock,
    /// An embedded page image.
    Image,
    /// Added vector/text content stream.
    AddedContent,
    /// Stroked vector line segment.
    VectorLine,
}

/// An identified element on a page with its bounding box in PDF user space.
#[derive(Clone, Debug, PartialEq)]
pub struct SelectableElement {
    /// 0-based page index.
    pub page: usize,
    /// The element type.
    pub kind: SelectableKind,
    /// Index in the corresponding page inventory (line, block, image, or added content index).
    pub index: usize,
    /// Bounding rectangle in PDF user space: `[min_x, min_y, max_x, max_y]`.
    pub user_rect: [f64; 4],
    /// Extracted or summary text if applicable.
    pub text: Option<String>,
    /// Source baseline coordinates `[x, y]` if a text element.
    pub baseline: Option<[f64; 2]>,
    /// If `kind == VectorLine`, the line geometry (None otherwise).
    pub line: Option<LineSegment>,
}

impl SelectableElement {
    /// Construct a selectable text line element.
    pub fn new_line(page: usize, index: usize, user_rect: [f64; 4], text: String, baseline: Option<[f64; 2]>) -> Self {
        Self { page, kind: SelectableKind::TextLine, index, user_rect, text: Some(text), baseline, line: None }
    }

    /// Construct a selectable text block element.
    pub fn new_block(page: usize, index: usize, user_rect: [f64; 4], text: String) -> Self {
        Self { page, kind: SelectableKind::TextBlock, index, user_rect, text: Some(text), baseline: None, line: None }
    }

    /// Construct a selectable image element.
    pub fn new_image(page: usize, index: usize, user_rect: [f64; 4]) -> Self {
        Self { page, kind: SelectableKind::Image, index, user_rect, text: None, baseline: None, line: None }
    }

    /// Construct a selectable vector-line element.
    pub fn new_line_vector(page: usize, line: LineSegment) -> Self {
        let bbox = line.bbox();
        Self { page, kind: SelectableKind::VectorLine, index: line.index, user_rect: bbox, text: None, baseline: None, line: Some(line) }
    }

    /// Transform the user-space bounding box to screen coordinates.
    pub fn screen_rect(&self, xf: &PageXform, info: &DocInfo) -> Rect {
        xf.user_rect(info, self.page, self.user_rect.map(|v| v as f32))
    }

    /// Check if a screen position hits this element's screen rectangle (with optional padding).
    pub fn contains_screen_pos(&self, pos: Pos2, xf: &PageXform, info: &DocInfo, padding: f32) -> bool {
        let r = self.screen_rect(xf, info).expand(padding);
        r.contains(pos)
    }
}

/// A resize, move, or selection handle on an element bounding box.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionHandle {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    Top,
    Right,
    Bottom,
    Left,
    Move,
    LineStart,
    LineEnd,
}

impl SelectionHandle {
    /// Cursor icon appropriate for this handle.
    pub fn cursor_icon(self) -> CursorIcon {
        match self {
            Self::TopLeft | Self::BottomRight => CursorIcon::ResizeNwSe,
            Self::TopRight | Self::BottomLeft => CursorIcon::ResizeNeSw,
            Self::Top | Self::Bottom => CursorIcon::ResizeVertical,
            Self::Left | Self::Right => CursorIcon::ResizeHorizontal,
            Self::Move => CursorIcon::Move,
            Self::LineStart | Self::LineEnd => CursorIcon::Crosshair,
        }
    }

    /// Calculate the screen rectangles for the eight boundary handles around `bounds`.
    pub fn handle_rects(bounds: Rect, size: f32) -> [(Self, Rect); 8] {
        let c = |pos: Pos2| Rect::from_center_size(pos, vec2(size, size));
        [
            (Self::TopLeft, c(bounds.left_top())),
            (Self::TopRight, c(bounds.right_top())),
            (Self::BottomLeft, c(bounds.left_bottom())),
            (Self::BottomRight, c(bounds.right_bottom())),
            (Self::Top, c(Pos2::new(bounds.center().x, bounds.top()))),
            (Self::Right, c(Pos2::new(bounds.right(), bounds.center().y))),
            (Self::Bottom, c(Pos2::new(bounds.center().x, bounds.bottom()))),
            (Self::Left, c(Pos2::new(bounds.left(), bounds.center().y))),
        ]
    }

    /// Hit-test against boundary handles for `bounds`.
    pub fn hit_test(bounds: Rect, pos: Pos2, handle_size: f32) -> Option<Self> {
        let reach = (handle_size * 0.5).max(6.0);
        for (handle, rect) in Self::handle_rects(bounds, handle_size) {
            if rect.expand(reach).contains(pos) {
                return Some(handle);
            }
        }
        bounds.contains(pos).then_some(Self::Move)
    }

    /// Calculate the resized bounding rectangle given a handle drag delta.
    pub fn resize_rect(self, rect: Rect, delta: egui::Vec2) -> Rect {
        let mut min = rect.min;
        let mut max = rect.max;
        match self {
            Self::TopLeft => {
                min.x += delta.x;
                min.y += delta.y;
            }
            Self::TopRight => {
                max.x += delta.x;
                min.y += delta.y;
            }
            Self::BottomLeft => {
                min.x += delta.x;
                max.y += delta.y;
            }
            Self::BottomRight => {
                max.x += delta.x;
                max.y += delta.y;
            }
            Self::Top => {
                min.y += delta.y;
            }
            Self::Bottom => {
                max.y += delta.y;
            }
            Self::Left => {
                min.x += delta.x;
            }
            Self::Right => {
                max.x += delta.x;
            }
            Self::Move => {
                min += delta;
                max += delta;
            }
            Self::LineStart | Self::LineEnd => {}
        }
        // Enforce minimum dimensions of 10 points and prevent rectangle inversion
        const MIN_SIZE: f32 = 10.0;
        if min.x > max.x - MIN_SIZE {
            if matches!(self, Self::TopLeft | Self::BottomLeft | Self::Left) {
                min.x = max.x - MIN_SIZE;
            } else {
                max.x = min.x + MIN_SIZE;
            }
        }
        if min.y > max.y - MIN_SIZE {
            if matches!(self, Self::TopLeft | Self::TopRight | Self::Top) {
                min.y = max.y - MIN_SIZE;
            } else {
                max.y = min.y + MIN_SIZE;
            }
        }
        Rect::from_min_max(min, max)
    }
}

/// Captured text formatting for the Format Painter.
#[derive(Clone, Debug, PartialEq)]
pub struct CapturedFormat {
    pub font_family: Option<String>,
    pub font_size: f32,
    pub is_bold: bool,
    pub is_italic: bool,
    pub is_strikethrough: bool,
    pub is_superscript: bool,
    pub is_subscript: bool,
    pub color: Color32,
    pub line_spacing: f64,
    pub char_spacing: f64,
    pub scale: f64,
}

/// The state of the canvas selection engine.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SelectionState {
    /// Actively selected element.
    pub selected: Option<SelectableElement>,
    /// Currently hovered element (outline preview).
    pub hovered: Option<SelectableElement>,
    /// Additional multi-selected elements if shift-selected.
    pub multi_selected: Vec<SelectableElement>,
    /// Handle currently being hovered or manipulated.
    pub active_handle: Option<SelectionHandle>,
    /// Screen position where a drag began.
    pub drag_start: Option<Pos2>,
    /// User rectangle when handle drag began.
    pub drag_start_rect: Option<[f64; 4]>,
    /// Format painter clipboard holding captured formatting.
    pub format_painter: Option<CapturedFormat>,
    /// Whether the symbol palette modal is open.
    pub symbol_palette_open: bool,
}

impl SelectionState {
    /// Clear all selections and active interaction states.
    pub fn clear(&mut self) {
        self.selected = None;
        self.hovered = None;
        self.multi_selected.clear();
        self.active_handle = None;
        self.drag_start = None;
        self.drag_start_rect = None;
        self.symbol_palette_open = false;
    }

    /// Check if any element is selected.
    pub fn has_selection(&self) -> bool {
        self.selected.is_some() || !self.multi_selected.is_empty()
    }

    /// Select or toggle an element with optional additive selection.
    pub fn select_element(&mut self, element: SelectableElement, additive: bool) {
        if additive {
            if let Some(pos) = self.multi_selected.iter().position(|e| e.page == element.page && e.kind == element.kind && e.index == element.index) {
                self.multi_selected.remove(pos);
            } else {
                self.multi_selected.push(element);
            }
        } else {
            self.selected = Some(element);
            self.multi_selected.clear();
        }
    }
}

/// Core selection and layout engine for canvas elements.
pub struct SelectionLayoutEngine;

impl SelectionLayoutEngine {
    /// Hit-test a screen point against a list of candidate elements.
    pub fn hit_test_element(pos: Pos2, xf: &PageXform, info: &DocInfo, elements: &[SelectableElement], padding: f32) -> Option<SelectableElement> {
        // Search in reverse order so topmost elements are hit first.
        elements.iter().rev().find(|el| el.contains_screen_pos(pos, xf, info, padding)).cloned()
    }

    /// Handle keyboard deletion (`Delete` or `Backspace`) for the currently selected element.
    ///
    /// Returns the engine `Edit` command to execute, clearing the selection on dispatch.
    pub fn handle_delete_key(state: &mut SelectionState) -> Option<Edit> {
        let el = state.selected.take()?;
        match el.kind {
            SelectableKind::TextLine => {
                // Replacing a line's text with an empty string removes its glyphs while preserving layout.
                Some(Edit::EditTextLine { page: el.page, line: el.index, text: String::new() })
            }
            SelectableKind::TextBlock => Some(Edit::EditTextBlock { page: el.page, block: el.index, text: String::new(), style: Default::default() }),
            SelectableKind::Image => Some(Edit::EditPageImage { page: el.page, index: el.index, change: ImageEdit::Delete }),
            SelectableKind::AddedContent => Some(Edit::DeleteContent { page: el.page, index: el.index }),
            SelectableKind::VectorLine => None,
        }
    }

    /// Render visual bounding box and selection handles for selected and hovered elements,
    /// wiring interactive resize dragging across all eight handles.
    pub fn paint_selection_overlay(
        ui: &egui::Ui,
        painter: &egui::Painter,
        page: usize,
        xf: &PageXform,
        info: &DocInfo,
        state: &mut SelectionState,
        mut line_editor: Option<&mut crate::edit_text_ui::LineEditor>,
    ) -> Option<Edit> {
        if page >= info.pages.len() {
            return None;
        }
        // Paint subtle hover outline if hovered element is distinct from selected.
        if let Some(hovered) = &state.hovered {
            let is_selected = state.selected.as_ref().is_some_and(|s| s.page == hovered.page && s.kind == hovered.kind && s.index == hovered.index);
            if !is_selected && hovered.page == page {
                let rect = hovered.screen_rect(xf, info).expand(2.0);
                painter.rect_stroke(rect, CornerRadius::same(2), Stroke::new(1.0, ACCENT_BLUE.gamma_multiply(0.4)), egui::StrokeKind::Outside);
            }
        }

        // Paint multi-selected elements if any.
        for el in state.multi_selected.iter().filter(|el| el.page == page) {
            let rect = el.screen_rect(xf, info).expand(2.0);
            painter.rect_stroke(rect, CornerRadius::same(2), Stroke::new(1.25, ACCENT_BLUE.gamma_multiply(0.7)), egui::StrokeKind::Outside);
        }

        let mut pending_edit = None;

        // Paint primary selected element bounding box with 8 handles and wire dragging interactions.
        if let Some(sel) = &mut state.selected {
            if sel.page != page || sel.kind == SelectableKind::VectorLine {
                return None;
            }
            let unpadded_rect = sel.screen_rect(xf, info);
            let rect = unpadded_rect.expand(2.0);
            let handle_size = 7.0;
            let mut handle_dragged = None;
            let mut drag_stopped = false;

            for (handle_idx, (handle, h_rect)) in SelectionHandle::handle_rects(rect, handle_size).into_iter().enumerate() {
                let handle_id = ui.id().with(("handle", sel.index, handle_idx));
                let resp = ui.interact(h_rect.expand(2.5), handle_id, egui::Sense::drag());

                if resp.hovered() || resp.dragged() {
                    ui.ctx().set_cursor_icon(handle.cursor_icon());
                }

                if resp.drag_started() {
                    state.drag_start_rect = Some(sel.user_rect);
                }

                if resp.dragged() {
                    let delta = resp.drag_delta();
                    if delta != egui::Vec2::ZERO {
                        handle_dragged = Some((handle, delta));
                    }
                }

                if resp.drag_stopped() {
                    drag_stopped = true;
                }
            }

            let active_rect = if let Some((handle, delta)) = handle_dragged {
                let new_screen_rect = handle.resize_rect(unpadded_rect, delta);
                if let Some(p) = info.pages.get(page) {
                    let (a, b) = (xf.screen_to_view(new_screen_rect.min), xf.screen_to_view(new_screen_rect.max));
                    let (u, v) = (p.view_to_user(a.0, a.1), p.view_to_user(b.0, b.1));
                    sel.user_rect = [u[0].min(v[0]) as f64, u[1].min(v[1]) as f64, u[0].max(v[0]) as f64, u[1].max(v[1]) as f64];
                    if let Some(ed) = line_editor.as_deref_mut()
                        && ed.page == sel.page
                        && ed.block == sel.index
                    {
                        ed.source_rect = [sel.user_rect[0] as f32, sel.user_rect[1] as f32, sel.user_rect[2] as f32, sel.user_rect[3] as f32];
                        ed.rect = xf.user_rect(info, ed.page, ed.source_rect);
                        ed.max_width = ed.rect.width();
                    }
                }
                state.active_handle = Some(handle);
                ui.ctx().request_repaint();
                new_screen_rect.expand(2.0)
            } else {
                rect
            };

            // Solid selection border
            painter.rect_stroke(active_rect, CornerRadius::same(2), Stroke::new(1.5, ACCENT_BLUE), egui::StrokeKind::Outside);
            // Light background tint
            painter.rect_filled(active_rect, CornerRadius::same(2), ACCENT_BLUE.gamma_multiply(0.04));

            // Handles (corners and edge midpoints)
            for (_, h_rect) in SelectionHandle::handle_rects(active_rect, handle_size) {
                painter.rect(h_rect, CornerRadius::same(1), Color32::WHITE, Stroke::new(1.0, ACCENT_BLUE), egui::StrokeKind::Middle);
            }

            if drag_stopped {
                let start_rect = state.drag_start_rect.take().unwrap_or(sel.user_rect);
                let offset = [sel.user_rect[0] - start_rect[0], sel.user_rect[1] - start_rect[1]];
                match sel.kind {
                    SelectableKind::TextBlock => {
                        let new_w = (sel.user_rect[2] - sel.user_rect[0]).max(10.0);
                        let mut edits = Vec::new();
                        if offset[0].abs() > 0.001 || offset[1].abs() > 0.001 {
                            edits.push(Edit::MoveObjects {
                                page: sel.page,
                                objects: vec![pdfcraft_engine::ObjectTarget {
                                    kind: pdfcraft_engine::ObjectKind::Text,
                                    index: sel.index,
                                }],
                                offset,
                            });
                        }
                        let text = if let Some(ed) = line_editor.as_deref()
                            && ed.page == sel.page && ed.block == sel.index
                        {
                            ed.text.clone()
                        } else {
                            sel.text.clone().unwrap_or_default()
                        };
                        edits.push(Edit::EditTextBlock {
                            page: sel.page,
                            block: sel.index,
                            text,
                            style: pdfcraft_engine::BlockStyle { width: Some(new_w), ..Default::default() },
                        });
                        pending_edit = Some(if edits.len() == 1 {
                            edits.remove(0)
                        } else {
                            Edit::Batch { label: "Resize text box".into(), edits }
                        });
                    }
                    SelectableKind::Image => {
                        pending_edit = Some(Edit::EditPageImage { page: sel.page, index: sel.index, change: ImageEdit::Move(sel.user_rect) });
                    }
                    SelectableKind::TextLine => {
                        let text = if let Some(ed) = line_editor.as_deref()
                            && ed.page == sel.page && ed.block == sel.index
                        {
                            ed.text.clone()
                        } else {
                            sel.text.clone().unwrap_or_default()
                        };
                        pending_edit = Some(Edit::EditTextLine { page: sel.page, line: sel.index, text });
                    }
                    SelectableKind::AddedContent | SelectableKind::VectorLine => {}
                }
                state.active_handle = None;
            }
        }

        pending_edit
    }
}

/// Action determined by the zoom-aware navigation engine.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ZoomNavAction {
    /// Turn/jump directly to adjacent page.
    StepPage { forward: bool, target_fraction: f32 },
    /// Smooth vertical panning by a point delta.
    ScrollLine { delta_points: f32 },
    /// Jump viewport by a screenful.
    ScrollScreen { forward: bool },
    /// No navigation action needed.
    None,
}

/// Zoom-aware navigation engine for intelligent arrow-key stepping and panning.
pub struct ZoomNavigationEngine;

impl ZoomNavigationEngine {
    /// Evaluate navigation inputs based on zoom level and page fit conditions.
    ///
    /// - In `Fit::Page` or single-page mode where the entire page fits within the viewport,
    ///   `ArrowUp`/`ArrowDown` steps whole pages immediately without dead scroll resistance.
    /// - When zoomed in (`zoom > 1.0` or page height exceeds viewport), `ArrowUp`/`ArrowDown`
    ///   smoothly pans by line increments, stepping to the adjacent page when boundary is reached.
    #[allow(clippy::too_many_arguments)]
    pub fn evaluate_navigation_step(
        key: Key,
        fit: Fit,
        continuous: bool,
        _zoom: f32,
        single_edges: Option<(usize, bool, bool)>,
        current_page: usize,
        speed_factor: f32,
        _viewport_h: f32,
    ) -> ZoomNavAction {
        let forward = match key {
            Key::ArrowDown | Key::PageDown => true,
            Key::ArrowUp | Key::PageUp => false,
            _ => return ZoomNavAction::None,
        };

        let is_page_step = matches!(key, Key::PageDown | Key::PageUp);
        let base_line_step = 40.0 * speed_factor.max(0.2);

        if continuous {
            if is_page_step {
                ZoomNavAction::ScrollScreen { forward }
            } else {
                let delta = if forward { base_line_step } else { -base_line_step };
                ZoomNavAction::ScrollLine { delta_points: delta }
            }
        } else {
            let entire_page_fits = fit == Fit::Page || single_edges.is_some_and(|(p, top, bottom)| p == current_page && top && bottom);

            if entire_page_fits {
                ZoomNavAction::StepPage { forward, target_fraction: if forward { 0.0 } else { 1.0 } }
            } else if is_page_step {
                let at_boundary = single_edges.is_some_and(|(p, top, bottom)| p == current_page && if forward { bottom } else { top });
                if at_boundary {
                    ZoomNavAction::StepPage { forward, target_fraction: if forward { 0.0 } else { 1.0 } }
                } else {
                    ZoomNavAction::ScrollScreen { forward }
                }
            } else {
                let at_boundary = single_edges.is_some_and(|(p, top, bottom)| p == current_page && if forward { bottom } else { top });
                if at_boundary {
                    ZoomNavAction::StepPage { forward, target_fraction: if forward { 0.0 } else { 1.0 } }
                } else {
                    let delta = if forward { base_line_step } else { -base_line_step };
                    ZoomNavAction::ScrollLine { delta_points: delta }
                }
            }
        }
    }
}

/// Actions triggered by the floating selection context menu.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FloatingMenuAction {
    /// Cut the element content to clipboard and remove it from the document.
    Cut,
    /// Copy the element content to clipboard.
    Copy,
    /// Paste content from clipboard onto the canvas or replace the selected element.
    Paste,
    /// Open the inline line/paragraph editor for this text element.
    EditText,
    /// Delete the element from the document.
    Delete,
    /// Trigger text recognition (OCR) on the selected element or page.
    RecognizeOcr,
    /// Format Painter: copy or apply style formatting.
    FormatPainter,
    /// Toggle bold text styling.
    ToggleBold,
    /// Toggle italic text styling.
    ToggleItalic,
    /// Toggle strikethrough text styling.
    ToggleStrikethrough,
    /// Open the symbol insertion palette.
    OpenSymbolPalette,
    /// Change stroke color for selected line or shape: `[r, g, b]` (0.0..=1.0)
    SetStrokeColor([f64; 3]),
    /// Change stroke width in points
    SetStrokeWidth(f64),
    /// Change dash pattern index: 0 = solid, 1 = dotted, 2 = dashed, 3 = dash-dot
    SetDash(usize),
    /// Toggle connected perpendicular lines mode (Excel grid)
    ToggleGridMode,
    /// Duplicate the selected element (PowerPoint-style Ctrl+D)
    Duplicate,
    /// Toggle word-like paragraph reflow mode
    ToggleReflow,
}

/// Floating context menu and quick action toolbar anchored to canvas selections.
pub struct FloatingContextMenu;

impl FloatingContextMenu {
    /// Calculate the screen anchor coordinates `Pos2` for the floating toolbar.
    ///
    /// Prefers positioning above the bounding box (`rect.top() - menu_height - 6.0`),
    /// falling back to below (`rect.bottom() + 6.0`) when clipped against the top of the viewport.
    /// Also clamps horizontally so the menu stays fully inside `viewport` bounds.
    pub fn compute_anchor(bounds: Rect, viewport: Rect, menu_width: f32, menu_height: f32) -> Option<Pos2> {
        if !viewport.intersects(bounds) {
            return None;
        }

        let margin = 8.0;
        let x = (bounds.center().x - menu_width * 0.5)
            .clamp(viewport.left() + margin, (viewport.right() - menu_width - margin).max(viewport.left() + margin));

        let space_above = bounds.top() - viewport.top();
        let y = if space_above >= menu_height + 10.0 {
            bounds.top() - menu_height - 6.0
        } else {
            (bounds.bottom() + 6.0).min(viewport.bottom() - menu_height - margin)
        };

        Some(Pos2::new(x, y))
    }

    /// Render the floating context menu anchored to the selected element.
    ///
    /// Shows a pill toolbar with Cut, Copy, Paste, Format Painter, Edit Text, Bold, Italic, Strikethrough, Symbols, and Delete icon buttons.
    pub fn show(ui: &mut egui::Ui, viewport: Rect, view: &DocView, info: &DocInfo, can_modify: bool, can_copy: bool) -> Option<FloatingMenuAction> {
        if view.line_editor.is_some() {
            return None;
        }

        let sel = view.canvas_selection.selected.as_ref()?;
        let xf = view.page_xform(sel.page)?;
        let screen_rect = sel.screen_rect(&xf, info);

        if sel.kind == SelectableKind::VectorLine {
            let menu_width = 330.0;
            let menu_height = 36.0;
            let anchor = Self::compute_anchor(screen_rect, viewport, menu_width, menu_height)?;
            let t = crate::theme::Tokens::get(ui.ctx());
            let mut action = None;

            egui::Area::new(egui::Id::new(("floating-line-menu", view.id.0, sel.page, sel.index)))
                .order(egui::Order::Foreground)
                .fixed_pos(anchor)
                .show(ui.ctx(), |ui| {
                    egui::Frame::NONE
                        .fill(t.card)
                        .stroke(Stroke::new(1.0, t.border))
                        .corner_radius(CornerRadius::same(8))
                        .shadow(egui::Shadow { offset: [0, 4], blur: 14, spread: 0, color: Color32::from_black_alpha(if t.dark() { 90 } else { 32 }) })
                        .inner_margin(egui::Margin::symmetric(6, 4))
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            ui.horizontal(|ui| {
                                // 1. Color Swatch & Quick Palette
                                let cur_color = view.line_properties.color;
                                let color_32 = Color32::from_rgb((cur_color[0] * 255.0) as u8, (cur_color[1] * 255.0) as u8, (cur_color[2] * 255.0) as u8);
                                let (rect, resp) = ui.allocate_exact_size(vec2(20.0, 20.0), egui::Sense::click());
                                ui.painter().rect_filled(rect, 4.0, color_32);
                                ui.painter().rect_stroke(rect, CornerRadius::same(4), Stroke::new(1.0, t.border), egui::StrokeKind::Outside);
                                egui::Popup::menu(&resp).show(|ui| {
                                    ui.label("Line Color:");
                                    let palette = [
                                        ([0.0, 0.0, 0.0], "Black"),
                                        ([0.85, 0.15, 0.15], "Red"),
                                        ([0.08, 0.45, 0.90], "Blue"),
                                        ([0.10, 0.65, 0.25], "Green"),
                                        ([0.95, 0.55, 0.10], "Orange"),
                                        ([0.50, 0.50, 0.50], "Gray"),
                                    ];
                                    ui.horizontal(|ui| {
                                        for (rgb, name) in palette {
                                            let col = Color32::from_rgb((rgb[0] * 255.0) as u8, (rgb[1] * 255.0) as u8, (rgb[2] * 255.0) as u8);
                                            let (cr, cresp) = ui.allocate_exact_size(vec2(18.0, 18.0), egui::Sense::click());
                                            ui.painter().rect_filled(cr, 3.0, col);
                                            ui.painter().rect_stroke(cr, CornerRadius::same(3), Stroke::new(1.0, t.border), egui::StrokeKind::Outside);
                                            if cresp.on_hover_text(name).clicked() {
                                                action = Some(FloatingMenuAction::SetStrokeColor(rgb));
                                                ui.close();
                                            }
                                        }
                                    });
                                });

                                ui.separator();

                                // 2. Stroke Width (Thickness)
                                let cur_w = view.line_properties.width;
                                if crate::icons::button(ui, "minus", 20.0, false, "Thinner").clicked() {
                                    action = Some(FloatingMenuAction::SetStrokeWidth((cur_w - 0.5).max(0.5)));
                                }
                                ui.label(format!("{:.1}pt", cur_w));
                                if crate::icons::button(ui, "plus", 20.0, false, "Thicker").clicked() {
                                    action = Some(FloatingMenuAction::SetStrokeWidth((cur_w + 0.5).min(20.0)));
                                }

                                ui.separator();

                                // 3. Dash Type (Solid, Dotted, Dashed)
                                let dash_names = ["Solid ──", "Dotted ··", "Dashed --", "DashDot -·-"];
                                let cur_dash = dash_names.get(view.line_properties.dash_idx).copied().unwrap_or(dash_names[0]);
                                egui::ComboBox::from_id_salt("floating_line_dash")
                                    .selected_text(cur_dash)
                                    .width(72.0)
                                    .show_ui(ui, |ui| {
                                        for (idx, name) in dash_names.iter().enumerate() {
                                            if ui.selectable_label(view.line_properties.dash_idx == idx, *name).clicked() {
                                                action = Some(FloatingMenuAction::SetDash(idx));
                                            }
                                        }
                                    });

                                ui.separator();

                                // 4. Excel Grid Toggle
                                let extend = view.extend_perpendicular_lines;
                                let grid_tip = if extend {
                                    "Connected Grid Mode: ON (drag extends perpendicular lines)"
                                } else {
                                    "Connected Grid Mode: OFF (click to extend perpendicular lines)"
                                };
                                if crate::icons::button(ui, "grid-3x3", 24.0, extend, grid_tip).clicked() {
                                    action = Some(FloatingMenuAction::ToggleGridMode);
                                }

                                // 5. Duplicate (Ctrl+D)
                                if crate::icons::button(ui, "copy-plus", 24.0, false, "Duplicate (Ctrl+D)").clicked() {
                                    action = Some(FloatingMenuAction::Duplicate);
                                }

                                // 6. Delete
                                if crate::icons::button(ui, "trash-2", 24.0, false, tl!("Delete")).clicked() {
                                    action = Some(FloatingMenuAction::Delete);
                                }
                            });
                        });
                });

            return action;
        }

        let is_text = matches!(sel.kind, SelectableKind::TextLine | SelectableKind::TextBlock);
        let is_image = matches!(sel.kind, SelectableKind::Image);
        let menu_width = if is_text {
            330.0
        } else if is_image {
            190.0
        } else {
            150.0
        };
        let menu_height = 36.0;

        let anchor = Self::compute_anchor(screen_rect, viewport, menu_width, menu_height)?;

        let t = crate::theme::Tokens::get(ui.ctx());
        let mut action = None;

        egui::Area::new(egui::Id::new(("floating-selection-menu", view.id.0, sel.page, sel.index)))
            .order(egui::Order::Foreground)
            .fixed_pos(anchor)
            .show(ui.ctx(), |ui| {
                egui::Frame::NONE
                    .fill(t.card)
                    .stroke(Stroke::new(1.0, t.border))
                    .corner_radius(CornerRadius::same(8))
                    .shadow(egui::Shadow { offset: [0, 4], blur: 14, spread: 0, color: Color32::from_black_alpha(if t.dark() { 90 } else { 32 }) })
                    .inner_margin(egui::Margin::symmetric(6, 4))
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        let shortcut = |key: &'static str| crate::commands::shortcut_label(ui.ctx(), pdfcraft_engine::commands::Shortcut::cmd(key));
                        let cut_tip = format!("Cut ({})", shortcut("X"));
                        let copy_tip = format!("Copy ({})", shortcut("C"));
                        let paste_tip = format!("Paste ({})", shortcut("V"));
                        ui.horizontal(|ui| {
                            // Cut (Ctrl+X)
                            if can_modify && can_copy && sel.text.is_some() && crate::icons::button(ui, "scissors", 26.0, false, &cut_tip).clicked() {
                                action = Some(FloatingMenuAction::Cut);
                            }
                            // Copy (Ctrl+C)
                            if can_copy && sel.text.is_some() && crate::icons::button(ui, "copy", 26.0, false, &copy_tip).clicked() {
                                action = Some(FloatingMenuAction::Copy);
                            }
                            // Paste (Ctrl+V)
                            if can_modify && crate::icons::button(ui, "clipboard-paste", 26.0, false, &paste_tip).clicked() {
                                action = Some(FloatingMenuAction::Paste);
                            }

                            // Format Painter (for text elements)
                            if is_text && can_modify {
                                let has_format = view.canvas_selection.format_painter.is_some();
                                let format_tip = if has_format { "Apply formatting (Format Painter)" } else { "Pick up formatting (Format Painter)" };
                                if crate::icons::button(
                                    ui,
                                    "brush",
                                    26.0,
                                    has_format,
                                    format_tip,
                                )
                                .clicked()
                                {
                                    action = Some(FloatingMenuAction::FormatPainter);
                                }
                            }

                            // Edit Text (for text elements)
                            if is_text && can_modify && crate::icons::button(ui, "text-cursor-input", 26.0, false, tl!("Edit text")).clicked() {
                                action = Some(FloatingMenuAction::EditText);
                            }

                            // Quick Text Styles
                            if is_text && can_modify {
                                ui.separator();
                                if crate::icons::button(ui, "bold", 26.0, false, tl!("Bold")).clicked() {
                                    action = Some(FloatingMenuAction::ToggleBold);
                                }
                                if crate::icons::button(ui, "italic", 26.0, false, tl!("Italic")).clicked() {
                                    action = Some(FloatingMenuAction::ToggleItalic);
                                }
                                if crate::icons::button(ui, "strikethrough", 26.0, false, tl!("Strikethrough")).clicked() {
                                    action = Some(FloatingMenuAction::ToggleStrikethrough);
                                }
                                if crate::icons::button(ui, "sparkles", 26.0, view.canvas_selection.symbol_palette_open, "Insert symbol")
                                    .clicked()
                                {
                                    action = Some(FloatingMenuAction::OpenSymbolPalette);
                                }
                                let reflow_on = view.reflow_paragraphs;
                                let reflow_tip = if reflow_on {
                                    "Paragraph Reflow: ON (editing shifts paragraphs below) - click to toggle"
                                } else {
                                    "Paragraph Reflow: OFF (bottom paragraphs stay fixed) - click to toggle"
                                };
                                if crate::icons::button(ui, "arrow-up-down", 26.0, reflow_on, reflow_tip).clicked() {
                                    action = Some(FloatingMenuAction::ToggleReflow);
                                }
                            }

                            // Recognize Text & Duplicate for images
                            if is_image && can_modify {
                                let w = (sel.user_rect[2] - sel.user_rect[0]).abs();
                                let h = (sel.user_rect[3] - sel.user_rect[1]).abs();
                                ui.label(format!("{:.0}×{:.0}pt", w, h));
                                if crate::icons::button(ui, "copy-plus", 26.0, false, "Duplicate (Ctrl+D)").clicked() {
                                    action = Some(FloatingMenuAction::Duplicate);
                                }
                                if crate::icons::button(ui, "scan-text", 26.0, false, "Recognize text (OCR)").clicked() {
                                    action = Some(FloatingMenuAction::RecognizeOcr);
                                }
                            }

                            // Separator before Delete
                            if can_modify {
                                ui.separator();
                                if crate::icons::button(ui, "trash-2", 26.0, false, "Delete (Del)").clicked() {
                                    action = Some(FloatingMenuAction::Delete);
                                }
                            }
                        });
                    });
            });

        action
    }
}

// ─── Vector line highlights ────────────────────────────────────────────────

/// Which highlight state a line segment is currently in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineHighlightState {
    Hover,
    Selected,
}

impl LineHighlightState {
    /// Colour used for this state.
    pub fn color(self) -> Color32 {
        match self {
            Self::Hover => ACCENT_YELLOW,
            Self::Selected => ACCENT_BLUE,
        }
    }
}

/// Hit-test a single line segment against a screen-space position.
///
/// `tolerance` is the pick radius in screen pixels, typically 4–6.
pub fn hit_test_line(pos: Pos2, line: &LineSegment, xf: &PageXform, info: &DocInfo, tolerance: f32) -> bool {
    let s = project_user_point(xf, info, line.page, line.start);
    let e = project_user_point(xf, info, line.page, line.end);
    let vx = e.x - s.x;
    let vy = e.y - s.y;
    let len_sq = vx * vx + vy * vy;
    let t = if len_sq <= f32::EPSILON { 0.0 } else { (((pos.x - s.x) * vx + (pos.y - s.y) * vy) / len_sq).clamp(0.0, 1.0) };
    let px = s.x + t * vx;
    let py = s.y + t * vy;
    let dx = pos.x - px;
    let dy = pos.y - py;
    (dx * dx + dy * dy).sqrt() <= tolerance
}

/// Paint a highlight over a line segment. Does nothing if the segment is
/// offscreen or degenerate.
pub fn paint_line_highlight(painter: &egui::Painter, line: &LineSegment, state: LineHighlightState, xf: &PageXform, info: &DocInfo) {
    let s = project_user_point(xf, info, line.page, line.start);
    let e = project_user_point(xf, info, line.page, line.end);
    let width = (line.thickness as f32).clamp(1.5, 6.0);
    painter.line_segment([s, e], Stroke::new(width, state.color()));
}

pub fn project_user_point(xf: &PageXform, info: &DocInfo, page: usize, p: [f64; 2]) -> Pos2 {
    if let Some(page_info) = info.pages.get(page) {
        let [vx, vy] = page_info.user_to_view(p[0] as f32, p[1] as f32);
        xf.norm_to_screen(vx / xf.pw.max(1e-3), vy / xf.ph.max(1e-3))
    } else {
        let (x, y) = (p[0] as f32, p[1] as f32);
        xf.user_rect(info, page, [x, y, x, y]).min
    }
}

/// Screen-space rectangles for the two endpoint handles of a line.
/// Only meaningful when the line is the sole selection.
pub fn line_endpoint_handles(line: &pdfcraft_engine::LineSegment, xf: &PageXform, info: &DocInfo, size: f32) -> [(SelectionHandle, Rect); 2] {
    let s = project_user_point(xf, info, line.page, line.start);
    let e = project_user_point(xf, info, line.page, line.end);
    [
        (SelectionHandle::LineStart, Rect::from_center_size(s, vec2(size, size))),
        (SelectionHandle::LineEnd, Rect::from_center_size(e, vec2(size, size))),
    ]
}

/// Hit-test the endpoint handles of a line. Uses an expanded reach beyond the
/// visible handle (8 pixels minimum) so a tap on mobile still connects.
pub fn hit_test_line_endpoint(line: &pdfcraft_engine::LineSegment, pos: Pos2, xf: &PageXform, info: &DocInfo, size: f32) -> Option<SelectionHandle> {
    let reach = (size * 0.5).max(8.0);
    for (handle, rect) in line_endpoint_handles(line, xf, info, size) {
        if rect.expand(reach).contains(pos) {
            return Some(handle);
        }
    }
    None
}

/// Paint endpoint handles for a selected vector line.
pub fn paint_line_endpoint_handles(painter: &egui::Painter, line: &pdfcraft_engine::LineSegment, xf: &PageXform, info: &DocInfo, size: f32) {
    for (_, rect) in line_endpoint_handles(line, xf, info, size) {
        painter.rect(rect, CornerRadius::same(1), Color32::WHITE, Stroke::new(1.0, ACCENT_BLUE), egui::StrokeKind::Middle);
    }
}

/// Screen point → PDF user space point for a given page.
/// Inverse of [`project_user_point`].
pub fn screen_to_user_point(xf: &PageXform, info: &DocInfo, page: usize, p: Pos2) -> [f64; 2] {
    let (vx, vy) = xf.screen_to_view(p);
    if let Some(page_info) = info.pages.get(page) {
        let u = page_info.view_to_user(vx, vy);
        [u[0] as f64, u[1] as f64]
    } else {
        [0.0, 0.0]
    }
}

/// An update to a line's endpoints on the page.
#[derive(Clone, Debug, PartialEq)]
pub struct LineUpdate {
    pub index: usize,
    pub target: pdfcraft_engine::ObjectTarget,
    pub new_start: [f64; 2],
    pub new_end: [f64; 2],
}

/// Computes updates for touching perpendicular lines when `dragged_line` moves from `orig_start` and `orig_end`
/// to `new_start` and `new_end`.
///
/// If a line is perpendicular and one of its endpoints touches the dragged line (within a 4.0pt tolerance),
/// that touching endpoint extends or contracts with the movement of the dragged line, maintaining grid border continuity.
pub fn compute_connected_perpendicular_line_updates(
    lines: &[LineSegment],
    dragged_index: usize,
    orig_start: [f64; 2],
    orig_end: [f64; 2],
    new_start: [f64; 2],
    new_end: [f64; 2],
) -> Vec<LineUpdate> {
    let mut updates = Vec::new();
    let dx = ((new_start[0] - orig_start[0]) + (new_end[0] - orig_end[0])) * 0.5;
    let dy = ((new_start[1] - orig_start[1]) + (new_end[1] - orig_end[1])) * 0.5;

    if dx.abs() < 1e-5 && dy.abs() < 1e-5 {
        return updates;
    }

    let orig_dx = (orig_end[0] - orig_start[0]).abs();
    let orig_dy = (orig_end[1] - orig_start[1]).abs();
    let tol = 4.0;

    let is_horizontal = orig_dx >= orig_dy;
    let is_vertical = orig_dy >= orig_dx;

    for line in lines {
        if line.index == dragged_index {
            continue;
        }

        let l_dx = (line.end[0] - line.start[0]).abs();
        let l_dy = (line.end[1] - line.start[1]).abs();

        if is_horizontal {
            // Dragged line is predominantly horizontal. Perpendicular lines are vertical.
            if l_dy > l_dx {
                let x_min = orig_start[0].min(orig_end[0]) - tol;
                let x_max = orig_start[0].max(orig_end[0]) + tol;
                let y_ref = (orig_start[1] + orig_end[1]) * 0.5;

                let mut modified = false;
                let mut updated_start = line.start;
                let mut updated_end = line.end;

                if (line.start[1] - y_ref).abs() <= tol && line.start[0] >= x_min && line.start[0] <= x_max {
                    updated_start[1] += dy;
                    modified = true;
                }
                if (line.end[1] - y_ref).abs() <= tol && line.end[0] >= x_min && line.end[0] <= x_max {
                    updated_end[1] += dy;
                    modified = true;
                }

                if modified {
                    let len = (updated_start[0] - updated_end[0]).hypot(updated_start[1] - updated_end[1]);
                    if len > 0.5 {
                        updates.push(LineUpdate {
                            index: line.index,
                            target: pdfcraft_engine::ObjectTarget {
                                kind: pdfcraft_engine::ObjectKind::VectorLine,
                                index: line.index,
                            },
                            new_start: updated_start,
                            new_end: updated_end,
                        });
                    }
                }
            }
        } else if is_vertical {
            // Dragged line is predominantly vertical. Perpendicular lines are horizontal.
            if l_dx > l_dy {
                let y_min = orig_start[1].min(orig_end[1]) - tol;
                let y_max = orig_start[1].max(orig_end[1]) + tol;
                let x_ref = (orig_start[0] + orig_end[0]) * 0.5;

                let mut modified = false;
                let mut updated_start = line.start;
                let mut updated_end = line.end;

                if (line.start[0] - x_ref).abs() <= tol && line.start[1] >= y_min && line.start[1] <= y_max {
                    updated_start[0] += dx;
                    modified = true;
                }
                if (line.end[0] - x_ref).abs() <= tol && line.end[1] >= y_min && line.end[1] <= y_max {
                    updated_end[0] += dx;
                    modified = true;
                }

                if modified {
                    let len = (updated_start[0] - updated_end[0]).hypot(updated_start[1] - updated_end[1]);
                    if len > 0.5 {
                        updates.push(LineUpdate {
                            index: line.index,
                            target: pdfcraft_engine::ObjectTarget {
                                kind: pdfcraft_engine::ObjectKind::VectorLine,
                                index: line.index,
                            },
                            new_start: updated_start,
                            new_end: updated_end,
                        });
                    }
                }
            }
        }
    }

    updates
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_selectable_element_line_construction() {
        let line = SelectableElement::new_line(0, 5, [10.0, 20.0, 110.0, 35.0], "Hello PDF".into(), Some([10.0, 22.0]));
        assert_eq!(line.page, 0);
        assert_eq!(line.kind, SelectableKind::TextLine);
        assert_eq!(line.index, 5);
        assert_eq!(line.text.as_deref(), Some("Hello PDF"));
        assert_eq!(line.baseline, Some([10.0, 22.0]));
    }

    #[test]
    fn test_selection_handle_hit_test() {
        let rect = Rect::from_min_max(Pos2::new(100.0, 100.0), Pos2::new(200.0, 200.0));
        let handle = SelectionHandle::hit_test(rect, Pos2::new(100.0, 100.0), 8.0);
        assert_eq!(handle, Some(SelectionHandle::TopLeft));

        let handle_center = SelectionHandle::hit_test(rect, Pos2::new(150.0, 150.0), 8.0);
        assert_eq!(handle_center, Some(SelectionHandle::Move));

        let handle_outside = SelectionHandle::hit_test(rect, Pos2::new(50.0, 50.0), 8.0);
        assert_eq!(handle_outside, None);
    }

    #[test]
    fn test_delete_key_action_for_line() {
        let mut state = SelectionState::default();
        let line = SelectableElement::new_line(2, 7, [0.0, 0.0, 100.0, 20.0], "Delete me".into(), None);
        state.selected = Some(line);

        let edit = SelectionLayoutEngine::handle_delete_key(&mut state);
        assert!(state.selected.is_none());
        assert_eq!(edit, Some(Edit::EditTextLine { page: 2, line: 7, text: String::new() }));
    }

    #[test]
    fn test_delete_key_action_for_block() {
        let mut state = SelectionState::default();
        let block = SelectableElement::new_block(1, 3, [0.0, 0.0, 200.0, 100.0], "Paragraph text".into());
        state.selected = Some(block);

        let edit = SelectionLayoutEngine::handle_delete_key(&mut state);
        assert!(state.selected.is_none());
        assert!(matches!(edit, Some(Edit::EditTextBlock { page: 1, block: 3, text, .. }) if text.is_empty()));
    }

    #[test]
    fn test_zoom_nav_action_fit_page() {
        let action = ZoomNavigationEngine::evaluate_navigation_step(Key::ArrowDown, Fit::Page, false, 1.0, Some((0, true, true)), 0, 1.0, 800.0);
        assert_eq!(action, ZoomNavAction::StepPage { forward: true, target_fraction: 0.0 });

        let action_up = ZoomNavigationEngine::evaluate_navigation_step(Key::ArrowUp, Fit::Page, false, 1.0, Some((1, true, true)), 1, 1.0, 800.0);
        assert_eq!(action_up, ZoomNavAction::StepPage { forward: false, target_fraction: 1.0 });
    }

    #[test]
    fn test_zoom_nav_action_zoomed_in() {
        // Zoomed in: single_edges top/bottom false
        let action = ZoomNavigationEngine::evaluate_navigation_step(Key::ArrowDown, Fit::None, false, 2.0, Some((0, true, false)), 0, 1.0, 600.0);
        assert_eq!(action, ZoomNavAction::ScrollLine { delta_points: 40.0 });

        // Zoomed in at bottom boundary
        let action_boundary =
            ZoomNavigationEngine::evaluate_navigation_step(Key::ArrowDown, Fit::None, false, 2.0, Some((0, false, true)), 0, 1.0, 600.0);
        assert_eq!(action_boundary, ZoomNavAction::StepPage { forward: true, target_fraction: 0.0 });
    }

    #[test]
    fn test_floating_menu_anchor_above() {
        let viewport = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1000.0, 800.0));
        let bounds = Rect::from_min_max(Pos2::new(200.0, 200.0), Pos2::new(400.0, 250.0));
        let anchor = FloatingContextMenu::compute_anchor(bounds, viewport, 180.0, 36.0);
        assert!(anchor.is_some());
        let a = anchor.unwrap();
        // Positioned above the bounding box
        assert!(a.y < bounds.top());
        // Centered relative to bounds
        assert_eq!(a.x, 300.0 - 90.0);
    }

    #[test]
    fn test_floating_menu_anchor_below_fallback() {
        let viewport = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1000.0, 800.0));
        // Element at the very top of viewport (no room above)
        let bounds = Rect::from_min_max(Pos2::new(200.0, 10.0), Pos2::new(400.0, 60.0));
        let anchor = FloatingContextMenu::compute_anchor(bounds, viewport, 180.0, 36.0);
        assert!(anchor.is_some());
        let a = anchor.unwrap();
        // Positioned below the bounding box
        assert!(a.y >= bounds.bottom());
    }

    #[test]
    fn test_floating_menu_horizontal_clamping() {
        let viewport = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1000.0, 800.0));
        // Near left edge
        let bounds_left = Rect::from_min_max(Pos2::new(10.0, 200.0), Pos2::new(50.0, 250.0));
        let anchor_left = FloatingContextMenu::compute_anchor(bounds_left, viewport, 180.0, 36.0).unwrap();
        assert!(anchor_left.x >= 8.0);

        // Near right edge
        let bounds_right = Rect::from_min_max(Pos2::new(950.0, 200.0), Pos2::new(990.0, 250.0));
        let anchor_right = FloatingContextMenu::compute_anchor(bounds_right, viewport, 180.0, 36.0).unwrap();
        assert!(anchor_right.x + 180.0 <= 1000.0 - 8.0);
    }

    #[test]
    fn test_selection_handle_resize_rect() {
        let rect = Rect::from_min_max(Pos2::new(100.0, 100.0), Pos2::new(200.0, 200.0));

        // Resizing TopLeft
        let r_tl = SelectionHandle::TopLeft.resize_rect(rect, vec2(-10.0, -20.0));
        assert_eq!(r_tl.min, Pos2::new(90.0, 80.0));
        assert_eq!(r_tl.max, Pos2::new(200.0, 200.0));

        // Resizing BottomRight
        let r_br = SelectionHandle::BottomRight.resize_rect(rect, vec2(15.0, 25.0));
        assert_eq!(r_br.min, Pos2::new(100.0, 100.0));
        assert_eq!(r_br.max, Pos2::new(215.0, 225.0));

        // Resizing Right
        let r_r = SelectionHandle::Right.resize_rect(rect, vec2(50.0, 0.0));
        assert_eq!(r_r.width(), 150.0);

        // Clamping to MIN_SIZE
        let r_clamped = SelectionHandle::Right.resize_rect(rect, vec2(-200.0, 0.0));
        assert!(r_clamped.width() >= 10.0);
    }

    #[test]
    fn test_connected_perpendicular_lines_grid_adjustment() {
        // Vertical column line at x = 100
        let col = LineSegment {
            page: 0,
            index: 0,
            start: [100.0, 50.0],
            end: [100.0, 150.0],
            thickness: 1.0,
        };
        // Horizontal row line touching col from left: end at (100, 80)
        let row_left = LineSegment {
            page: 0,
            index: 1,
            start: [50.0, 80.0],
            end: [100.0, 80.0],
            thickness: 1.0,
        };
        // Horizontal row line touching col from right: start at (100, 80)
        let row_right = LineSegment {
            page: 0,
            index: 2,
            start: [100.0, 80.0],
            end: [180.0, 80.0],
            thickness: 1.0,
        };
        // An unrelated line far away
        let row_far = LineSegment {
            page: 0,
            index: 3,
            start: [20.0, 20.0],
            end: [40.0, 20.0],
            thickness: 1.0,
        };

        let lines = vec![col.clone(), row_left, row_right, row_far];

        // Drag column line right by +25.0pt (from x=100 to x=125)
        let new_start = [125.0, 50.0];
        let new_end = [125.0, 150.0];

        let updates = compute_connected_perpendicular_line_updates(&lines, 0, col.start, col.end, new_start, new_end);

        assert_eq!(updates.len(), 2, "Both touching perpendicular row lines should be updated");

        let left_up = updates.iter().find(|u| u.index == 1).expect("row_left update must exist");
        assert_eq!(left_up.new_start, [50.0, 80.0]);
        assert_eq!(left_up.new_end, [125.0, 80.0], "row_left should extend its end to x=125");

        let right_up = updates.iter().find(|u| u.index == 2).expect("row_right update must exist");
        assert_eq!(right_up.new_start, [125.0, 80.0], "row_right should shift its start to x=125");
        assert_eq!(right_up.new_end, [180.0, 80.0]);
    }

    #[test]
    fn test_selectable_element_vector_line_and_bbox() {
        let seg = LineSegment {
            page: 1,
            index: 4,
            start: [10.0, 50.0],
            end: [110.0, 50.0],
            thickness: 2.0,
        };
        let el = SelectableElement::new_line_vector(1, seg);
        assert_eq!(el.page, 1);
        assert_eq!(el.kind, SelectableKind::VectorLine);
        assert_eq!(el.index, 4);
        assert_eq!(el.user_rect, [10.0, 50.0, 110.0, 50.0]);
    }
}
