use craft::pages::ui::THUMBNAIL_ZOOM;
use craft::pages::{Fit, LayoutOptions, PageDisplay, shown_row};

use super::{DocInfo, DocView, PT, SavedDisplay, vec2};

impl DocView {
    /// Whether Select All of the document applies: it belongs to continuous scrolling, and the flag stays set
    /// while the view is paged.
    pub fn all_text_active(&self) -> bool {
        self.session.all_text && self.view.continuous
    }

    /// Full screen, as Acrobat's Full Screen Mode (#379, #1459): one page at a time (or one
    /// spread, when two-page view was on: #1353), fitted to the screen, from the current page.
    /// The display it replaces is kept for [`Self::leave_presentation`]. A no-op while presenting.
    pub fn enter_presentation(&mut self) {
        if self.presentation.is_some() {
            return;
        }
<<<<<<< HEAD
        self.presentation = Some(SavedDisplay {
            display: self.view.display,
            continuous: self.view.continuous,
            cover: self.view.cover,
            fit: self.view.fit,
            zoom: self.view.zoom,
        });
        self.view.continuous = false;
        self.view.fit = Fit::Page;
        self.session.text.selection = None;
        self.session.presenter = Default::default();
        self.view.go_to(self.view.current);
=======
        self.presentation =
            Some(SavedDisplay { layout: self.layout, continuous: self.continuous, cover: self.cover, fit: self.fit, zoom: self.zoom });
        self.selection = None;
        self.presenter = Default::default();
        // Off, the Fill screen preference keeps the display full screen was entered from (#4273),
        // and the reading position in it.
        if self.full_screen_fill {
            self.continuous = false;
            self.fit = Fit::Page;
            self.goto = Some((self.current, 0.0));
        } else {
            self.goto = Some(self.reading_position());
        }
>>>>>>> upstream/main
    }

    /// Leave full screen: the display from before it comes back, at the page reached meanwhile.
    /// From a display full screen kept, at the reading position.
    pub fn leave_presentation(&mut self) {
        let goto = if self.fills_screen() { (self.current, 0.0) } else { self.reading_position() };
        let Some(saved) = self.presentation.take() else { return };
<<<<<<< HEAD
        self.session.presenter = Default::default();
        self.view.display = saved.display;
        self.view.continuous = saved.continuous;
        self.view.cover = saved.cover;
        self.view.fit = saved.fit;
        self.view.zoom = saved.zoom;
        self.view.go_to(self.view.current);
=======
        self.presenter = Default::default();
        self.layout = saved.layout;
        self.continuous = saved.continuous;
        self.cover = saved.cover;
        self.fit = saved.fit;
        self.zoom = saved.zoom;
        self.goto = Some(goto);
    }

    /// Where the reader is, as of the last frame: the page at the top of the document area and
    /// the fraction of it scrolled past, or the top of the current page when no page is there.
    fn reading_position(&self) -> (usize, f32) {
        let top = self.viewport_screen.top();
        self.screen_xforms
            .iter()
            .find(|(_, xf)| xf.rect.top() <= top && top < xf.rect.bottom() && xf.rect.height() > 0.0)
            .map_or((self.current, 0.0), |(p, xf)| (*p, ((top - xf.rect.top()) / xf.rect.height()).clamp(0.0, 1.0)))
>>>>>>> upstream/main
    }

    /// The display shown outside full screen: the one presenting replaced, or the current one.
    pub fn normal_display(&self) -> SavedDisplay {
        self.presentation.unwrap_or(SavedDisplay {
            display: self.view.display,
            continuous: self.view.continuous,
            cover: self.view.cover,
            fit: self.view.fit,
            zoom: self.view.zoom,
        })
    }

    /// The display saved before entering full screen, while presenting.
    pub fn saved_display_mut(&mut self) -> Option<&mut SavedDisplay> {
        self.presentation.as_mut()
    }

    /// Whether the view is in full screen's one-page-at-a-time display.
    pub fn presenting(&self) -> bool {
        self.presentation.is_some()
    }

