use super::zoom::GRID_ZOOM_STEP;
use super::{DestView, DocInfo, DocView, Fit, PT, PageLayout, SIDE, ViewAction, pos2, vec2};

impl DocView {
    /// Where a bookmark made now should go: the page and the `/XYZ` position `[left, top]` in user
    /// space. With text selected it is the corner of the selection that shows at its top-left
    /// (its page, both coordinates); otherwise the top of the window (the page showing there, and
    /// only the position along the window's vertical, so the reader's other scroll position and
    /// zoom are kept). `None` before the first frame has laid pages out, when the bookmark just
    /// goes to the current page.
    pub(crate) fn bookmark_position(&self, info: &DocInfo) -> Option<(usize, [Option<f32>; 2])> {
        if let Some(sel) = self.selection
            && let Some(text) = self.texts.get(&sel.page)
            && let Some(p) = info.pages.get(sel.page)
        {
            // The selection's bounds in view space, which View ▸ Rotate View leaves unturned: the
            // corner at the screen's top-left is a different one for each quarter turn.
            let [x0, y0, x1, y1] = sel.rects(text).iter().fold([f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY], |b, r| {
                [b[0].min(r[0]), b[1].min(r[1]), b[2].max(r[2]), b[3].max(r[3])]
            });
            let (x, y) = match self.rotation {
                90 => (x0, y1),
                180 => (x1, y1),
                270 => (x1, y0),
                _ => (x0, y0),
            };
            if x.is_finite() && y.is_finite() {
                return Some((sel.page, p.xyz_for_view(Some(x), Some(y))));
            }
        }
        // The page showing at the window's top edge: one that crosses it (the first, in a spread
        // of pages of different heights, whose shorter page can start below it), else the
        // nearest one below.
        let top = self.viewport_screen.top();
        let showing = || self.screen_xforms.iter().filter(|(_, xf)| xf.rect.bottom() > top);
        let (page, xf) = showing()
            .filter(|(_, xf)| xf.rect.top() <= top)
            .min_by_key(|(p, _)| *p)
            .or_else(|| showing().min_by(|a, b| a.1.rect.top().total_cmp(&b.1.rect.top()).then(a.0.cmp(&b.0))))?;
        let p = info.pages.get(*page)?;
        // Above the page (in the gap before it) is the page's own top. The window's vertical is
        // the view's y, or its x when the view is turned a quarter.
        let (x, y) = xf.screen_to_view(pos2(xf.rect.center().x, top.max(xf.rect.top())));
        let at = if xf.rot.is_multiple_of(180) {
            p.xyz_for_view(None, Some(y.clamp(0.0, p.height)))
        } else {
            p.xyz_for_view(Some(x.clamp(0.0, p.width)), None)
        };
        Some((*page, at))
    }

    /// The user-space `[left, top]` at the window's top-left (where `/XYZ` puts its point), on the
    /// page showing there: what a destination's null coordinate keeps (ISO 32000-2 §12.3.2.2).
    /// Acrobat keeps the place this way across pages, so a CAD sheet's cross-reference opens the
    /// next sheet where the reader was looking. A coordinate is `None` along a screen axis the
    /// page fits, which keeps the layout's own placement (centred, or the page's top); the whole
    /// place is `None` before the first frame has laid pages out.
    fn place_at_corner(&self, info: &DocInfo) -> Option<[Option<f32>; 2]> {
        let area = self.viewport_screen;
        if !area.is_positive() {
            return None;
        }
        let corner = area.min + vec2(SIDE, 0.0);
        let (page, xf) = self
            .screen_xforms
            .iter()
            .filter(|(_, xf)| xf.rect.is_positive())
            .min_by(|a, b| a.1.rect.distance_sq_to_pos(corner).total_cmp(&b.1.rect.distance_sq_to_pos(corner)).then(a.0.cmp(&b.0)))?;
        let p = info.pages.get(*page)?;
        let (x, y) = xf.screen_to_view(corner.clamp(xf.rect.min, xf.rect.max));
        // `max` drops a NaN size, so the bounds stay ordered and `clamp` can't panic.
        let (x, y) = (x.clamp(0.0, p.width.max(0.0)), y.clamp(0.0, p.height.max(0.0)));
        // The page counts with its side gutters, as the layout places it.
        let wide = xf.rect.width() + 2.0 * SIDE > area.width();
        let tall = xf.rect.height() > area.height();
        // The view's x runs across the screen unless the view is turned a quarter.
        let (keep_x, keep_y) = if xf.rot.is_multiple_of(180) { (wide, tall) } else { (tall, wide) };
        Some(p.xyz_for_view(keep_x.then_some(x), keep_y.then_some(y)))
    }

