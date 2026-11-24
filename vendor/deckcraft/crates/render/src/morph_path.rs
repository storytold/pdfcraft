//! Morph of an outline: a frame shape whose geometry turns from the old shape's into its own (a
//! circle into a star), with the points aligned by `deckcraft_geom::morph`.

use deckcraft_geom::Xfrm;
use deckcraft_geom::morph::{Fade, GeometryMorph};
use deckcraft_geom::preset::Geometry;
use deckcraft_model::{Geom, Shape, ShapeId};
use kurbo::Vec2;

use crate::shape_geometry;

/// A frame shape (see [`crate::render_blend`]) whose outline morphs from `from` into its own
/// geometry.
#[derive(Clone, Debug, PartialEq)]
pub struct PathMorph {
    pub id: ShapeId,
    /// The old shape's geometry.
    pub from: Geom,
    /// The boxes of the old and the new shape on their own slides.
    pub from_box: Xfrm,
    pub to_box: Xfrm,
    /// Progress, 0..1 (already eased).
    pub t: f64,
}

/// `now / own`, or 1 when that is no usable scale (a line's zero height).
fn ratio(now: f64, own: f64) -> f64 {
    let r = now / own;
    if own.abs() > 1e-9 && r.is_finite() { r } else { 1.0 }
}

/// What a [`GeometryMorph`] is built from: both outlines at their own sizes. None of it changes
/// while a transition plays, only `t` and the current box do.
#[derive(Debug)]
struct Source {
    from: Geom,
    from_wh: (f64, f64),
    to: Geom,
    to_wh: (f64, f64),
}

impl Source {
    fn of(pm: &PathMorph, s: &Shape) -> Self {
        Source { from: pm.from.clone(), from_wh: (pm.from_box.w, pm.from_box.h), to: s.geom.clone(), to_wh: (pm.to_box.w, pm.to_box.h) }
    }

    fn is(&self, pm: &PathMorph, s: &Shape) -> bool {
        self.from == pm.from && self.from_wh == (pm.from_box.w, pm.from_box.h) && self.to == s.geom && self.to_wh == (pm.to_box.w, pm.to_box.h)
    }

    fn build(&self) -> GeometryMorph {
        let geo = |geom: &Geom, (w, h): (f64, f64)| shape_geometry(&Shape { geom: geom.clone(), ..Shape::default() }, w, h);
        GeometryMorph::new(&geo(&self.from, self.from_wh), &geo(&self.to, self.to_wh))
    }
}

/// The aligned outlines of the morphs being played, kept across frames: aligning is the expensive
/// part, blending them at `t` is cheap.
#[derive(Debug, Default)]
pub(crate) struct Cache {
    /// With whether the current frame used it.
    morphs: Vec<(Source, GeometryMorph, bool)>,
}

impl Cache {
    /// The geometry of frame shape `s` at its current size `w` x `h`: both outlines are built at
    /// their own sizes (so the alignment does not change while the box grows), then scaled into the
    /// box. With a [`Fade`] per sub-path.
    pub(crate) fn geometry(&mut self, pm: &PathMorph, s: &Shape, w: f64, h: f64) -> (Geometry, Vec<Fade>) {
        let k = match self.morphs.iter().position(|(src, ..)| src.is(pm, s)) {
            Some(k) => k,
            None => {
                let src = Source::of(pm, s);
                let m = src.build();
                self.morphs.push((src, m, false));
                self.morphs.len() - 1
            }
        };
        let scale = |x: Xfrm| Vec2::new(ratio(w, x.w), ratio(h, x.h));
        let Some((_, m, used)) = self.morphs.get_mut(k) else {
            // Not reachable (k was just found or pushed); align afresh rather than panic.
            return Source::of(pm, s).build().at(pm.t, scale(pm.from_box), scale(pm.to_box));
        };
        *used = true;
        m.at(pm.t, scale(pm.from_box), scale(pm.to_box))
    }

    /// Ends a frame: forgets the morphs it did not use (their transition is over).
    pub(crate) fn sweep(&mut self) {
        self.morphs.retain(|(.., used)| *used);
        for (.., used) in &mut self.morphs {
            *used = false;
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.morphs.len()
    }
}
