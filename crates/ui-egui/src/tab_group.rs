//! Microsoft Edge-styled tab grouping and management for document tabs.

use egui::{Color32, CornerRadius, Rect, Response, Sense, Stroke, vec2};

use crate::PdfCraftApp;
use crate::theme::Tokens;

/// Unique identifier for a tab group.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TabGroupId(pub u64);

/// Microsoft Edge standard curated group colors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabGroupColor {
    Grey,
    Blue,
    Red,
    Yellow,
    Green,
    Pink,
    Purple,
    Cyan,
    Orange,
}

impl TabGroupColor {
    pub const ALL: [Self; 9] = [Self::Grey, Self::Blue, Self::Red, Self::Yellow, Self::Green, Self::Pink, Self::Purple, Self::Cyan, Self::Orange];

    pub fn name(self) -> &'static str {
        match self {
            Self::Grey => "Grey",
            Self::Blue => "Blue",
            Self::Red => "Red",
            Self::Yellow => "Yellow",
            Self::Green => "Green",
            Self::Pink => "Pink",
            Self::Purple => "Purple",
            Self::Cyan => "Cyan",
            Self::Orange => "Orange",
        }
    }

    pub fn color32(self) -> Color32 {
        match self {
            Self::Grey => Color32::from_rgb(117, 117, 117),
            Self::Blue => Color32::from_rgb(26, 115, 232),
            Self::Red => Color32::from_rgb(217, 48, 37),
            Self::Yellow => Color32::from_rgb(249, 171, 0),
            Self::Green => Color32::from_rgb(30, 142, 62),
            Self::Pink => Color32::from_rgb(229, 37, 146),
            Self::Purple => Color32::from_rgb(147, 52, 230),
            Self::Cyan => Color32::from_rgb(18, 181, 203),
            Self::Orange => Color32::from_rgb(250, 123, 23),
        }
    }

    pub fn text_color(self) -> Color32 {
        match self {
            Self::Yellow => Color32::from_rgb(32, 33, 36),
            _ => Color32::WHITE,
        }
    }
}

/// A tab group representing a set of grouped document tabs.
#[derive(Clone, Debug, PartialEq)]
pub struct TabGroup {
    pub id: TabGroupId,
    pub title: String,
    pub color: TabGroupColor,
    pub collapsed: bool,
}

impl TabGroup {
    pub fn new(id: TabGroupId, title: String, color: TabGroupColor) -> Self {
        Self { id, title, color, collapsed: false }
    }
}

impl PdfCraftApp {
    /// Create a new tab group and return its identifier.
    pub fn create_tab_group(&mut self, title: String, color: TabGroupColor) -> TabGroupId {
        let id = TabGroupId(self.next_group_id);
        self.next_group_id = self.next_group_id.saturating_add(1);
        self.tab_groups.push(TabGroup::new(id, title, color));
        id
    }

    /// Look up a tab group by its id.
    pub fn group(&self, id: TabGroupId) -> Option<&TabGroup> {
        self.tab_groups.iter().find(|g| g.id == id)
    }

    /// Look up a mutable tab group by its id.
    pub fn group_mut(&mut self, id: TabGroupId) -> Option<&mut TabGroup> {
        self.tab_groups.iter_mut().find(|g| g.id == id)
    }

    /// Look up the group for a tab at index `tab_index`.
    pub fn group_for_tab(&self, tab_index: usize) -> Option<&TabGroup> {
        let gid = self.views.get(tab_index)?.group?;
        self.group(gid)
    }

    /// Add tab at `tab_index` to group `group_id`.
    /// Tabs in a group are contiguous: moves the tab adjacent to the group if needed.
    pub fn add_tab_to_group(&mut self, tab_index: usize, group_id: TabGroupId) {
        if tab_index >= self.views.len() {
            return;
        }
        self.views[tab_index].group = Some(group_id);
        self.views[tab_index].pinned = false;

        // Position this tab contiguous with other tabs of the group
        let group_tab_indices: Vec<usize> =
            self.views.iter().enumerate().filter(|(i, v)| *i != tab_index && v.group == Some(group_id)).map(|(i, _)| i).collect();

        if let Some(&last_idx) = group_tab_indices.last() {
            if tab_index > last_idx + 1 {
                let view = self.views.remove(tab_index);
                let target = last_idx + 1;
                self.views.insert(target.min(self.views.len()), view);
                self.active = Some(target.min(self.views.len().saturating_sub(1)));
            } else if tab_index < group_tab_indices[0] {
                let view = self.views.remove(tab_index);
                let target = group_tab_indices[0];
                self.views.insert(target.min(self.views.len()), view);
                self.active = Some(target.min(self.views.len().saturating_sub(1)));
            }
        }
    }