    /// Whether full screen fills the screen with one page (or spread) at a time: then the page
    /// takes the whole screen, with no margin around it (#576).
    pub fn fills_screen(&self) -> bool {
        self.presentation.is_some() && self.full_screen_fill
    }

    /// The space above the first page and below the last: none while full screen fills the screen.
    pub(super) fn margin(&self) -> f32 {
        if self.fills_screen() { 0.0 } else { MARGIN }
    }

    /// The least space beside the pages: none while full screen fills the screen.
    pub(super) fn side(&self) -> f32 {
        if self.fills_screen() { 0.0 } else { SIDE }
    }

    /// Whether full screen is playing a page transition (or holding the page before it until the
    /// next one has rendered).
    pub fn presenting_a_transition(&self) -> bool {
        self.session.presenter.turning()
    }

    /// Whether the view scrolls continuously (as opposed to one page or spread at a time).
    pub fn continuous(&self) -> bool {
        self.view.continuous
    }

    /// Whether the cover page setting applies: only two-page view has a cover page.
    pub fn cover_applies(&self) -> bool {
        self.view.display == PageDisplay::TwoUp
    }

    /// The page (or spread) the view shows first, as `FitWindow` sizes the window to it
    /// ([`crate::viewer_prefs::window_commands`]). `None` when there is no such page.
    ///
    /// The row is the framework's own spread rule. The margins are the spacing the last layout used: the
    /// gutter at each side, the gap between a spread's pages, and the margin above and below.
    pub(crate) fn first_page_now(&self, info: &DocInfo) -> Option<crate::viewer_prefs::PageNow> {
        info.pages.get(self.view.current)?;
        let spacing = self.spacing?;
        let options = LayoutOptions {
            display: self.view.display,
            cover: self.view.cover,
            continuous: self.view.continuous,
            page_gaps: self.view.page_gaps,
            ..LayoutOptions::ACROBAT
        };
        let row = shown_row(info.pages.len(), self.view.current, &options);
        let turned = self.view.rotation % 180 == 90;
        let sizes: Vec<(f32, f32)> =
            info.pages.get(row).unwrap_or_default().iter().map(|p| if turned { (p.height, p.width) } else { (p.width, p.height) }).collect();
        let w: f32 = sizes.iter().map(|s| s.0).sum();
        let h = sizes.iter().map(|s| s.1).fold(0.0, f32::max);
        let gap = if self.view.page_gaps { spacing.gap as f32 } else { 0.0 };
        let across = gap * sizes.len().saturating_sub(1) as f32;
        Some(crate::viewer_prefs::PageNow {
            size: vec2(w * PT, h * PT),
            margins: vec2(2.0 * spacing.side as f32 + across, 2.0 * spacing.margin as f32),
            zoom: (self.view.fit == Fit::None).then_some(self.view.zoom as f32),
        })
    }

    /// Scale of the thumbnails in the Pages inspector (1.0 is the usual size).
    pub fn thumbnail_zoom(&self) -> f32 {
        self.thumbnail_zoom
    }

    /// Resize the inspector thumbnails.
    /// Out-of-range values are clamped to the framework's thumbnail zoom; non-finite values are ignored.
    pub fn set_thumbnail_zoom(&mut self, zoom: f32) {
        if zoom.is_finite() {
            let range = THUMBNAIL_ZOOM.range();
            self.thumbnail_zoom = zoom.clamp(*range.start() as f32, *range.end() as f32);
        }
    }

    /// A zoom typed as a percentage ("150", "150%", " 62.5 % ") as a zoom factor; `None` unless it is a
    /// finite, positive number. The page view clamps it to the supported range.
    pub fn parse_zoom_percent(typed: &str) -> Option<f32> {
        let percent = typed.trim().trim_end_matches('%').trim_end().parse::<f32>().ok()?;
        (percent.is_finite() && percent > 0.0).then_some(percent / 100.0)
    }
<<<<<<< HEAD
=======

