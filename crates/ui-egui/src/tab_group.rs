//! Microsoft Edge-styled tab grouping and management for document tabs.

use craft::theme::Variant;
use craft::ui::buttons::Button;
use craft::ui::choice::{Swatch, SwatchShape};
use craft::ui::contract::{Bind, Props as _, Show as _, ShowContainer as _};
use craft::ui::display::Text;
use craft::ui::layout_components::Stack;
use craft::ui::text::TextField;
use egui::{Color32, CornerRadius, Rect, Stroke, pos2, vec2};

use crate::PdfCraftApp;

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
            Self::Green => Color32::from_rgb(24, 128, 56),
            Self::Pink => Color32::from_rgb(208, 24, 130),
            Self::Purple => Color32::from_rgb(147, 52, 230),
            Self::Cyan => Color32::from_rgb(18, 181, 203),
            Self::Orange => Color32::from_rgb(250, 123, 23),
        }
    }

    pub fn text_color(self) -> Color32 {
        match self {
            Self::Yellow | Self::Cyan | Self::Orange => Color32::from_rgb(32, 33, 36),
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
    /// Smooth expansion animation progress (0.0 = fully collapsed, 1.0 = fully open).
    pub open_progress: f32,
}

impl TabGroup {
    pub fn new(id: TabGroupId, title: String, color: TabGroupColor) -> Self {
        Self { id, title, color, collapsed: false, open_progress: 1.0 }
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
    /// Tabs in a group are contiguous within their pane: moves the tab adjacent to the group if needed.
    pub fn add_tab_to_group(&mut self, tab_index: usize, group_id: TabGroupId) {
        if tab_index >= self.views.len() {
            return;
        }
        let active = self.active_key();
        let place = self.views[tab_index].pane.place;
        let prev_group = self.views[tab_index].group;
        let was_pinned = self.views[tab_index].pinned;
        self.views[tab_index].group = Some(group_id);
        self.views[tab_index].pinned = false;

        // Position this tab contiguous with other tabs of the group in the same pane.
        let group_tab_indices: Vec<usize> = self
            .views
            .iter()
            .enumerate()
            .filter(|(i, v)| *i != tab_index && v.pane.place == place && v.group == Some(group_id))
            .map(|(i, _)| i)
            .collect();

        if let Some(&last_idx) = group_tab_indices.last() {
            if tab_index > last_idx + 1 {
                let view = self.views.remove(tab_index);
                let target = last_idx + 1;
                self.views.insert(target.min(self.views.len()), view);
            } else if tab_index < group_tab_indices[0] {
                let view = self.views.remove(tab_index);
                let target = group_tab_indices[0];
                self.views.insert(target.min(self.views.len()), view);
            }
        } else if was_pinned {
            // Destination group had no other members. Move the newly unpinned tab after all remaining pinned tabs in this pane.
            let after_pinned = self
                .views
                .iter()
                .enumerate()
                .filter(|(i, v)| *i != tab_index && v.pane.place == place && v.pinned)
                .map(|(i, _)| if i > tab_index { i } else { i + 1 })
                .max()
                .unwrap_or(0);
            if tab_index < after_pinned {
                let view = self.views.remove(tab_index);
                self.views.insert(after_pinned.min(self.views.len()), view);
            }
        }

        self.active = active.and_then(|k| self.tab_with_key(k));

        if let Some(og) = prev_group
            && og != group_id
        {
            self.cleanup_empty_group(og);
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

    /// Close tabs identified by `keys`: clean tabs go through `request_close_tab`, and
    /// dirty tabs are queued in strip order for `begin_close`.
    fn close_tab_keys(&mut self, ctx: &egui::Context, keys: Vec<u64>) {
        for &key in keys.iter().rev() {
            let Some(i) = self.tab_with_key(key) else { continue };
            if !self.asks_before_closing(i) {
                self.request_close_tab(ctx, i);
            }
        }
        let mut dirty = Vec::new();
        for key in keys {
            let Some(i) = self.tab_with_key(key) else { continue };
            if self.hold_close_for_ocr_review(i) {
                continue;
            }
            if self.asks_before_closing(i)
                && let Some(v) = self.views.get(i)
                && !dirty.contains(&v.id)
            {
                dirty.push(v.id);
            }
        }
        if !dirty.is_empty() {
            self.begin_close(ctx, craft::quit::CloseScope::Close, dirty);
        }
    }

    /// Close all tabs in a group.
    pub fn close_tab_group(&mut self, ctx: &egui::Context, group_id: TabGroupId) {
        let keys: Vec<u64> = self.views.iter().filter(|v| v.group == Some(group_id)).map(|v| v.pane.key).collect();
        self.close_tab_keys(ctx, keys);
        self.cleanup_empty_group(group_id);
    }

    /// Toggle collapsed state of a group.
    pub fn toggle_group_collapsed(&mut self, group_id: TabGroupId) {
        if let Some(g) = self.group_mut(group_id) {
            g.collapsed = !g.collapsed;
        }
    }

    /// Pin or unpin a tab. Pinned tabs sit at the start of their strip.
    pub fn set_tab_pinned(&mut self, tab_index: usize, pinned: bool) {
        if tab_index >= self.views.len() {
            return;
        }
        let active = self.active_key();
        let place = self.views[tab_index].pane.place;
        self.views[tab_index].pinned = pinned;
        if pinned {
            // Pinned tabs cannot be in a group
            let prev_group = self.views[tab_index].group.take();
            if let Some(gid) = prev_group {
                self.cleanup_empty_group(gid);
            }
            // Move to end of pinned tabs in this pane
            let target = self
                .views
                .iter()
                .enumerate()
                .filter(|(i, v)| *i < tab_index && v.pane.place == place && v.pinned)
                .map(|(i, _)| i + 1)
                .max()
                .or_else(|| self.tabs_in(place).first().copied())
                .unwrap_or(0);
            if target < tab_index {
                let view = self.views.remove(tab_index);
                self.views.insert(target.min(self.views.len()), view);
            }
        } else {
            // Move after all pinned tabs in this pane
            let after_pinned = self
                .views
                .iter()
                .enumerate()
                .filter(|(i, v)| *i != tab_index && v.pane.place == place && v.pinned)
                .map(|(i, _)| if i > tab_index { i } else { i + 1 })
                .max()
                .unwrap_or(0);
            if tab_index < after_pinned {
                let view = self.views.remove(tab_index);
                self.views.insert(after_pinned.min(self.views.len()), view);
            }
        }
        self.active = active.and_then(|k| self.tab_with_key(k));
    }

    /// Duplicate a tab (creates a second twin view of the document in the same pane).
    pub fn duplicate_tab(&mut self, tab_index: usize) {
        let Some(view) = self.views.get(tab_index) else { return };
        let id = view.id;
        let group = view.group;
        let is_pinned = view.pinned;
        let place = view.pane.place;
        let Some(doc) = self.session.get(id) else { return };
        let mut twin = view.twin(&doc.info);
        twin.pane.place = place;
        twin.pane.placed = true;
        twin.group = group;
        twin.pinned = false;
        let at = if is_pinned {
            // Unpinned duplicate sits at the start of the unpinned section in this pane, preserving the pinned section
            self.views.iter().enumerate().filter(|(_, v)| v.pane.place == place && v.pinned).map(|(i, _)| i + 1).max().unwrap_or(0)
        } else {
            (tab_index + 1).min(self.views.len())
        };
        self.views.insert(at, twin);
        self.active = Some(at);
    }

    /// Close all unpinned tabs in the same pane except `keep_index`.
    pub fn close_other_tabs(&mut self, ctx: &egui::Context, keep_index: usize) {
        let Some(view) = self.views.get(keep_index) else { return };
        let keep_key = view.pane.key;
        let place = view.pane.place;
        let keys: Vec<u64> = self
            .tabs_in(place)
            .into_iter()
            .filter_map(|i| self.views.get(i))
            .filter(|v| v.pane.key != keep_key && !v.pinned)
            .map(|v| v.pane.key)
            .collect();
        self.close_tab_keys(ctx, keys);
    }

    /// Close all unpinned tabs to the right of `from_index` in its pane's strip order.
    pub fn close_tabs_to_right(&mut self, ctx: &egui::Context, from_index: usize) {
        let Some(place) = self.views.get(from_index).map(|v| v.pane.place) else { return };
        let strip_tabs = self.tabs_in(place);
        let Some(pos) = strip_tabs.iter().position(|&i| i == from_index) else { return };
        let keys: Vec<u64> = strip_tabs[pos + 1..].iter().filter_map(|&i| self.views.get(i)).filter(|v| !v.pinned).map(|v| v.pane.key).collect();
        self.close_tab_keys(ctx, keys);
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

/// Paint an Edge-styled group header pill (dot, title, and collapse/expand chevron) over `rect`.
pub fn paint_group_pill(ui: &egui::Ui, group: &TabGroup, rect: Rect, hovered: bool) {
    let font = egui::FontId::proportional(11.5);
    let title = group.title.trim();
    let label = if title.is_empty() { tl!("Tab group") } else { title };
    let text_color = group.color.text_color();
    let bg_color = group.color.color32();
    let fill = if hovered { bg_color.lerp_to_gamma(Color32::WHITE, 0.15) } else { bg_color };

    let pill_rect = rect.shrink2(vec2(2.0, 2.0));
    ui.painter().rect_filled(pill_rect, CornerRadius::same(5), fill);

    let dot_center = pos2(pill_rect.left() + 10.0, pill_rect.center().y);
    ui.painter().circle_filled(dot_center, 3.0, text_color);

    let chevron_center = pos2(pill_rect.right() - 10.0, pill_rect.center().y);
    let chevron_stroke = Stroke::new(1.5, text_color);
    let open = group.open_progress.clamp(0.0, 1.0);
    if group.collapsed && open < 0.5 {
        // Right-pointing chevron (collapsed)
        ui.painter().line_segment([chevron_center + vec2(-2.0, -3.5), chevron_center + vec2(2.0, 0.0)], chevron_stroke);
        ui.painter().line_segment([chevron_center + vec2(2.0, 0.0), chevron_center + vec2(-2.0, 3.5)], chevron_stroke);
    } else {
        // Down-pointing chevron (expanded)
        ui.painter().line_segment([chevron_center + vec2(-3.5, -1.5), chevron_center + vec2(0.0, 2.0)], chevron_stroke);
        ui.painter().line_segment([chevron_center + vec2(0.0, 2.0), chevron_center + vec2(3.5, -1.5)], chevron_stroke);
    }

    let text_left = pill_rect.left() + 18.0;
    let text_right = (pill_rect.right() - 18.0).max(text_left);
    let text_clip = Rect::from_min_max(pos2(text_left, pill_rect.top()), pos2(text_right, pill_rect.bottom()));
    let galley = ui.painter().layout_no_wrap(label.to_string(), font, text_color);
    let text_pos = pos2(text_left, pill_rect.center().y - galley.size().y * 0.5);
    ui.painter().with_clip_rect(text_clip).galley(text_pos, galley, text_color);
}

/// Render the Group Edit Popover (name input, color swatches, new tab in group, ungroup, close group).
pub fn group_editor_popup(app: &mut PdfCraftApp, ui: &mut egui::Ui, place: crate::panes::Place, group_id: TabGroupId, close_popup: &mut bool) {
    let Some(group) = app.group(group_id).cloned() else {
        *close_popup = true;
        return;
    };

    ui.set_min_width(220.0);
    Stack::new().show(ui, |ui, _| {
        Text::new(tl!("Group name")).show(ui);
        let mut title_buf = app.group_title_draft.clone();
        let edit = TextField::new(tl!("Group name"), Bind::bound(&mut title_buf)).submits().show(ui);
        if title_buf != app.group_title_draft {
            app.group_title_draft = title_buf.clone();
            if let Some(g) = app.group_mut(group_id) {
                g.title = title_buf;
            }
        }
        if edit.submitted {
            *close_popup = true;
        }

        Text::new(tl!("Color")).show(ui);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            for &color in &TabGroupColor::ALL {
                let [r, g, b, _] = color.color32().to_srgba_unmultiplied();
                let srgba = craft::theme::Srgba::rgb(f32::from(r) / 255.0, f32::from(g) / 255.0, f32::from(b) / 255.0);
                let selected = group.color == color;
                if Swatch::new(tl!(color.name()), srgba).shape(SwatchShape::Round).cell().selected(selected).show(ui).clicked
                    && let Some(g) = app.group_mut(group_id)
                {
                    g.color = color;
                }
            }
        });

        ui.separator();

        if Button::new(tl!("New tab in group")).variant(Variant::GHOST).show(ui).clicked() {
            app.open_dialog_in_group(Some((group_id, place)));
            *close_popup = true;
        }

        if Button::new(tl!("Ungroup")).variant(Variant::GHOST).show(ui).clicked() {
            app.tab_action(ui.ctx(), crate::tab_context::Action::Ungroup(group_id));
            *close_popup = true;
        }

        if Button::new(tl!("Close group")).variant(Variant::GHOST).show(ui).clicked() {
            app.tab_action(ui.ctx(), crate::tab_context::Action::CloseGroup(group_id));
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
        let mut app = PdfCraftApp::for_unit_tests();
        let ctx = egui::Context::default();
        app.open_bytes(&ctx, "doc1.pdf", None, FIXTURE.to_vec()).unwrap();
        app.open_bytes(&ctx, "doc2.pdf", None, FIXTURE.to_vec()).unwrap();
        app.open_bytes(&ctx, "doc3.pdf", None, FIXTURE.to_vec()).unwrap();

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
        let mut app = PdfCraftApp::for_unit_tests();
        let ctx = egui::Context::default();
        app.open_bytes(&ctx, "doc1.pdf", None, FIXTURE.to_vec()).unwrap();
        app.open_bytes(&ctx, "doc2.pdf", None, FIXTURE.to_vec()).unwrap();
        app.open_bytes(&ctx, "doc3.pdf", None, FIXTURE.to_vec()).unwrap();

        app.set_tab_pinned(2, true);
        assert!(app.views[0].pinned);
        assert_eq!(app.session.get(app.views[0].id).unwrap().name, "doc3.pdf");

        app.set_tab_pinned(0, false);
        assert!(!app.views[0].pinned);
    }

    #[test]
    fn duplicate_and_close_tab_actions() {
        let mut app = PdfCraftApp::for_unit_tests();
        let ctx = egui::Context::default();
        app.open_bytes(&ctx, "doc1.pdf", None, FIXTURE.to_vec()).unwrap();
        app.open_bytes(&ctx, "doc2.pdf", None, FIXTURE.to_vec()).unwrap();
        app.open_bytes(&ctx, "doc3.pdf", None, FIXTURE.to_vec()).unwrap();

        app.duplicate_tab(1);
        assert_eq!(app.views.len(), 4);
        assert_eq!(app.views[1].id, app.views[2].id);

        app.close_tabs_to_right(&ctx, 0);
        assert_eq!(app.views.len(), 1);
        assert_eq!(app.session.get(app.views[0].id).unwrap().name, "doc1.pdf");
    }

    #[test]
    fn close_other_tabs_action() {
        let mut app = PdfCraftApp::for_unit_tests();
        let ctx = egui::Context::default();
        app.open_bytes(&ctx, "doc1.pdf", None, FIXTURE.to_vec()).unwrap();
        app.open_bytes(&ctx, "doc2.pdf", None, FIXTURE.to_vec()).unwrap();
        app.open_bytes(&ctx, "doc3.pdf", None, FIXTURE.to_vec()).unwrap();

        app.close_other_tabs(&ctx, 1);
        assert_eq!(app.views.len(), 1);
        assert_eq!(app.session.get(app.views[0].id).unwrap().name, "doc2.pdf");
    }

    #[test]
    fn close_tab_group_action() {
        let mut app = PdfCraftApp::for_unit_tests();
        let ctx = egui::Context::default();
        app.open_bytes(&ctx, "doc1.pdf", None, FIXTURE.to_vec()).unwrap();
        app.open_bytes(&ctx, "doc2.pdf", None, FIXTURE.to_vec()).unwrap();
        app.open_bytes(&ctx, "doc3.pdf", None, FIXTURE.to_vec()).unwrap();

        let gid = app.create_tab_group("Work".into(), TabGroupColor::Green);
        app.add_tab_to_group(0, gid);
        app.add_tab_to_group(1, gid);

        app.close_tab_group(&ctx, gid);
        assert_eq!(app.tab_groups.len(), 0);
        assert_eq!(app.views.len(), 1);
        assert_eq!(app.session.get(app.views[0].id).unwrap().name, "doc3.pdf");
    }

    #[test]
    fn add_pinned_tab_to_empty_group_moves_past_pinned_tabs() {
        let mut app = PdfCraftApp::for_unit_tests();
        let ctx = egui::Context::default();
        app.open_bytes(&ctx, "doc1.pdf", None, FIXTURE.to_vec()).unwrap();
        app.open_bytes(&ctx, "doc2.pdf", None, FIXTURE.to_vec()).unwrap();
        app.open_bytes(&ctx, "doc3.pdf", None, FIXTURE.to_vec()).unwrap();

        // Pin doc1 and doc2
        app.set_tab_pinned(0, true);
        app.set_tab_pinned(1, true);
        assert!(app.views[0].pinned);
        assert!(app.views[1].pinned);

        // Add doc1 (index 0) to a new empty group
        let gid = app.create_tab_group("Test".into(), TabGroupColor::Blue);
        app.add_tab_to_group(0, gid);

        // doc1 is now unpinned and in the group, and must be after remaining pinned doc2
        assert!(!app.views[1].pinned);
        assert_eq!(app.views[1].group, Some(gid));
        assert!(app.views[0].pinned);
        assert_eq!(app.session.get(app.views[0].id).unwrap().name, "doc2.pdf");
        assert_eq!(app.session.get(app.views[1].id).unwrap().name, "doc1.pdf");
    }

    #[test]
    fn dragging_a_member_to_the_edge_of_another_group_ungroups_it() {
        let mut app = PdfCraftApp::for_unit_tests();
        let ctx = egui::Context::default();
        for name in ["a1.pdf", "a2.pdf", "b1.pdf", "b2.pdf"] {
            app.open_bytes(&ctx, name, None, FIXTURE.to_vec()).unwrap();
        }
        let a = app.create_tab_group("A".into(), TabGroupColor::Blue);
        let b = app.create_tab_group("B".into(), TabGroupColor::Green);
        app.add_tab_to_group(0, a);
        app.add_tab_to_group(1, a);
        app.add_tab_to_group(2, b);
        app.add_tab_to_group(3, b);

        // Drag a1.pdf behind b2.pdf: it sits after group B's last member, so it joins neither
        // group, and group A keeps a2.pdf.
        let tabs = app.tabs_in(crate::panes::Place::Main);
        app.move_tab_in_strip(0, &tabs, 3);

        let names: Vec<String> = app.views.iter().map(|v| app.session.get(v.id).unwrap().name.clone()).collect();
        assert_eq!(names, ["a2.pdf", "b1.pdf", "b2.pdf", "a1.pdf"]);
        assert_eq!(app.views[3].group, None);
        assert_eq!(app.views[0].group, Some(a));
        assert_eq!(app.views[2].group, Some(b));
    }

    #[test]
    fn moving_last_member_to_another_group_cleans_up_previous_group() {
        let mut app = PdfCraftApp::for_unit_tests();
        let ctx = egui::Context::default();
        app.open_bytes(&ctx, "doc1.pdf", None, FIXTURE.to_vec()).unwrap();
        app.open_bytes(&ctx, "doc2.pdf", None, FIXTURE.to_vec()).unwrap();

        let g1 = app.create_tab_group("G1".into(), TabGroupColor::Blue);
        let g2 = app.create_tab_group("G2".into(), TabGroupColor::Green);
        app.add_tab_to_group(0, g1);
        app.add_tab_to_group(1, g2);

        assert_eq!(app.tab_groups.len(), 2);
        // Move doc1 from g1 to g2
        app.add_tab_to_group(0, g2);

        // g1 is now empty and should be cleaned up
        assert_eq!(app.tab_groups.len(), 1);
        assert_eq!(app.tab_groups[0].id, g2);
    }

    #[test]
    fn multi_dirty_bulk_close_with_close_guard() {
        let mut app = PdfCraftApp::for_unit_tests();
        let ctx = egui::Context::default();
        let bytes = app.session.create_blank(200.0, 300.0, 1).unwrap();
        app.open_bytes(&ctx, "doc1.pdf", None, bytes.to_vec()).unwrap();
        app.open_bytes(&ctx, "doc2.pdf", None, bytes.to_vec()).unwrap();
        app.open_bytes(&ctx, "doc3.pdf", None, bytes.to_vec()).unwrap();

        // Mark doc2 and doc3 dirty by applying edits
        app.active = Some(1);
        assert!(app.apply_edit(&ctx, pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }));
        app.active = Some(2);
        assert!(app.apply_edit(&ctx, pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }));
        app.active = Some(0);

        app.close_tabs_to_right(&ctx, 0);

        // Dirty tabs held by close_guard
        assert!(app.close_guard.is_some());
        assert_eq!(app.views.len(), 3);
    }

    #[test]
    fn close_other_and_to_right_respect_pane_strip_order_and_keep_pinned() {
        let mut app = PdfCraftApp::for_unit_tests();
        let ctx = egui::Context::default();
        app.open_bytes(&ctx, "pinned.pdf", Some("/tmp/pinned.pdf".into()), FIXTURE.to_vec()).unwrap();
        app.open_bytes(&ctx, "main_a.pdf", Some("/tmp/main_a.pdf".into()), FIXTURE.to_vec()).unwrap();
        app.open_bytes(&ctx, "main_b.pdf", Some("/tmp/main_b.pdf".into()), FIXTURE.to_vec()).unwrap();
        app.open_bytes(&ctx, "second.pdf", Some("/tmp/second.pdf".into()), FIXTURE.to_vec()).unwrap();
        app.open_bytes(&ctx, "main_c.pdf", Some("/tmp/main_c.pdf".into()), FIXTURE.to_vec()).unwrap();

        app.set_tab_pinned(0, true);
        // Move second.pdf (index 3) to the second pane, leaving Main tabs interleaved around it in `views`.
        app.move_tab_to(3, crate::panes::Place::Second);
        assert_eq!(app.views[3].place(), crate::panes::Place::Second);
        assert_eq!(app.views[4].place(), crate::panes::Place::Main);

        // Close tabs to the right of main_a.pdf (index 1) in Place::Main.
        app.close_tabs_to_right(&ctx, 1);
        let remaining: Vec<String> = app.views.iter().map(|v| app.session.get(v.id).unwrap().name.clone()).collect();
        assert_eq!(remaining, ["pinned.pdf", "main_a.pdf", "second.pdf"]);

        // Reopen another main tab and run close_other_tabs on it: pinned.pdf and second.pdf stay open.
        app.open_bytes(&ctx, "main_d.pdf", None, FIXTURE.to_vec()).unwrap();
        let keep_idx = app.views.len() - 1;
        app.close_other_tabs(&ctx, keep_idx);
        let remaining: Vec<String> = app.views.iter().map(|v| app.session.get(v.id).unwrap().name.clone()).collect();
        assert_eq!(remaining, ["pinned.pdf", "second.pdf", "main_d.pdf"]);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn open_path_in_group_adds_opened_tab_to_group() {
        use std::io::Write as _;
        use std::sync::Arc;
        let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir().join(format!("pdfcraft-group-open-{}-{nonce}.pdf", std::process::id()));
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&path).unwrap();
        file.write_all(FIXTURE).unwrap();
        drop(file);

        let jobs = Arc::new(craft::host::ManualJobs::new());
        let host = craft::host::Host::builder(crate::APP_INFO).jobs(jobs.clone()).build();
        let mut app = PdfCraftApp::for_unit_tests_on(Arc::new(host));
        let ctx = egui::Context::default();
        app.open_bytes(&ctx, "first.pdf", None, FIXTURE.to_vec()).unwrap();
        let gid = app.create_tab_group("Bundle".into(), TabGroupColor::Purple);
        app.add_tab_to_group(0, gid);

        app.open_path_in_group(path.to_str().unwrap(), Some((gid, crate::panes::Place::Main)));
        jobs.run_all();
        app.poll_pending_opens(&ctx);

        assert_eq!(app.views.len(), 2);
        assert_eq!(app.views[1].group, Some(gid));
        std::fs::remove_file(path).unwrap();
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn open_path_in_group_in_second_pane_places_tab_next_to_group() {
        use std::io::Write as _;
        use std::sync::Arc;
        let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let filename = format!("pdfcraft-split-group-open-{}-{nonce}.pdf", std::process::id());
        let path = std::env::temp_dir().join(&filename);
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&path).unwrap();
        file.write_all(FIXTURE).unwrap();
        drop(file);

        let jobs = Arc::new(craft::host::ManualJobs::new());
        let host = craft::host::Host::builder(crate::APP_INFO).jobs(jobs.clone()).build();
        let mut app = PdfCraftApp::for_unit_tests_on(Arc::new(host));
        let ctx = egui::Context::default();
        app.open_bytes(&ctx, "main.pdf", None, FIXTURE.to_vec()).unwrap();
        app.open_bytes(&ctx, "grouped.pdf", None, FIXTURE.to_vec()).unwrap();
        app.open_bytes(&ctx, "unrelated.pdf", None, FIXTURE.to_vec()).unwrap();
        app.settle_panes(&ctx);

        // Put grouped.pdf and unrelated.pdf in the second (right) pane, with grouped.pdf before unrelated.pdf.
        app.move_tab_to(1, crate::panes::Place::Second);
        app.move_tab_to(2, crate::panes::Place::Second);
        let gid = app.create_tab_group("RightBundle".into(), TabGroupColor::Blue);
        app.add_tab_to_group(1, gid);
        app.active = Some(1);
        app.settle_panes(&ctx);

        app.open_path_in_group(path.to_str().unwrap(), Some((gid, crate::panes::Place::Second)));
        jobs.run_all();
        app.poll_pending_opens(&ctx);
        app.settle_panes(&ctx);

        let second_names: Vec<String> =
            app.tabs_in(crate::panes::Place::Second).into_iter().map(|i| app.session.get(app.views[i].id).unwrap().name.clone()).collect();
        let main_names: Vec<String> =
            app.tabs_in(crate::panes::Place::Main).into_iter().map(|i| app.session.get(app.views[i].id).unwrap().name.clone()).collect();
        assert_eq!(main_names, ["main.pdf"]);
        assert_eq!(second_names, ["grouped.pdf", filename.as_str(), "unrelated.pdf"]);
        let new_idx = app.tabs_in(crate::panes::Place::Second)[1];
        assert_eq!(app.views[new_idx].group, Some(gid));
        assert_eq!(app.views[new_idx].place(), crate::panes::Place::Second);

        std::fs::remove_file(path).unwrap();
    }
}