    /// Remove tab at `tab_index` from its current group.
    pub fn remove_tab_from_group(&mut self, tab_index: usize) {
        let Some(view) = self.views.get_mut(tab_index) else { return };
        let prev_group = view.group.take();
        if let Some(gid) = prev_group {
            self.cleanup_empty_group(gid);
        }
    }

    /// Disband a group without closing its member tabs.
    pub fn ungroup(&mut self, group_id: TabGroupId) {
        for v in &mut self.views {
            if v.group == Some(group_id) {
                v.group = None;
            }
        }
        self.tab_groups.retain(|g| g.id != group_id);
        if self.editing_group == Some(group_id) {
            self.editing_group = None;
        }
    }

    /// Close all tabs in a group.
    pub fn close_tab_group(&mut self, group_id: TabGroupId) {
        let member_indices: Vec<usize> = self.views.iter().enumerate().filter(|(_, v)| v.group == Some(group_id)).map(|(i, _)| i).collect();
        for &idx in member_indices.iter().rev() {
            self.request_close_tab(idx);
        }
        self.tab_groups.retain(|g| g.id != group_id);
        if self.editing_group == Some(group_id) {
            self.editing_group = None;
        }
    }

    /// Toggle collapsed state of a group.
    pub fn toggle_group_collapsed(&mut self, group_id: TabGroupId) {
        if let Some(g) = self.group_mut(group_id) {
            g.collapsed = !g.collapsed;
        }
    }

    /// Pin or unpin a tab. Pinned tabs sit on the far left.
    pub fn set_tab_pinned(&mut self, tab_index: usize, pinned: bool) {
        if tab_index >= self.views.len() {
            return;
        }
        self.views[tab_index].pinned = pinned;
        if pinned {
            // Pinned tabs cannot be in a group
            let prev_group = self.views[tab_index].group.take();
            if let Some(gid) = prev_group {
                self.cleanup_empty_group(gid);
            }
            // Move to end of pinned tabs section
            let target = self.views.iter().take(tab_index).filter(|v| v.pinned).count();
            if target != tab_index {
                let view = self.views.remove(tab_index);
                self.views.insert(target.min(self.views.len()), view);
                self.active = Some(target.min(self.views.len().saturating_sub(1)));
            }
        } else {
            // Move to beginning of unpinned tabs section
            let pinned_count = self.views.iter().filter(|v| v.pinned).count();
            if tab_index < pinned_count {
                let view = self.views.remove(tab_index);
                self.views.insert(pinned_count.min(self.views.len()), view);
                self.active = Some(pinned_count.min(self.views.len().saturating_sub(1)));
            }
        }
    }

    /// Duplicate a tab (creates a second twin view of the document).
    pub fn duplicate_tab(&mut self, tab_index: usize) {
        let Some(view) = self.views.get(tab_index) else { return };
        let id = view.id;
        let group = view.group;
        let Some(doc) = self.session.get(id) else { return };
        let mut twin = view.twin(&doc.info);
        twin.group = group;
        twin.pinned = false;
        let at = (tab_index + 1).min(self.views.len());
        self.views.insert(at, twin);
        self.active = Some(at);
    }

    /// Close all tabs except `keep_index`.
    pub fn close_other_tabs(&mut self, keep_index: usize) {
        let len = self.views.len();
        for i in (0..len).rev() {
            if i != keep_index {
                self.request_close_tab(i);
            }
        }
    }

    /// Close all tabs to the right of `from_index`.
    pub fn close_tabs_to_right(&mut self, from_index: usize) {
        let len = self.views.len();
        for i in (from_index + 1..len).rev() {
            self.request_close_tab(i);
        }
    }

    /// Remove a group if it has no member tabs remaining.
    pub fn cleanup_empty_group(&mut self, group_id: TabGroupId) {
        let has_tabs = self.views.iter().any(|v| v.group == Some(group_id));
        if !has_tabs {
            self.tab_groups.retain(|g| g.id != group_id);
            if self.editing_group == Some(group_id) {
                self.editing_group = None;
            }
        }
    }
}

/// Natural width required by a group header pill.
pub fn group_pill_natural_width(ui: &egui::Ui, title: &str) -> f32 {
    if title.trim().is_empty() {
        24.0
    } else {
        let font = crate::theme::medium(11.0);
        let text_w = ui.fonts_mut(|f| f.layout_no_wrap(title.to_string(), font, Color32::WHITE).size().x);
        text_w + 16.0
    }
}

