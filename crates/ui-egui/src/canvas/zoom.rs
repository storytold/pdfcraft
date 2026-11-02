use super::{DocInfo, DocView, Fit, GAP, MARGIN, PT, PageLayout, Pos2, Rect, SIDE, SavedDisplay, Vec2, device_pixels, pos2, vec2};

/// The page grid's zoom range and the step of its buttons and keys.
pub const GRID_ZOOM_RANGE: std::ops::RangeInclusive<f32> = 0.5..=3.0;

/// Thumbnail size in the Pages inspector, relative to its usual size.
pub const THUMBNAIL_ZOOM_RANGE: std::ops::RangeInclusive<f32> = 0.5..=1.5;

pub(super) const GRID_ZOOM_STEP: f32 = 1.25;

impl DocView {
    /// Rotate the view 90° clockwise or counter-clockwise (View ▸ Rotate View, ⇧⌘+ / ⇧⌘−).
    pub fn rotate_view(&mut self, clockwise: bool) {
        self.rotation = (self.rotation + if clockwise { 90 } else { 270 }) % 360;
        self.goto = Some((self.current, 0.0));
        self.reset_coarse_state();
    }

    /// Switch the zoom mode, keeping the current page in view.
    pub fn set_fit(&mut self, fit: Fit) {
        self.fit = fit;
        self.goto = Some((self.current, 0.0));
        self.reset_coarse_state();
    }

    /// Full screen, as Acrobat's Full Screen Mode (#379, #1459): one page at a time (or one
    /// spread, when two-page view was on: #1353), fitted to the screen, from the current page.
    /// The display it replaces is kept for [`Self::leave_presentation`]. A no-op while presenting.
    pub fn enter_presentation(&mut self) {
        if self.presentation.is_some() {
            return;
        }
        self.presentation =
            Some(SavedDisplay { layout: self.layout, continuous: self.continuous, cover: self.cover, fit: self.fit, zoom: self.zoom });
        self.continuous = false;
        self.fit = Fit::Page;
        self.selection = None;
        self.goto = Some((self.current, 0.0));
    }

    /// Leave full screen: the display from before it comes back, at the page reached meanwhile.
    pub fn leave_presentation(&mut self) {
        let Some(saved) = self.presentation.take() else { return };
        self.layout = saved.layout;
        self.continuous = saved.continuous;
        self.cover = saved.cover;
        self.fit = saved.fit;
        self.zoom = saved.zoom;
        self.goto = Some((self.current, 0.0));
    }

    /// The display shown outside full screen: the one presenting replaced, or the current one.
    pub fn normal_display(&self) -> SavedDisplay {
        self.presentation.unwrap_or(SavedDisplay {
            layout: self.layout,
            continuous: self.continuous,
            cover: self.cover,
            fit: self.fit,
            zoom: self.zoom,
        })
    }

    /// Whether the view is in full screen's one-page-at-a-time display.
    pub fn presenting(&self) -> bool {
        self.presentation.is_some()
    }

    /// Switch the base page display (Single or Two-page), leaving continuous scrolling as it is.
    /// A no-op when the layout is already current, so re-selecting it never yanks a scrolled view
    /// back to the top.
    pub fn set_layout(&mut self, layout: PageLayout) {
        if self.layout == layout {
            return;
        }
        self.layout = layout;
        self.goto = Some((self.current, 0.0));
        self.reset_coarse_state();
    }

    /// Whether the view scrolls continuously (as opposed to one page or spread at a time).
    pub fn continuous(&self) -> bool {
        self.continuous
    }

    /// Change page separation without editing the PDF or losing the reader's place.
    pub fn set_page_gaps(&mut self, show: bool) {
        if self.show_page_gaps == show {
            return;
        }
        if !self.organize && self.viewport_screen.is_positive() {
            let at = self.viewport_screen.center();
            if let Some((page, r)) = self
                .screen_rects
                .iter()
                .filter(|(_, r)| r.is_positive())
                .min_by(|(_, a), (_, b)| a.distance_sq_to_pos(at).total_cmp(&b.distance_sq_to_pos(at)))
            {
                let fraction = (at - r.min) / r.size();
                self.zoom_anchor = Some((*page, fraction.x, fraction.y, at - self.viewport_screen.min));
            }
        }
        self.show_page_gaps = show;
    }

    /// Separation between adjacent page rectangles, in logical screen points.
    pub(super) fn page_gap(&self) -> f32 {
        if self.show_page_gaps { GAP } else { 0.0 }
    }

    /// Turn continuous scrolling on or off, keeping the current page in view. A no-op when it is
    /// already in that state.
    pub fn set_continuous(&mut self, continuous: bool) {
        if self.continuous != continuous {
            self.continuous = continuous;
            // Select All of the document belongs to continuous scrolling.
            self.all_text = false;
            self.goto = Some((self.current, 0.0));
            self.reset_coarse_state();
        }
    }

