use craft::behavior::kurbo::{Point, Rect};
use craft::pages::Fit;
use craft::pages::ui::view::GoTo;

use super::selection::DocTexts;
use super::{DestView, DocInfo, DocView, pos2};

/// Screen points a page edge may show above the window's top and still be a rounding sliver, not
/// what the reader is looking at.
const SLIVER: f32 = 1.0;

/// A page-space point from page fractions and the displayed page size in points, with a missing
/// coordinate as NaN, which the page view reads as "keep that axis's offset"
/// (`PageViewState::go_to_point`).
fn point_of([fx, fy]: [Option<f32>; 2], (dw, dh): (f32, f32)) -> Point {
    Point::new(fx.map_or(f64::NAN, |f| f64::from(f * dw)), fy.map_or(f64::NAN, |f| f64::from(f * dh)))
}

/// The page's size in points as the view shows it: the view's quarter turns swap its sides.
fn displayed(p: &pdfcraft_render::PageInfo, rotation: u16) -> (f32, f32) {
    if rotation % 180 == 90 { (p.height, p.width) } else { (p.width, p.height) }
}

impl DocView {
    /// Where a bookmark made now should go: the page and the `/XYZ` position `[left, top]` in user
    /// space. With text selected it is the corner of the selection that shows at its top-left
    /// (its page, both coordinates); otherwise the top of the window (the page showing there, and
    /// only the position along the window's vertical, so the reader's other scroll position and
    /// zoom are kept). `None` before the first frame has laid pages out, when the bookmark just
    /// goes to the current page.
    pub(crate) fn bookmark_position(&self, info: &DocInfo) -> Option<(usize, [Option<f32>; 2])> {
        // The page grid moves the viewport without refreshing where the pages are on screen, so
        // there is no position to read: the bookmark goes to the current page.
        if self.organize {
            return None;
        }
        // Across pages, the corner is on the selection's first page.
        // The page's text at view rotation 0: the bounds below are in view space, which View ▸ Rotate View leaves unturned.
        let upright = DocTexts::new(&self.session.texts, &self.session.text_failed, 0, &self.page_sizes);
        if let Some(sel) = self.session.text.selection
            && let first = *sel.pages().start()
            && let Some(source) = upright.app(first)
            && let Some(p) = info.pages.get(first)
        {
            // The selection's bounds in view space, which View ▸ Rotate View leaves unturned: the
            // corner at the screen's top-left is a different one for each quarter turn.
            let [x0, y0, x1, y1] =
                sel.rects_on(first, source).iter().fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |b, r| {
                    [b[0].min(r.x0), b[1].min(r.y0), b[2].max(r.x1), b[3].max(r.y1)]
                });
            let (x, y) = match self.view.rotation {
                90 => (x0, y1),
                180 => (x1, y1),
                270 => (x1, y0),
                _ => (x0, y0),
            };
            if x.is_finite() && y.is_finite() {
                return Some((first, p.xyz_for_view(Some(x as f32), Some(y as f32))));
            }
        }
        // The page showing at the window's top edge: one that crosses it (the first, in a spread
        // of pages of different heights, whose shorter page can start below it), else the
        // nearest one below.
        let top = self.viewport_rect().top();
        let xforms = self.page_xforms();
        // A page showing less than a sliver is rounding, not what the reader is looking at: a jump
        // to the next page leaves the previous one's bottom edge within a pixel of the top (or,
        // when `PageView`'s top margin exceeds its page gap, peeking into that top margin). Such a
        // sliver still counts when nothing else shows (the last page at the very end).
        let peek = self.spacing.as_ref().map_or(0.0, |s| (s.margin - s.gap).max(0.0) as f32);
        let pick = |slack: f32| {
            let showing = || xforms.iter().filter(move |(_, xf)| xf.rect.bottom() > top + slack);
            showing()
                .filter(|(_, xf)| xf.rect.top() <= top)
                .min_by_key(|(p, _)| *p)
                .or_else(|| showing().min_by(|a, b| a.1.rect.top().total_cmp(&b.1.rect.top()).then(a.0.cmp(&b.0))))
        };
        let (page, xf) = pick(peek + SLIVER).or_else(|| pick(SLIVER)).or_else(|| pick(0.0))?;
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