    /// Standard zoom steps (as in Acrobat's zoom menu).
    pub fn zoom_step(&mut self, up: bool) {
        const STEPS: [f32; 17] = [0.1, 0.25, 0.33, 0.5, 0.66, 0.75, 1.0, 1.25, 1.5, 2.0, 3.0, 4.0, 6.0, 8.0, 12.0, 16.0, 32.0];
        let z = self.zoom;
        let next = if up { STEPS.iter().copied().find(|s| *s > z * 1.01) } else { STEPS.iter().rev().copied().find(|s| *s < z * 0.99) };
        if let Some(n) = next {
            self.set_zoom(n);
        }
    }

    /// The first page of the two-page spread page `c` is in: spreads are [0, 1], [2, 3], … or,
    /// with a cover page, [0], [1, 2], [3, 4], ….
    pub(super) fn spread_first(&self, c: usize) -> usize {
        if self.cover { c.saturating_sub((c + 1) % 2) } else { c - c % 2 }
    }

    /// The display sizes of the spread full screen shows in two-page view (one spread at a time):
    /// its two pages, or one alone. `None` in every other display, full screen that keeps the
    /// window's display (Fill off, #4273) included: that fits the document as the window does.
    fn full_screen_spread(&self, info: &DocInfo) -> Option<Vec<(f32, f32)>> {
        if !self.fills_screen() || self.layout != PageLayout::TwoUp || self.continuous {
            return None;
        }
        let first = self.spread_first(self.current);
        let last = if self.cover && first == 0 { 0 } else { first.saturating_add(1) };
        Some(info.pages.iter().skip(first).take(last - first + 1).map(|p| self.display_size(p)).collect())
    }

    pub(super) fn fit_zoom(&mut self, info: &DocInfo) {
        let largest = |side: fn((f32, f32)) -> f32| info.pages.iter().map(|p| side(self.display_size(p))).fold(1.0, f32::max);
        let max_w = largest(|s| s.0);
        // A single page shown on its own fits that page, and full screen filling the screen in
        // two-page view fits the spread it shows (review on #2070). Every other display
        // (scrolling, or two-page view in the window or in a full screen that keeps the window's
        // display) fits its largest page, so the zoom holds still while pages of other sizes
        // scroll or page past. A spread of two pages fits their own widths side by
        // side; a page alone in a spread keeps a spread's room (a cover keeps its blank slot).
        let mut spread_w = None;
        let (w, h) = if self.layout == PageLayout::Single && !self.continuous {
            info.pages.get(self.current).map_or_else(|| (max_w, largest(|s| s.1)), |p| self.display_size(p))
        } else if let Some(spread) = self.full_screen_spread(info) {
            if spread.is_empty() {
                (max_w, largest(|s| s.1))
            } else {
                if let [a, b] = spread.as_slice() {
                    spread_w = Some((a.0 + b.0).max(1.0));
                }
                (spread.iter().map(|s| s.0).fold(1.0, f32::max), spread.iter().map(|s| s.1).fold(1.0, f32::max))
            }
        } else {
            (max_w, largest(|s| s.1))
        };
        let avail_w = (self.viewport_w - 2.0 * self.side()).max(100.0);
        let per_row = if self.layout == PageLayout::TwoUp { 2.0 } else { 1.0 };
        let zw = (avail_w - self.page_gap() * (per_row - 1.0)) / (spread_w.unwrap_or(w * per_row) * PT);
        match self.fit {
            Fit::Width => self.zoom = zw,
            Fit::Page => {
                let zh = (self.viewport_h - 2.0 * self.margin()) / (h * PT);
                self.zoom = zw.min(zh);
            }
            Fit::Height => self.zoom = (self.viewport_h - 2.0 * self.margin()) / (h * PT),
            Fit::None => {}
        }
        self.zoom = self.zoom.clamp(0.08, 64.0);
    }

