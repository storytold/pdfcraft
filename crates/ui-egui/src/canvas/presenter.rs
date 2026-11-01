//! Full screen as a presentation (ISO 32000-2 §12.4.4): a page's transition (`/Trans`, Table 164)
//! plays when the page comes into view, a page with a display time (`/Dur`) advances by itself,
//! and the pointer hides while it stays still.
//!
//! Transitions play in single-page view without view rotation, from the outgoing page's raster
//! to the incoming one's once it has rendered; spreads, a rotated view and animations turned off
//! (the Animation effects preference, which by default follows the system's request for no
//! animations) change pages at once. A style the spec doesn't name is its default, Replace, a
//! cut. Display times count from the end of the transition and stop at the last page; the keys
//! and clicks still turn pages meanwhile (§12.4.4.1, NOTE 1), and leaving full screen stops them.
//! The file's numbers are bounded: a transition plays for at most `MAX_TRANSITION` seconds, and a
//! display time waits at most `MAX_DISPLAY`.

use egui::{Color32, CornerRadius, Painter, Pos2, Rect, TextureHandle, pos2};
use pdfcraft_render::transition::{self, Effect, Piece};
use pdfcraft_render::{DocInfo, PageTransition};

use super::{DocView, PageLayout};

/// Seconds the pointer stays still before full screen hides it.
pub(crate) const HIDE_POINTER_AFTER: f64 = 3.0;
/// Bounds on a document's numbers, which are untrusted: the longest transition played and the
/// longest display time waited for (a day: a timer can't hold the largest number a file can
/// write).
const MAX_TRANSITION: f64 = 10.0;
const MAX_DISPLAY: f64 = 24.0 * 3600.0;

/// What full screen remembers between frames.
#[derive(Default)]
pub(crate) struct Presenter {
    /// The page wholly in view, and since when (its transition over).
    shown: Option<(usize, f64)>,
    turn: Option<Turn>,
    /// Where the pointer last moved to, and when.
    pointer: Option<(Pos2, f64)>,
}

/// A transition under way, from one page to the next.
struct Turn {
    from_page: usize,
    /// The outgoing page's raster, kept while the transition plays.
    from: TextureHandle,
    to_page: usize,
    effect: Effect,
    seconds: f64,
    /// When the incoming page's raster arrived and the transition began.
    started: Option<f64>,
}

impl Presenter {
    /// Whether a page transition is playing or waiting for its incoming page.
    pub(crate) fn turning(&self) -> bool {
        self.turn.is_some()
    }
}

/// The transition and display time of page `page` (0-based), if it has either.
fn entry(info: &DocInfo, page: usize) -> Option<&PageTransition> {
    info.transitions.iter().find(|t| t.page == page)
}

/// The effect a page's transition plays and its length in seconds; `None` for no visible effect.
fn effect_of(t: &PageTransition) -> Option<(Effect, f64)> {
    let style = t.style?;
    let effect = Effect::from_parts(style.name(), t.horizontal, t.inward, t.direction, t.scale);
    let seconds = f64::from(t.duration);
    (effect != Effect::Replace && seconds.is_finite() && seconds > 0.0).then(|| (effect, seconds.min(MAX_TRANSITION)))
}

/// `rect` with the aspect ratio of a `w` by `h` page, as large as fits in `within` and centred
/// there.
fn fitted(w: f32, h: f32, within: Rect) -> Rect {
    let (w, h) = (w.max(1.0), h.max(1.0));
    let k = (within.width() / w).min(within.height() / h);
    if !k.is_finite() || k <= 0.0 {
        return within;
    }
    Rect::from_center_size(within.center(), egui::vec2(w * k, h * k))
}

/// Where a piece of a page goes on screen and which part of its raster it shows: `dst` in `page`
/// (screen), `src` as texture coordinates, and its tint. `None` for a piece with nothing to show.
pub(crate) fn piece_on_screen(page: Rect, p: &Piece) -> Option<(Rect, Rect, Color32)> {
    let alpha = if p.alpha.is_finite() { p.alpha.clamp(0.0, 1.0) } else { 0.0 };
    let at = |f: [f32; 4]| {
        let f = f.map(|v| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 });
        (pos2(f[0], f[1]), pos2(f[2], f[3]))
    };
    let (d0, d1) = at(p.dst);
    let (s0, s1) = at(p.src);
    let dst = Rect::from_min_max(page.lerp_inside(d0.to_vec2()), page.lerp_inside(d1.to_vec2()));
    let uv = Rect::from_min_max(s0, s1);
    (alpha > 0.0 && dst.width() > 0.0 && dst.height() > 0.0).then(|| (dst, uv, Color32::WHITE.gamma_multiply(alpha)))
}