    /// Records `from` as the place the reader leaves, for Previous view / Next view.
    fn remember_place(&mut self, from: usize) {
        if self.back.last() != Some(&from) {
            self.back.push(from);
            if self.back.len() > 200 {
                self.back.remove(0);
            }
        }
        self.forward.clear();
    }

    pub fn go_to_page(&mut self, page: usize) {
        let page = page.min(self.page_count.saturating_sub(1));
        self.end_find_following();
        if page != self.view.current {
            self.remember_place(self.view.current);
        }
        self.view.go_to(page);
    }

    /// The end of the document (End): the last page, scrolled to its bottom as far as the view
    /// goes, as browsers and word processors do (#6434). [`go_to_page`](Self::go_to_page) shows a
    /// page's top.
    pub fn go_to_end(&mut self) {
        let last = self.page_count.saturating_sub(1);
        self.go_to_page(last);
        self.view.go_to_fraction(last, 1.0);
    }

    /// Reveal a user-space rectangle without changing zoom, centred where the scroll bounds allow.
    pub(crate) fn reveal_rect(&mut self, page: usize, rect: [f32; 4], info: &DocInfo) {
        let page = page.min(self.page_count.saturating_sub(1));
        self.go_to_page(page);
        let Some(p) = info.pages.get(page) else { return };
        if !rect.iter().all(|v| v.is_finite()) {
            return;
        }
        // Halve before adding so even large finite coordinates cannot overflow the midpoint.
        let x = rect[0] * 0.5 + rect[2] * 0.5;
        let y = rect[1] * 0.5 + rect[3] * 0.5;
        let rotation = self.view.rotation;
        let at = point_of(p.dest_fraction(Some(x), Some(y), rotation), displayed(p, rotation));
        self.view.go_to_point(page, at, (0.5, 0.5));
    }

    /// Go to a destination, a bookmark's or a link's (ISO 32000-2 §12.3.2.2): its page, scrolled
    /// and zoomed as `dest` asks, as Acrobat does. `/XYZ` puts (left, top) at the window's
    /// top-left at its zoom (null or 0 keeps the zoom); `/Fit`, `/FitH` and `/FitV` switch to fit
    /// page, width and height with `top` or `left` at the window's edge; `/FitR` zooms so the
    /// rectangle fits, centred. The `B` (content box) forms are read as the page. A null `/XYZ`
    /// coordinate keeps the reader's place in user space, as Acrobat does: the coordinate at the
    /// window's top-left now is the one put there on the new page. A point past the end of the
    /// document scrolls as far as it can.
    pub fn go_to_dest(&mut self, page: usize, dest: DestView, info: &DocInfo) {
        let kept = match dest {
            DestView::Xyz { left, top, .. } if left.is_none() || top.is_none() => self.place_at_corner(info),
            _ => None,
        };
        let dest = match (dest, kept) {
            (DestView::Xyz { left, top, zoom }, Some([x, y])) => DestView::Xyz { left: left.or(Some(x)), top: top.or(Some(y)), zoom },
            (dest, _) => dest,
        };
        let page = page.min(self.page_count.saturating_sub(1));
        self.go_to_page(page);
        let Some(p) = info.pages.get(page) else { return };
        let rotation = self.view.rotation;
        let size = displayed(p, rotation);
        let fractions = |x: Option<f32>, y: Option<f32>| p.dest_fraction(x, y, rotation);
        let at = match dest {
            DestView::Top => return,
            DestView::Xyz { left, top, zoom } => {
                if let Some(z) = zoom {
                    // The page view's pass clamps the written zoom (`ZOOM_RANGE`).
                    self.view.zoom = f64::from(z);
                    self.view.fit = Fit::None;
                }
                fractions(left, top)
            }
            DestView::Fit => {
                self.view.fit = Fit::Page;
                return;
            }
            DestView::FitH { top } => {
                self.view.fit = Fit::Width;
                fractions(None, top)
            }
            DestView::FitV { left } => {
                self.view.fit = Fit::Height;
                fractions(left, None)
            }
            DestView::FitR { rect } => {
                let ([Some(ax), Some(ay)], [Some(bx), Some(by)]) = (fractions(Some(rect[0]), Some(rect[3])), fractions(Some(rect[2]), Some(rect[1])))
                else {
                    return;
                };
                // The rectangle as displayed, in page points (it can be turned a quarter).
                let (dw, dh) = size;
                let (w, h) = ((ax - bx).abs() * dw, (ay - by).abs() * dh);
                if w >= 1.0 && h >= 1.0 {
                    let rect =
                        Rect::new(f64::from(ax.min(bx) * dw), f64::from(ay.min(by) * dh), f64::from(ax.max(bx) * dw), f64::from(ay.max(by) * dh));
                    self.view.go_to = Some(GoTo::Rect { page, rect, margin: 0.0 });
                }
                return;
            }
        };
        if at.iter().any(Option::is_some) {
            // The pass places the point at the viewport's start corner beside the page gutter.
            self.view.go_to = Some(GoTo::Corner { page, at: point_of(at, size) });
        }
    }