    /// Width of the scroll content. Single-page view sizes it to the page it shows, so a
    /// wider page elsewhere in the document can't push the shown one off-centre.
    pub(super) fn content_width(&self, info: &DocInfo, avail_w: f32) -> f32 {
        // Full screen in two-page view is as wide as the spread it shows, as `fit_zoom` sized it:
        // two pages side by side, or a page alone with the room of a spread (a cover keeps its
        // blank slot). Room for twice the document's widest page would centre a narrower spread
        // past the screen's edge.
        if let Some(spread) = self.full_screen_spread(info).filter(|s| !s.is_empty()) {
            let s = self.zoom * PT;
            let w = match spread.as_slice() {
                [a, b] => (a.0 + b.0) * s,
                _ => spread.iter().map(|p| p.0).fold(0.0, f32::max) * s * 2.0,
            };
            return (w + self.page_gap() + 2.0 * self.side()).max(avail_w);
        }
        let single = (self.layout == PageLayout::Single && !self.continuous).then(|| self.current.min(info.pages.len().saturating_sub(1)));
        let shown = info.pages.iter().enumerate().filter(|&(i, _)| single.is_none_or(|c| c == i));
        let max_w = shown.map(|(_, p)| self.display_size(p).0).fold(0.0, f32::max);
        let per_row = if self.layout == PageLayout::TwoUp { 2.0 } else { 1.0 };
        (max_w * self.zoom * PT * per_row + 2.0 * self.side()).max(avail_w)
    }

    /// Page rects in content coordinates (origin at the scroll content's top-left).
    pub(super) fn layout(&self, info: &DocInfo, content_w: f32, ppp: f32) -> Vec<Rect> {
        let s = self.zoom * PT;
        let gap = self.page_gap();
        let page_size = |p: &pdfcraft_render::PageInfo| {
            let (w, h) = self.display_size(p);
            if self.show_page_gaps {
                vec2(w * s, h * s)
            } else {
                // Match the raster's whole physical pixels. Snapping each page's origin
                // independently with fractional sizes would leave hairline seams.
                vec2(device_pixels(w, s * ppp) as f32 / ppp, device_pixels(h, s * ppp) as f32 / ppp)
            }
        };
        let mut rects = Vec::with_capacity(info.pages.len());
        let mut y = self.margin();
        let side = self.side();
        match self.layout {
            PageLayout::Single => {
                for p in &info.pages {
                    let size = page_size(p);
                    rects.push(Rect::from_min_size(pos2(((content_w - size.x) / 2.0).max(side), y), size));
                    y += size.y + gap;
                }
            }
            PageLayout::TwoUp => {
                // With a cover page, the first page sits alone on the right.
                let rows: Vec<&[pdfcraft_render::PageInfo]> = if self.cover && !info.pages.is_empty() {
                    std::iter::once(&info.pages[..1]).chain(info.pages[1..].chunks(2)).collect()
                } else {
                    info.pages.chunks(2).collect()
                };
                for (ri, pair) in rows.into_iter().enumerate() {
                    let sizes: Vec<Vec2> = pair.iter().map(page_size).collect();
                    let row_w: f32 = sizes.iter().map(|v| v.x).sum::<f32>() + gap * (sizes.len() as f32 - 1.0);
                    let row_h = sizes.iter().map(|v| v.y).fold(0.0, f32::max);
                    let mut x = if self.cover && ri == 0 { (content_w / 2.0 + gap / 2.0).max(side) } else { ((content_w - row_w) / 2.0).max(side) };
                    for size in sizes {
                        rects.push(Rect::from_min_size(pos2(x, y + (row_h - size.y) / 2.0), size));
                        x += size.x + gap;
                    }
                    y += row_h + gap;
                }
            }
        }
        rects
    }
}

/// The scroll offset that puts `point` (content coordinates) at `rel` in the viewport, kept within
/// the content as egui keeps it. egui clamps only after drawing, so an offset past the end showed
/// the page misplaced for a frame and then jumped (zooming out beside a page that fits).
pub(super) fn anchored_scroll(point: Pos2, rel: Vec2, content: Vec2, viewport: Vec2) -> Vec2 {
    let max = vec2((content.x - viewport.x).max(0.0), (content.y - viewport.y).max(0.0));
    vec2((point.x - rel.x).clamp(0.0, max.x), (point.y - rel.y).clamp(0.0, max.y))
>>>>>>> upstream/main
}