    pub fn go_to_page(&mut self, page: usize) {
        let page = page.min(self.page_count.saturating_sub(1));
        self.end_find_following();
        if page != self.current {
            // The view history (Previous view / Next view).
            if self.back.last() != Some(&self.current) {
                self.back.push(self.current);
                if self.back.len() > 200 {
                    self.back.remove(0);
                }
            }
            self.forward.clear();
        }
        // Partial wheel motion belongs to the old place.
        self.reset_coarse_state();
        self.goto = Some((page, 0.0));
        self.current = page;
    }

    /// Reveal a user-space rectangle without changing zoom, centred where the scroll bounds allow.
    pub(crate) fn reveal_rect(&mut self, page: usize, rect: [f32; 4], info: &DocInfo) {
        self.go_to_page(page);
        self.goto_point = None;
        let Some(p) = info.pages.get(self.current) else { return };
        if !rect.iter().all(|v| v.is_finite()) {
            return;
        }
        // Halve before adding so even large finite coordinates cannot overflow the midpoint.
        let x = rect[0] * 0.5 + rect[2] * 0.5;
        let y = rect[1] * 0.5 + rect[3] * 0.5;
        let [fx, fy] = p.dest_fraction(Some(x), Some(y), self.rotation);
        self.goto_point = Some((self.current, fx, fy, vec2(self.viewport_w, self.viewport_h) / 2.0));
    }

    /// Go to a destination, a bookmark's or a link's (ISO 32000-2 §12.3.2.2): its page, scrolled
    /// and zoomed as `dest` asks, as Acrobat does. `/XYZ` puts (left, top) at the window's
    /// top-left at its zoom (null or 0 keeps the zoom); `/Fit`, `/FitH` and `/FitV` switch to fit
    /// page, width and height with `top` or `left` at the window's edge; `/FitR` zooms so the
    /// rectangle fits, centred. The `B` (content box) forms are read as the page. A null `/XYZ`
    /// coordinate keeps the reader's place: the same user-space point stays at the window's
    /// top-left on the new page, as Acrobat does. A point past the end of the document scrolls
    /// as far as it can.
    pub fn go_to_dest(&mut self, page: usize, dest: DestView, info: &DocInfo) {
        // Read before the jump: it is where the reader is now.
        let kept = match dest {
            DestView::Xyz { left, top, .. } if left.is_none() || top.is_none() => self.place_at_corner(info),
            _ => None,
        };
        self.go_to_page(page);
        let page = self.current;
        self.goto_point = None;
        let Some(p) = info.pages.get(page) else { return };
        let point = |x: Option<f32>, y: Option<f32>| p.dest_fraction(x, y, self.rotation);
        // The window's top-left as far as pages go: its left edge is the gutter that keeps them
        // clear of the floating quick-action bar, so `left` 0 shows the page's edge beside it.
        let corner = vec2(SIDE, 0.0);
        let (fx, fy, rel) = match dest {
            DestView::Top => return,
            DestView::Xyz { left, top, zoom } => {
                if let Some(z) = zoom {
                    self.zoom = z.clamp(0.08, 64.0);
                    self.fit = Fit::None;
                    self.zoom_anchor = None;
                }
                let [kept_left, kept_top] = kept.unwrap_or([None, None]);
                let [fx, fy] = point(left.or(kept_left), top.or(kept_top));
                (fx, fy, corner)
            }
            DestView::Fit => {
                self.fit = Fit::Page;
                return;
            }
            DestView::FitH { top } => {
                self.fit = Fit::Width;
                let [fx, fy] = point(None, top);
                (fx, fy, corner)
            }
            DestView::FitV { left } => {
                self.fit = Fit::Height;
                let [fx, fy] = point(left, None);
                (fx, fy, corner)
            }
            DestView::FitR { rect } => {
                let ([Some(ax), Some(ay)], [Some(bx), Some(by)]) = (point(Some(rect[0]), Some(rect[3])), point(Some(rect[2]), Some(rect[1]))) else {
                    return;
                };
                // The rectangle as displayed, in points (it can be turned a quarter).
                let (dw, dh) = self.display_size(p);
                let (w, h) = ((ax - bx).abs() * dw, (ay - by).abs() * dh);
                if w >= 1.0 && h >= 1.0 && self.viewport_w >= 1.0 && self.viewport_h >= 1.0 {
                    self.zoom = (self.viewport_w / (w * PT)).min(self.viewport_h / (h * PT)).clamp(0.08, 64.0);
                    self.fit = Fit::None;
                    self.zoom_anchor = None;
                }
                (Some((ax + bx) / 2.0), Some((ay + by) / 2.0), vec2(self.viewport_w, self.viewport_h) / 2.0)
            }
        };
        if fx.is_some() || fy.is_some() {
            self.goto_point = Some((page, fx, fy, rel));
        }
    }