fn paint(painter: &Painter, tex: &TextureHandle, page: Rect, pieces: &[Piece]) {
    for p in pieces {
        if let Some((dst, uv, tint)) = piece_on_screen(page, p) {
            painter.image(tex.id(), dst, uv, tint);
        }
    }
}

/// One frame of full screen for `view`, after its pages were drawn: start, paint or finish the
/// current page's transition, and advance a page whose display time is up. Returns whether the
/// pointer has been still long enough to hide: the caller hides it once the page has chosen its
/// cursor, which would otherwise show it again over a link. `animates` is the Animation effects
/// preference as it applies now.
pub(super) fn frame(
    view: &mut DocView,
    info: &DocInfo,
    painter: &Painter,
    ctx: &egui::Context,
    now: f64,
    animates: bool,
    pointer: Option<Pos2>,
) -> bool {
    let page = view.current;
    let showing = view.presenter.shown.map(|(p, _)| p);
    let arriving = view.presenter.turn.as_ref().map(|t| t.to_page);
    if showing != Some(page) && arriving != Some(page) {
        // A new page: play its transition from the page before, when there is one to play.
        let plays = animates && view.layout == PageLayout::Single && view.rotation == 0;
        let turn = showing.or(arriving).filter(|_| plays).and_then(|from_page| {
            let (effect, seconds) = entry(info, page).and_then(effect_of)?;
            let from = view.pages.get(&from_page)?.tex.clone();
            Some(Turn { from_page, from, to_page: page, effect, seconds, started: None })
        });
        // Its display time starts once the page is on screen, not while it still says Rendering…
        // (or at once for a page the renderer can't draw, which never will be). In two-page view
        // that is every page of the spread, so the spread isn't turned before both were shown.
        let drawn = |p: &usize| view.pages.contains_key(p) || view.errors.contains_key(p);
        let ready = drawn(&page) && view.screen_rects.iter().all(|(p, _)| drawn(p));
        view.presenter.shown = if turn.is_some() || !ready { None } else { Some((page, now)) };
        view.presenter.turn = turn;
    }

    if let Some(turn) = view.presenter.turn.as_mut() {
        // A page the renderer can't draw ends the turn: waiting for it would hold the page before
        // forever, and its display time would never start.
        let failed = view.errors.contains_key(&turn.to_page);
        let to_rect = view.screen_rects.iter().find(|(i, _)| *i == turn.to_page).map(|&(_, r)| r).filter(|_| !failed);
        let incoming = view.pages.get(&turn.to_page).map(|p| p.tex.clone());
        match to_rect {
            None => {
                view.presenter.turn = None;
                view.presenter.shown = Some((page, now));
            }
            Some(to_rect) => {
                // The outgoing page keeps its own shape, centred where the incoming one is.
                let from_rect = info.pages.get(turn.from_page).map_or(to_rect, |p| fitted(p.width, p.height, to_rect.union(view.viewport_screen)));
                let from_rect = if (from_rect.aspect_ratio() - to_rect.aspect_ratio()).abs() < 0.01 { to_rect } else { from_rect };
                let t = match (&incoming, turn.started) {
                    // Hold the outgoing page until the incoming one has rendered.
                    (None, _) => 0.0,
                    (Some(_), Some(start)) => ((now - start) / turn.seconds) as f32,
                    (Some(_), None) => {
                        turn.started = Some(now);
                        0.0
                    }
                };
                if t >= 1.0 {
                    view.presenter.turn = None;
                    view.presenter.shown = Some((page, now));
                } else {
                    let f = transition::frame(&turn.effect, t);
                    painter.rect_filled(from_rect.union(to_rect), CornerRadius::ZERO, Color32::BLACK);
                    paint(painter, &turn.from, from_rect, &f.outgoing);
                    if let Some(tex) = &incoming {
                        paint(painter, tex, to_rect, &f.incoming);
                        ctx.request_repaint();
                    } else {
                        // Nothing moves until the page arrives, and the render cache wakes the window
                        // while pages are queued: look again now and then rather than every frame.
                        ctx.request_repaint_after(std::time::Duration::from_millis(50));
                    }
                }
            }
        }
    }

    // A page with a display time advances once it is up (not past the last page).
    if let Some((shown, since)) = view.presenter.shown
        && shown == page
        && page + 1 < info.pages.len()
        && let Some(seconds) = entry(info, page).and_then(|t| t.display).map(f64::from).filter(|d| d.is_finite() && *d >= 0.0)
    {
        let left = since + seconds.min(MAX_DISPLAY) - now;
        if left <= 0.0 {
            // In two-page view the last spread can't advance: then there is nothing to redraw.
            let before = view.current;
            view.step_page(true);
            if view.current != before {
                ctx.request_repaint();
            }
        } else {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(left));
        }
    }

    // The pointer hides once it has been still for a while, and comes back when it moves.
    match pointer {
        None => view.presenter.pointer = None,
        Some(p) if view.presenter.pointer.map(|(q, _)| q) != Some(p) => view.presenter.pointer = Some((p, now)),
        Some(_) => {}
    }
    let Some((_, since)) = view.presenter.pointer else { return false };
    let left = since + HIDE_POINTER_AFTER - now;
    if left > 0.0 {
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(left));
    }
    left <= 0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page() -> Rect {
        Rect::from_min_size(pos2(100.0, 50.0), egui::vec2(200.0, 400.0))
    }

    #[test]
    fn a_piece_maps_page_fractions_to_the_screen_and_its_raster() {
        let p = Piece { src: [0.0, 0.0, 0.5, 1.0], dst: [0.5, 0.0, 1.0, 1.0], alpha: 1.0 };
        let (dst, uv, tint) = piece_on_screen(page(), &p).unwrap();
        assert_eq!(dst, Rect::from_min_max(pos2(200.0, 50.0), pos2(300.0, 450.0)));
        assert_eq!(uv, Rect::from_min_max(pos2(0.0, 0.0), pos2(0.5, 1.0)));
        assert_eq!(tint, Color32::WHITE);
        let faded = piece_on_screen(page(), &Piece { alpha: 0.5, ..p }).unwrap().2;
        assert!(faded.a() > 0 && faded.a() < 255, "half transparent: {faded:?}");
    }

    #[test]
    fn hostile_pieces_paint_nothing_or_stay_on_the_page() {
        for p in [
            Piece { src: [0.0; 4], dst: [0.2, 0.2, 0.2, 0.9], alpha: 1.0 },
            Piece { src: [0.0, 0.0, 1.0, 1.0], dst: [0.0, 0.0, 1.0, 1.0], alpha: 0.0 },
            Piece { src: [0.0, 0.0, 1.0, 1.0], dst: [0.0, 0.0, 1.0, 1.0], alpha: f32::NAN },
        ] {
            assert!(piece_on_screen(page(), &p).is_none(), "{p:?}");
        }
        let (dst, uv, _) =
            piece_on_screen(page(), &Piece { src: [f32::NAN, -3.0, 9.0, f32::INFINITY], dst: [-1.0, f32::NAN, 2.0, 1.5], alpha: 1.0 }).unwrap();
        assert!(page().contains_rect(dst), "{dst:?}");
        assert!(Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)).contains_rect(uv), "{uv:?}");
    }

    #[test]
    fn only_a_visible_effect_with_a_length_plays() {
        use pdfcraft_render::TransitionStyle;
        let t = |style, duration| PageTransition {
            page: 1,
            style,
            duration,
            horizontal: true,
            inward: true,
            direction: Some(0),
            scale: 1.0,
            opaque: false,
            display: None,
        };
        assert_eq!(effect_of(&t(Some(TransitionStyle::Wipe), 1.5)), Some((Effect::Wipe { direction: 0 }, 1.5)));
        assert_eq!(effect_of(&t(Some(TransitionStyle::Fade), 600.0)).map(|e| e.1), Some(MAX_TRANSITION), "capped");
        for (style, duration) in
            [(None, 1.0), (Some(TransitionStyle::Replace), 1.0), (Some(TransitionStyle::Fade), 0.0), (Some(TransitionStyle::Fade), f32::NAN)]
        {
            assert_eq!(effect_of(&t(style, duration)), None, "{style:?} {duration}");
        }
    }

    #[test]
    fn the_outgoing_page_keeps_its_shape() {
        let r = fitted(600.0, 300.0, Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(400.0, 400.0)));
        assert_eq!(r, Rect::from_min_max(pos2(0.0, 100.0), pos2(400.0, 300.0)));
        assert_eq!(fitted(0.0, f32::NAN, Rect::ZERO), Rect::ZERO);
    }
}
