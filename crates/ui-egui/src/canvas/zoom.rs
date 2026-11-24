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
    /// With [`Self::full_screen_fill`] off it keeps the display it is entered from (#4273), at the
    /// reading position. The display it replaces is kept for [`Self::leave_presentation`]. A
    /// no-op while presenting.
    pub fn enter_presentation(&mut self) {
        if self.presentation.is_some() {
            return;
        }
        self.presentation = Some(SavedDisplay {
            display: self.view.display,
            continuous: self.view.continuous,
            cover: self.view.cover,
            fit: self.view.fit,
            zoom: self.view.zoom,
        });
        self.session.text.selection = None;
        self.session.presenter = Default::default();
        if self.full_screen_fill {
            self.view.continuous = false;
            self.view.fit = Fit::Page;
            self.view.go_to(self.view.current);
        } else {
            self.go_to_reading_position();
        }
    }

    /// Enter full screen with the app's Fill screen preference as it stands (#576, #4273): every
    /// view entering full screen, a document opened in it included, takes the preference here. A
    /// no-op while presenting, so a change made meanwhile waits for the next time.
    pub fn enter_full_screen(&mut self, fill: bool) {
        if self.presentation.is_some() {
            return;
        }
        self.full_screen_fill = fill;
        self.enter_presentation();
    }

    /// Leave full screen: the display from before it comes back, at the page reached meanwhile.
    /// From a display full screen kept, at the reading position.
    pub fn leave_presentation(&mut self) {
        let filled = self.fills_screen();
        let Some(saved) = self.presentation.take() else { return };
        self.session.presenter = Default::default();
        self.view.display = saved.display;
        self.view.continuous = saved.continuous;
        self.view.cover = saved.cover;
        self.view.fit = saved.fit;
        self.view.zoom = saved.zoom;
        if filled {
            self.view.go_to(self.view.current);
        } else {
            self.go_to_reading_position();
        }
    }

    /// Takes the next pass to where the reader is, as of the last pass: the page at the top of the
    /// page view and the fraction of it scrolled past, or the top of the current page when no page
    /// is there.
    fn go_to_reading_position(&mut self) {
        let top = self.viewport.top();
        let at = self.session.placed.iter().find(|p| p.rect.top() <= top && top < p.rect.bottom() && p.rect.height() > 0.0);
        match at {
            Some(p) => {
                let fraction = f64::from(((top - p.rect.top()) / p.rect.height()).clamp(0.0, 1.0));
                self.view.go_to_fraction(p.page, fraction);
            }
            None => self.view.go_to(self.view.current),
        }
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
}