    /// Next page (`true`) or previous page. In two-page view this moves a whole spread: the other
    /// page of the spread is already on screen, so stepping to it wouldn't move the view (#70).
    pub fn step_page(&mut self, forward: bool) {
        let c = self.current;
        let last = self.page_count.saturating_sub(1);
        let (target, first) = match self.layout {
            PageLayout::TwoUp => {
                let first = self.spread_first(c);
                let target = match (forward, self.cover && c == 0) {
                    (true, true) => 1,
                    (true, false) => first.saturating_add(2),
                    (false, _) if self.cover && first <= 1 => 0,
                    (false, _) => first.saturating_sub(2),
                };
                (target, first)
            }
            PageLayout::Single if forward => (c + 1, c),
            PageLayout::Single => (c.saturating_sub(1), c),
        };
        // Past either end there is nowhere to go: stay put rather than re-align the view, which
        // would scroll the page back to its top.
        if if forward { target > last } else { first == 0 } {
            return;
        }
        self.go_to_page(target);
    }

    /// Page-editor focus and drafts belong to a document, even while another tab is active.
    pub(crate) fn page_input_id(&self) -> egui::Id {
        egui::Id::unique(("page-input", self.id.0))
    }

    /// The idle page box and unchanged-input lookup share the same label fallback.
    pub(crate) fn current_page_label(&self, label: Option<&str>) -> String {
        label.filter(|l| !l.is_empty()).map(str::to_owned).unwrap_or_else(|| (self.current + 1).to_string())
    }

    /// Retain the current page when `typed` exactly matches its displayed label, even if
    /// another page has that label. Otherwise resolve a trimmed label (case-insensitive),
    /// then a physical page number. `false` when it names no page.
    pub fn go_to_typed(&mut self, typed: &str, labels: &[String]) -> bool {
        // Compare the displayed value before trimming: a label may itself contain spaces.
        // Repeated labels occur in restarted sections and prefix-only ranges.
        if self.current < self.page_count && self.current_page_label(labels.get(self.current).map(String::as_str)) == typed {
            return true;
        }
        let t = typed.trim();
        let page = labels
            .iter()
            .position(|l| !l.is_empty() && l.eq_ignore_ascii_case(t))
            .or_else(|| t.parse::<usize>().ok().filter(|n| *n >= 1).map(|n| n - 1));
        match page {
            Some(p) if p < self.page_count => {
                self.go_to_page(p);
                true
            }
            _ => false,
        }
    }

    /// View ▸ Page navigation ▸ Previous view (`false`) / Next view (`true`).
    pub fn view_history(&mut self, forward: bool) -> bool {
        let has_target = if forward { !self.forward.is_empty() } else { !self.back.is_empty() };
        if !has_target {
            return false;
        }
        self.end_find_following();
        let (from, to) = if forward { (&mut self.forward, &mut self.back) } else { (&mut self.back, &mut self.forward) };
        let Some(page) = from.pop() else { return false };
        to.push(self.current);
        let page = page.min(self.page_count.saturating_sub(1));
        self.goto = Some((page, 0.0));
        self.current = page;
        self.reset_coarse_state();
        true
    }

    /// One wheel event in a non-continuous view, where the wheel turns pages or spreads (the
    /// rules are in `wheel_pager`): `dy` is its vertical delta (negative scrolls down) and `now`
    /// is egui time. With `can_turn` false the gesture is followed but the page stays. Returns
    /// whether the page turned.
    pub fn single_page_wheel(&mut self, unit: egui::MouseWheelUnit, dy: f32, phase: egui::TouchPhase, now: f64, can_turn: bool) -> bool {
        if self.continuous {
            return false;
        }
        let Some(forward) = self.wheel.feed(unit, dy, phase, now, can_turn) else { return false };
        let before = self.current;
        self.step_page(forward);
        self.current != before
    }