/// Render an Edge-styled group header pill.
pub fn group_pill(ui: &mut egui::Ui, group: &TabGroup, _t: &Tokens, height: f32, on_edit: &mut Option<TabGroupId>) -> Response {
    let font = crate::theme::medium(11.0);
    let title = group.title.trim();
    let text_color = group.color.text_color();
    let bg_color = group.color.color32();

    let width = if title.is_empty() {
        22.0
    } else {
        let text_w = ui.fonts_mut(|f| f.layout_no_wrap(title.to_string(), font.clone(), text_color).size().x);
        (text_w + 16.0).max(24.0)
    };

    let pill_height = 20.0f32.min(height - 4.0);
    let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::click_and_drag());
    let pill_rect = Rect::from_center_size(rect.center(), vec2(width, pill_height));

    let is_hovered = response.hovered();
    let fill = if is_hovered { bg_color.lerp_to_gamma(Color32::WHITE, 0.15) } else { bg_color };

    ui.painter().rect_filled(pill_rect, CornerRadius::same(5), fill);

    if title.is_empty() {
        // Draw a neat indicator dot if untitled
        ui.painter().circle_filled(pill_rect.center(), 4.0, text_color);
    } else {
        let galley = crate::bidi::layout_ltr(ui, title.to_string(), font, text_color);
        let text_pos = pill_rect.min + vec2((pill_rect.width() - galley.size().x) / 2.0, (pill_rect.height() - galley.size().y) / 2.0);
        ui.painter().galley(text_pos, galley, text_color);
    }

    // Collapse indicator badge when collapsed
    if group.collapsed {
        let icon_rect = Rect::from_center_size(pill_rect.right_center() - vec2(4.0, 0.0), vec2(8.0, 8.0));
        ui.painter().circle_filled(icon_rect.center(), 2.0, text_color);
    }

    if response.secondary_clicked() {
        *on_edit = Some(group.id);
    }

    let tooltip = if group.collapsed {
        format!("{}: Click to expand", if title.is_empty() { "Tab group" } else { title })
    } else {
        format!("{}: Click to collapse, right-click to edit", if title.is_empty() { "Tab group" } else { title })
    };
    response.on_hover_text(tooltip)
}