    /// The user-space point of the current page at the window's top-left as far as pages go (past
    /// the side gutter) in the last pass, if that page was drawn.
    fn place_at_corner(&self, info: &DocInfo) -> Option<[f32; 2]> {
        let page = self.view.current;
        let xf = self.page_xform(page)?;
        let side = self.spacing.as_ref().map_or(0.0, |spacing| spacing.side as f32);
        let (vx, vy) = xf.screen_to_view(self.viewport_rect().min + egui::vec2(side, 0.0));
        let u = info.pages.get(page)?.view_to_user(vx, vy);
        Some([u[0], u[1]])
    }

    /// Whether the window's Escape closes the find bar: a search on the page, outside full screen
    /// one shown in the Search panel too. Full screen hides the panel and keeps its search for when
    /// the panel is back, so that Escape leaves full screen instead (#5302).
    pub fn escape_closes_find(&self) -> bool {
        self.find.as_ref().is_some_and(|f| !(f.in_panel && self.presenting()))
    }

    /// Next page (`true`) or previous page. In two-page view this moves a whole spread: the other
    /// page of the spread is already on screen, so stepping to it wouldn't move the view (#70).
    pub fn step_page(&mut self, forward: bool) {
        let before = self.view.current;
        // The page view turns in its next pass, so nothing here can see whether the step moved;
        // asking where there is no page to turn to is not a step.
        let can_turn = if forward { before.saturating_add(1) < self.page_count } else { before > 0 };
        if !can_turn {
            return;
        }
        self.end_find_following();
        self.remember_place(before);
        self.view.step(forward);
    }

    /// Page-editor focus and drafts belong to a document, even while another tab is active.
    pub(crate) fn page_input_id(&self) -> egui::Id {
        egui::Id::unique(("page-input", self.id.0))
    }

    /// The idle page box and unchanged-input lookup share the same label fallback.
    pub(crate) fn current_page_label(&self, label: Option<&str>) -> String {
        label.filter(|l| !l.is_empty()).map(str::to_owned).unwrap_or_else(|| (self.view.current + 1).to_string())
    }

    /// Retain the current page when `typed` exactly matches its displayed label, even if
    /// another page has that label. Otherwise resolve a trimmed label (case-insensitive),
    /// then a position in parentheses ("(3)", as print ranges take it), then a physical page
    /// number. `false` when it names no page.
    pub fn go_to_typed(&mut self, typed: &str, labels: &[String]) -> bool {
        // Compare the displayed value before trimming: a label may itself contain spaces.
        // Repeated labels occur in restarted sections and prefix-only ranges.
        let current = self.view.current;
        if current < self.page_count && self.current_page_label(labels.get(current).map(String::as_str)) == typed {
            return true;
        }
        let t = typed.trim();
        let page = labels
            .iter()
            .position(|l| !l.is_empty() && l.eq_ignore_ascii_case(t))
            .or_else(|| pdfcraft_engine::print::position(t).or_else(|| t.parse::<usize>().ok()).filter(|n| *n >= 1).map(|n| n - 1));
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
        to.push(self.view.current);
        let page = page.min(self.page_count.saturating_sub(1));
        self.view.go_to(page);
        true
    }
}