    /// One wheel event in a non-continuous view whose page is taller than the window, where the
    /// wheel pans (#1924). Once the view is at the bottom (top) edge, scrolling on turns to the
    /// next (previous) page, which shows its top (bottom). A smooth gesture that panned the page
    /// has to end first (see `WheelPager::feed_edge`). `can_turn` is false when something else
    /// owns the wheel. Returns whether the page turned.
    pub fn single_page_wheel_edge(&mut self, unit: egui::MouseWheelUnit, dy: f32, phase: egui::TouchPhase, now: f64, can_turn: bool) -> bool {
        if self.continuous {
            return false;
        }
        // Only an up-to-date view counts: the page drawn is the shown one, with no jump pending.
        let edge = self.goto.is_none()
            && self.goto_point.is_none()
            && self.single_edges.is_some_and(|(page, top, bottom)| page == self.current && if dy < 0.0 { bottom } else { top });
        let Some(forward) = self.wheel.feed_edge(unit, dy, phase, now, can_turn && edge) else { return false };
        let before = self.current;
        self.step_page(forward);
        if self.current == before {
            return false;
        }
        if !forward {
            // Going back lands on the bottom of the previous page.
            self.goto = Some((self.current, 1.0));
        }
        self.single_edges = None;
        true
    }
}

/// ⌘+ / ⌘− / ⌘0 / ⌘1 / ⌘2: zoom the page, or size the pages in the page grid.
fn zoom_shortcuts(view: &mut DocView, pressed: &impl Fn(egui::KeyboardShortcut) -> bool) {
    use egui::{Key, KeyboardShortcut, Modifiers};
    let cmd = |k| KeyboardShortcut::new(Modifiers::COMMAND, k);
    // In the page grid ⌘+ / ⌘− / ⌘0 size its pages instead (looking is always allowed).
    let (zoom_in, zoom_out, zoom_reset) = (pressed(cmd(Key::Plus)) || pressed(cmd(Key::Equals)), pressed(cmd(Key::Minus)), pressed(cmd(Key::Num0)));
    if view.organize {
        if zoom_in {
            view.grid_zoom_request = Some(view.grid_zoom * GRID_ZOOM_STEP);
        }
        if zoom_out {
            view.grid_zoom_request = Some(view.grid_zoom / GRID_ZOOM_STEP);
        }
        if zoom_reset {
            view.grid_zoom_request = Some(1.0);
        }
    } else {
        if zoom_in {
            view.zoom_step(true);
        }
        if zoom_out {
            view.zoom_step(false);
        }
        if zoom_reset {
            view.fit = Fit::Page;
            view.goto = Some((view.current, 0.0));
        }
    }
    if pressed(cmd(Key::Num1)) {
        view.set_zoom(1.0);
    }
    if pressed(cmd(Key::Num2)) {
        view.fit = Fit::Width;
        view.goto = Some((view.current, 0.0));
    }
}