/// Render the Group Edit Popup / Context Menu (Edge standard: name input, color swatches, actions).
pub fn group_editor_popup(app: &mut PdfCraftApp, ui: &mut egui::Ui, group_id: TabGroupId, close_popup: &mut bool) {
    let _t = Tokens::get(ui.ctx());
    let Some(group) = app.group(group_id).cloned() else {
        *close_popup = true;
        return;
    };

    ui.set_min_width(220.0);
    ui.vertical(|ui| {
        ui.add_space(4.0);
        ui.label(egui::RichText::new(tl!("Group name")).font(crate::theme::medium(12.0)));
        let mut title_buf = app.group_title_draft.clone();
        let edit = ui.text_edit_singleline(&mut title_buf);
        if edit.changed() {
            app.group_title_draft = title_buf.clone();
            if let Some(g) = app.group_mut(group_id) {
                g.title = title_buf;
            }
        }

        ui.add_space(8.0);
        ui.label(egui::RichText::new(tl!("Color")).font(crate::theme::medium(12.0)));
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            for &color in &TabGroupColor::ALL {
                let col = color.color32();
                let selected = group.color == color;
                let (swatch_rect, swatch_resp) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::click());
                if swatch_resp.hovered() {
                    ui.painter().circle_filled(swatch_rect.center(), 10.0, col.lerp_to_gamma(Color32::WHITE, 0.2));
                } else {
                    ui.painter().circle_filled(swatch_rect.center(), 9.0, col);
                }
                if selected {
                    ui.painter().circle_stroke(swatch_rect.center(), 8.0, Stroke::new(2.0, Color32::WHITE));
                }
                if swatch_resp.clicked()
                    && let Some(g) = app.group_mut(group_id)
                {
                    g.color = color;
                }
                swatch_resp.on_hover_text(color.name());
            }
        });

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(4.0);

        if ui.button(tl!("New tab in group")).clicked() {
            app.open_dialog();
            *close_popup = true;
        }

        if ui.button(tl!("Ungroup")).clicked() {
            app.ungroup(group_id);
            *close_popup = true;
        }

        if ui.button(tl!("Close group")).clicked() {
            app.close_tab_group(group_id);
            *close_popup = true;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &[u8] = include_bytes!("../tests/data/form.pdf");

    #[test]
    fn create_and_manage_tab_groups() {
        let mut app = PdfCraftApp::new();
        app.open_bytes("doc1.pdf", None, FIXTURE.to_vec()).unwrap();
        app.open_bytes("doc2.pdf", None, FIXTURE.to_vec()).unwrap();
        app.open_bytes("doc3.pdf", None, FIXTURE.to_vec()).unwrap();

        assert_eq!(app.views.len(), 3);
        assert_eq!(app.tab_groups.len(), 0);

        let gid = app.create_tab_group("Research".into(), TabGroupColor::Blue);
        assert_eq!(app.tab_groups.len(), 1);
        assert_eq!(app.tab_groups[0].title, "Research");
        assert_eq!(app.tab_groups[0].color, TabGroupColor::Blue);
        assert!(!app.tab_groups[0].collapsed);

        app.add_tab_to_group(0, gid);
        app.add_tab_to_group(1, gid);

        assert_eq!(app.views[0].group, Some(gid));
        assert_eq!(app.views[1].group, Some(gid));
        assert_eq!(app.views[2].group, None);

        app.toggle_group_collapsed(gid);
        assert!(app.tab_groups[0].collapsed);
        app.toggle_group_collapsed(gid);
        assert!(!app.tab_groups[0].collapsed);

        app.remove_tab_from_group(0);
        assert_eq!(app.views[0].group, None);
        assert_eq!(app.views[1].group, Some(gid));

        app.ungroup(gid);
        assert_eq!(app.tab_groups.len(), 0);
        assert_eq!(app.views[1].group, None);
    }

    #[test]
    fn pin_and_unpin_tabs() {
        let mut app = PdfCraftApp::new();
        app.open_bytes("doc1.pdf", None, FIXTURE.to_vec()).unwrap();
        app.open_bytes("doc2.pdf", None, FIXTURE.to_vec()).unwrap();
        app.open_bytes("doc3.pdf", None, FIXTURE.to_vec()).unwrap();

        app.set_tab_pinned(2, true);
        assert!(app.views[0].pinned);
        assert_eq!(app.session.get(app.views[0].id).unwrap().name, "doc3.pdf");

        app.set_tab_pinned(0, false);
        assert!(!app.views[0].pinned);
    }

    #[test]
    fn duplicate_and_close_tab_actions() {
        let mut app = PdfCraftApp::new();
        app.open_bytes("doc1.pdf", None, FIXTURE.to_vec()).unwrap();
        app.open_bytes("doc2.pdf", None, FIXTURE.to_vec()).unwrap();
        app.open_bytes("doc3.pdf", None, FIXTURE.to_vec()).unwrap();

        app.duplicate_tab(1);
        assert_eq!(app.views.len(), 4);
        assert_eq!(app.views[1].id, app.views[2].id);

        app.close_tabs_to_right(0);
        assert_eq!(app.views.len(), 1);
        assert_eq!(app.session.get(app.views[0].id).unwrap().name, "doc1.pdf");
    }

    #[test]
    fn close_other_tabs_action() {
        let mut app = PdfCraftApp::new();
        app.open_bytes("doc1.pdf", None, FIXTURE.to_vec()).unwrap();
        app.open_bytes("doc2.pdf", None, FIXTURE.to_vec()).unwrap();
        app.open_bytes("doc3.pdf", None, FIXTURE.to_vec()).unwrap();

        app.close_other_tabs(1);
        assert_eq!(app.views.len(), 1);
        assert_eq!(app.session.get(app.views[0].id).unwrap().name, "doc2.pdf");
    }

    #[test]
    fn close_tab_group_action() {
        let mut app = PdfCraftApp::new();
        app.open_bytes("doc1.pdf", None, FIXTURE.to_vec()).unwrap();
        app.open_bytes("doc2.pdf", None, FIXTURE.to_vec()).unwrap();
        app.open_bytes("doc3.pdf", None, FIXTURE.to_vec()).unwrap();

        let gid = app.create_tab_group("Work".into(), TabGroupColor::Green);
        app.add_tab_to_group(0, gid);
        app.add_tab_to_group(1, gid);

        app.close_tab_group(gid);
        assert_eq!(app.tab_groups.len(), 0);
        assert_eq!(app.views.len(), 1);
        assert_eq!(app.session.get(app.views[0].id).unwrap().name, "doc3.pdf");
    }
}