    /// Set the whole page display at once (the base layout and continuous scrolling together),
    /// as the control channel and settings do through a single [`display_token`].
    pub fn set_display(&mut self, layout: PageLayout, continuous: bool) {
        self.set_layout(layout);
        self.set_continuous(continuous);
    }

    /// Set the cover page in two-page view. A no-op unless something changed.
    pub fn set_cover(&mut self, cover: bool) {
        if self.cover != cover {
            self.cover = cover;
            self.goto = Some((self.current, 0.0));
            self.reset_coarse_state();
        }
    }

    /// Whether the cover page setting applies: only two-page view has a cover page.
    pub fn cover_applies(&self) -> bool {
        self.layout == PageLayout::TwoUp
    }

    /// Displayed page size in points for this view rotation.
    pub(super) fn display_size(&self, p: &pdfcraft_render::PageInfo) -> (f32, f32) {
        if self.rotation % 180 == 90 { (p.height, p.width) } else { (p.width, p.height) }
    }

    /// Marquee Zoom: zoom so `r` (on screen) fills the window, centred.
    pub fn zoom_to_rect(&mut self, r: Rect) {
        let Some((page, pr)) = self.screen_rects.iter().find(|(_, pr)| pr.contains(r.center())).copied() else { return };
        let k = (self.viewport_screen.width() / r.width().max(1.0)).min(self.viewport_screen.height() / r.height().max(1.0));
        let f = (r.center() - pr.min) / pr.size();
        self.zoom = (self.zoom * k).clamp(0.08, 64.0);
        self.fit = Fit::None;
        self.zoom_anchor = Some((page, f.x.clamp(0.0, 1.0), f.y.clamp(0.0, 1.0), self.viewport_screen.center() - self.viewport_screen.min));
    }

    /// View ▸ Zoom ▸ Fit Visible: zoom so the visible content of `page` (display-normalised
    /// [x0, y0, x1, y1], i.e. after view rotation) spans the window's width, its left edge at
    /// the window's left and its top at the top.
    pub fn fit_content(&mut self, page: usize, content: [f32; 4]) -> bool {
        const PAD: f32 = 16.0;
        let Some((_, pr)) = self.screen_rects.iter().find(|(p, _)| *p == page).copied() else { return false };
        let w = (content[2] - content[0]).max(0.01) * pr.width();
        let k = (self.viewport_screen.width() - 2.0 * PAD).max(50.0) / w.max(1.0);
        self.zoom = (self.zoom * k).clamp(0.08, 64.0);
        self.fit = Fit::None;
        self.zoom_anchor = Some((page, content[0].clamp(0.0, 1.0), content[1].clamp(0.0, 1.0), vec2(PAD, MARGIN)));
        true
    }

    /// How large the page grid draws its pages (1.0 = the usual thumbnails).
    pub fn grid_zoom(&self) -> f32 {
        self.grid_zoom
    }

    /// Scale of the thumbnails in the Pages inspector (1.0 is the usual size).
    pub fn thumbnail_zoom(&self) -> f32 {
        self.thumbnail_zoom
    }

    /// Resize the inspector thumbnails, keeping the current page in view after layout.
    /// Out-of-range values are clamped; non-finite values are ignored.
    pub fn set_thumbnail_zoom(&mut self, zoom: f32) {
        if zoom.is_finite() {
            let zoom = zoom.clamp(*THUMBNAIL_ZOOM_RANGE.start(), *THUMBNAIL_ZOOM_RANGE.end());
            if zoom != self.thumbnail_zoom {
                self.thumbnail_zoom = zoom;
                self.panel_followed = None;
            }
        }
    }

    /// Zoom the page grid; out-of-range values are clamped and nonsense is ignored.
    pub fn set_grid_zoom(&mut self, zoom: f32) {
        if zoom.is_finite() {
            self.grid_zoom = zoom.clamp(*GRID_ZOOM_RANGE.start(), *GRID_ZOOM_RANGE.end());
        }
    }

    /// Zoom keeping the centre of the view still.
    pub fn set_zoom(&mut self, zoom: f32) {
        // CLI/view options can set zoom before the first layout. Rect::NOTHING has no
        // finite centre yet, but an initial zoom still needs to leave the default fit mode.
        let centre = if self.viewport_screen.is_positive() { self.viewport_screen.center() } else { Pos2::ZERO };
        self.zoom_at(zoom, centre);
    }