pub fn shortcuts(view: &mut DocView, ctx: &egui::Context, speed: crate::scroll_speed::ScrollSpeed) {
    use egui::{Key, KeyboardShortcut, Modifiers};
    if ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::F))) {
        view.open_find();
    }
    let cmd = |k| KeyboardShortcut::new(Modifiers::COMMAND, k);
    let pressed = |s: KeyboardShortcut| ctx.input_mut(|i| i.consume_shortcut(&s));
    // ⌘G / ⇧⌘G step through the matches from the find field too, as in Acrobat and Preview
    // (#2669): ahead of the text-input check, but only while there is a search to step.
    if view.find.is_some() {
        if pressed(KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::G)) {
            view.find_step(false);
        } else if pressed(cmd(Key::G)) {
            view.find_step(true);
        }
    }
    // A focused check box or radio button holds egui's keyboard focus only for Space, and a
    // radio button for the arrows that move within its group: the other page keys still work.
    let form_box = view.forms.keys.filter(|(id, _)| ctx.memory(|m| m.focused()) == Some(*id));
    if ctx.egui_wants_keyboard_input() && form_box.is_none() {
        return;
    }
    let arrows = !form_box.is_some_and(|(_, radio)| radio);
    // Rotation first: egui matches ⌘+ loosely with respect to Shift.
    if pressed(KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::Plus))
        || pressed(KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::Equals))
    {
        view.rotate_view(true);
    }
    if pressed(KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::Minus)) {
        view.rotate_view(false);
    }
    zoom_shortcuts(view, &pressed);
    // Alt+← / Alt+→ (⌥ on macOS) too, as in Acrobat and browsers, back to where a link was
    // followed from (#3632). Consumed here, before ← / → turn the page. egui matches the pattern
    // logically, so an extra Shift is ignored and Shift+Alt+← goes back as well (harmless).
    let alt = |k| KeyboardShortcut::new(Modifiers::ALT, k);
    if pressed(cmd(Key::OpenBracket)) || pressed(alt(Key::ArrowLeft)) {
        view.view_history(false);
    }
    if pressed(cmd(Key::CloseBracket)) || pressed(alt(Key::ArrowRight)) {
        view.view_history(true);
    }
    let key = |k| (arrows || !matches!(k, Key::ArrowLeft | Key::ArrowRight | Key::ArrowUp | Key::ArrowDown)) && ctx.input(|i| i.key_pressed(k));
    // An open menu takes Escape first and closes; the selection stays for the next one (#3535).
    if key(Key::Escape) && !egui::Popup::is_any_open(ctx) {
        if view.camera.is_some() || view.camera_done.is_some() {
            view.pending_action = Some(ViewAction::CancelCamera);
        } else if view.selection.is_some() || view.all_text {
            view.selection = None;
            view.all_text = false;
        } else {
            view.find = None;
        }
    }
    if key(Key::Home) {
        view.go_to_page(0);
    }
    if key(Key::End) {
        view.go_to_page(usize::MAX);
    }
    // Full screen: Space and Enter go forward a page and Backspace back, as in Acrobat (the
    // arrows and Page Up / Page Down already turn pages where nothing scrolls, below).
    if view.presentation.is_some() {
        if key(Key::Space) || key(Key::Enter) {
            view.step_page(true);
        }
        if key(Key::Backspace) {
            view.step_page(false);
        }
    }
    // As in Acrobat: → / ← go to the next / previous page in every layout (#185), and so do
    // ⌘Page Down / ⌘Page Up, or plain Page Down / Page Up in single-page view.
    let command = ctx.input(|i| i.modifiers.command);
    let scrolls = view.continuous;
    if key(Key::ArrowRight) || (key(Key::PageDown) && (command || !scrolls)) {
        view.step_page(true);
    }
    if key(Key::ArrowLeft) || (key(Key::PageUp) && (command || !scrolls)) {
        view.step_page(false);
    }
    // ↓ / ↑ scroll a line, and Page Down / Page Up a screen where the pages scroll. In
    // single-page view, ↓ / ↑ turn the page once there is nothing left to scroll that way, as
    // the wheel does (#273): always on a page that fits.
    // The Scroll speed preference sets how far ↓ / ↑ go as well as the wheel (#985); Page Down /
    // Page Up keep a screen.
    if !command {
        let line = KEY_SCROLL_LINE * speed.factor();
        let screen = (view.viewport_h - KEY_SCROLL_LINE).max(KEY_SCROLL_LINE);
        let steps = [(Key::ArrowDown, line), (Key::ArrowUp, -line), (Key::PageDown, screen), (Key::PageUp, -screen)];
        for (k, by) in steps {
            let arrow = matches!(k, Key::ArrowDown | Key::ArrowUp);
            if !key(k) || !(scrolls || arrow) {
                continue;
            }
            let forward = k == Key::ArrowDown;
            // Only an up-to-date view counts: the page drawn is the current one, with no jump pending.
            let at_end = !scrolls
                && view.goto.is_none()
                && view.single_edges.is_some_and(|(page, top, bottom)| page == view.current && if forward { bottom } else { top });
            if at_end {
                let before = view.current;
                view.step_page(forward);
                if !forward && view.current != before {
                    // Going back up lands on the bottom of the previous page.
                    view.goto = Some((view.current, 1.0));
                }
                view.single_edges = None;
            } else {
                view.key_scroll += by;
            }
        }
    }
}

/// How far ↓ / ↑ scroll at Normal speed (Scroll speed halves or doubles it). Page Down / Page Up
/// keep this much of the previous screen in view.
const KEY_SCROLL_LINE: f32 = 40.0;