    /// Zoom keeping the document point under `screen_pos` still (pinch, ⌘-scroll).
    pub fn zoom_at(&mut self, zoom: f32, screen_pos: Pos2) {
        if !zoom.is_finite() || !screen_pos.is_finite() {
            return;
        }
        let zoom = zoom.clamp(0.08, 64.0);
        if zoom == self.zoom && self.fit == Fit::None {
            return;
        }
        // In gutters/gaps, anchor relative to the nearest page rather than jumping to its top.
        // Fractions outside 0..1 preserve the pointer's position outside the page as well.
        let anchor =
            self.screen_rects.iter().min_by(|(_, a), (_, b)| a.distance_sq_to_pos(screen_pos).total_cmp(&b.distance_sq_to_pos(screen_pos))).map(
                |(p, r)| {
                    let f = (screen_pos - r.min) / r.size();
                    (*p, f.x, f.y, screen_pos - self.viewport_screen.min)
                },
            );
        self.zoom = zoom;
        self.fit = Fit::None;
        match anchor {
            Some(a) => self.zoom_anchor = Some(a),
            // Nothing on screen yet (e.g. a launch option): align the current page instead.
            None => self.goto = Some((self.current, 0.0)),
        }
    }

    /// A zoom typed as a percentage ("150", "150%", " 62.5 % ") as a zoom factor; `None` unless it is a
    /// finite, positive number. `set_zoom` clamps it to the supported range.
    pub fn parse_zoom_percent(typed: &str) -> Option<f32> {
        let percent = typed.trim().trim_end_matches('%').trim_end().parse::<f32>().ok()?;
        (percent.is_finite() && percent > 0.0).then_some(percent / 100.0)
    }

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

    pub(super) fn fit_zoom(&mut self, info: &DocInfo) {
        let largest = |side: fn((f32, f32)) -> f32| info.pages.iter().map(|p| side(self.display_size(p))).fold(1.0, f32::max);
        let max_w = largest(|s| s.0);
        // A single page shown on its own fits that page, and full screen in two-page view fits
        // the spread it shows (review on #2070). Every other display (scrolling, or two-page
        // view in the window) fits its largest page, so the zoom holds still while pages of
        // other sizes scroll or page past.
        let (w, h) = if self.layout == PageLayout::Single && !self.continuous {
            info.pages.get(self.current).map_or_else(|| (max_w, largest(|s| s.1)), |p| self.display_size(p))
        } else if self.presentation.is_some() && self.layout == PageLayout::TwoUp && !self.continuous {
            let first = self.spread_first(self.current);
            let last = if self.cover && first == 0 { 0 } else { first.saturating_add(1) };
            let spread: Vec<(f32, f32)> = info.pages.iter().skip(first).take(last - first + 1).map(|p| self.display_size(p)).collect();
            if spread.is_empty() {
                (max_w, largest(|s| s.1))
            } else {
                (spread.iter().map(|s| s.0).fold(1.0, f32::max), spread.iter().map(|s| s.1).fold(1.0, f32::max))
            }
        } else {
            (max_w, largest(|s| s.1))
        };
        let avail_w = (self.viewport_w - 2.0 * SIDE).max(100.0);
        let per_row = if self.layout == PageLayout::TwoUp { 2.0 } else { 1.0 };
        match self.fit {
            Fit::Width => self.zoom = (avail_w - self.page_gap() * (per_row - 1.0)) / (w * PT * per_row),
            Fit::Page => {
                let zw = (avail_w - self.page_gap() * (per_row - 1.0)) / (w * PT * per_row);
                let zh = (self.viewport_h - 2.0 * MARGIN) / (h * PT);
                self.zoom = zw.min(zh);
            }
            Fit::Height => self.zoom = (self.viewport_h - 2.0 * MARGIN) / (h * PT),
            Fit::None => {}
        }
        self.zoom = self.zoom.clamp(0.08, 64.0);
    }

    /// Width of the scroll content. Single-page view sizes it to the page it shows, so a
    /// wider page elsewhere in the document can't push the shown one off-centre.
    pub(super) fn content_width(&self, info: &DocInfo, avail_w: f32) -> f32 {
        let single = (self.layout == PageLayout::Single && !self.continuous).then(|| self.current.min(info.pages.len().saturating_sub(1)));
        let shown = info.pages.iter().enumerate().filter(|&(i, _)| single.is_none_or(|c| c == i));
        let max_w = shown.map(|(_, p)| self.display_size(p).0).fold(0.0, f32::max);
        let per_row = if self.layout == PageLayout::TwoUp { 2.0 } else { 1.0 };
        (max_w * self.zoom * PT * per_row + 2.0 * SIDE).max(avail_w)
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
        let mut y = MARGIN;
        match self.layout {
            PageLayout::Single => {
                for p in &info.pages {
                    let size = page_size(p);
                    rects.push(Rect::from_min_size(pos2(((content_w - size.x) / 2.0).max(SIDE), y), size));
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
                    let mut x = if self.cover && ri == 0 { (content_w / 2.0 + gap / 2.0).max(SIDE) } else { ((content_w - row_w) / 2.0).max(SIDE) };
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
}
