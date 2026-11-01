//! The geometry of a page transition (ISO 32000-2 §12.4.4, table 164) part-way through its run:
//! which parts of the outgoing and the incoming page a presentation paints, where, and how
//! opaque. Pure functions in page fractions (0 to 1 across and down the page box on screen), so
//! the painter only scales them; nothing here touches pixels or time.
//!
//! `Effect` is the transition as a presentation plays it (the `/Trans` entries that matter for
//! each style, with the spec's defaults), `frame` the picture at progress `t`. Every frame starts
//! with the outgoing page alone (`t = 0`) and ends with the incoming page alone (`t = 1`); the
//! pieces of a frame never leave the page box.

/// A page transition as it is played: the style with the entries that style reads. Directions
/// are degrees as `/Di` gives them: 0 left to right, 90 bottom to top, 180 right to left, 270 top
/// to bottom, 315 top-left to bottom-right (Glitter; Wipe treats it the same way).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Effect {
    /// Two halves split along a horizontal (`horizontal`) or vertical line, revealing the new page
    /// from the edges inward (`inward`) or from the centre outward.
    Split { horizontal: bool, inward: bool },
    /// Strips across (`horizontal`) or down the page open together, like Venetian blinds.
    Blinds { horizontal: bool },
    /// A rectangle grows from the centre (`inward` false) or the edges close in on the centre.
    Box { inward: bool },
    /// A line sweeps across the page in `direction`, revealing the new page behind it.
    Wipe { direction: u16 },
    /// Random squares of the new page appear until it is whole.
    Dissolve,
    /// Like Dissolve, but the squares appear in a band that sweeps in `direction`.
    Glitter { direction: u16 },
    /// No effect: the new page replaces the old one.
    Replace,
    /// The new page flies in from off-screen in `direction` (or from the centre, `None`), growing
    /// from `scale`; `outward` flies the old page off instead. With `opaque` the moving page hides
    /// what is under it (it always does here: the pieces overlap in paint order).
    Fly { direction: Option<u16>, inward: bool, scale: f32 },
    /// The new page pushes the old one off the page in `direction`.
    Push { direction: u16 },
    /// The new page slides in over the old one in `direction`.
    Cover { direction: u16 },
    /// The old page slides off in `direction`, uncovering the new one beneath.
    Uncover { direction: u16 },
    /// The new page fades in over the old one.
    Fade,
}

impl Effect {
    /// The effect for a `/Trans` dictionary read as its parts: the `/S` name, `/Dm` as
    /// `horizontal`, `/M` as `inward`, `/Di` as `direction` (`None` for `/None`), `/SS` and `/B`.
    /// An unknown style is `Replace`, as the spec's default; an entry a style does not read is
    /// ignored.
    pub fn from_parts(style: &str, horizontal: bool, inward: bool, direction: Option<u16>, scale: f32) -> Self {
        let dir = direction.filter(|d| matches!(d, 0 | 90 | 180 | 270 | 315)).unwrap_or(0);
        match style {
            "Split" => Self::Split { horizontal, inward },
            "Blinds" => Self::Blinds { horizontal },
            "Box" => Self::Box { inward },
            "Wipe" => Self::Wipe { direction: dir },
            "Dissolve" => Self::Dissolve,
            "Glitter" => Self::Glitter { direction: dir },
            "Fly" => Self::Fly {
                direction: direction.filter(|d| matches!(d, 0 | 90 | 180 | 270)),
                inward,
                scale: if scale.is_finite() && scale > 0.0 { scale.min(1.0) } else { 1.0 },
            },
            "Push" => Self::Push { direction: dir },
            "Cover" => Self::Cover { direction: dir },
            "Uncover" => Self::Uncover { direction: dir },
            "Fade" => Self::Fade,
            _ => Self::Replace,
        }
    }
}

/// A part of a page to paint: `src` is the part of that page, `dst` where it goes, both as
/// fractions `[x0, y0, x1, y1]` of the page box (y down), `alpha` its opacity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Piece {
    pub src: [f32; 4],
    pub dst: [f32; 4],
    pub alpha: f32,
}

/// One moment of a transition: the pieces of the outgoing page, then of the incoming page, in
/// paint order (later pieces over earlier ones).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Frame {
    pub outgoing: Vec<Piece>,
    pub incoming: Vec<Piece>,
}

/// Strips in Blinds, and cells per side in Dissolve and Glitter.
const BLINDS: usize = 8;
const GRID: usize = 8;
/// Strips that approximate a diagonal sweep.
const DIAGONAL_STRIPS: usize = 16;
const FULL: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

fn whole(alpha: f32) -> Piece {
    Piece { src: FULL, dst: FULL, alpha }
}

/// A piece of the page at `rect` (fractions), shown in place.
fn at(rect: [f32; 4]) -> Piece {
    Piece { src: rect, dst: rect, alpha: 1.0 }
}

/// The whole page shifted by `dx, dy` (fractions), clipped to the page box, or `None` when it is
/// entirely off the page.
fn shifted(dx: f32, dy: f32) -> Option<Piece> {
    let dst = [dx.max(0.0), dy.max(0.0), (1.0 + dx).min(1.0), (1.0 + dy).min(1.0)];
    if dst[2] - dst[0] <= 0.0 || dst[3] - dst[1] <= 0.0 {
        return None;
    }
    let src = [dst[0] - dx, dst[1] - dy, dst[2] - dx, dst[3] - dy];
    Some(Piece { src, dst, alpha: 1.0 })
}

/// Where a page is at `k` of the way along `direction` (0 at the start, 1 at the end of a move
/// across the whole page), as an offset in fractions.
fn offset(direction: u16, k: f32) -> (f32, f32) {
    match direction {
        90 => (0.0, -k),
        180 => (-k, 0.0),
        270 => (0.0, k),
        _ => (k, 0.0),
    }
}

/// A small deterministic hash, so Dissolve and Glitter look random but repeat exactly.
fn hash(i: u32) -> u32 {
    let mut x = i.wrapping_mul(0x9E37_79B9).wrapping_add(0x7F4A_7C15);
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    x
}

/// The cells of the dissolve grid in the order they appear: by `key`, ties by position.
fn cells_in_order(key: impl Fn(usize, usize) -> f32) -> Vec<[f32; 4]> {
    let mut cells: Vec<(f32, usize, usize)> = (0..GRID * GRID).map(|i| (key(i % GRID, i / GRID), i / GRID, i % GRID)).collect();
    cells.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    let side = 1.0 / GRID as f32;
    cells.into_iter().map(|(_, row, col)| [col as f32 * side, row as f32 * side, (col + 1) as f32 * side, (row + 1) as f32 * side]).collect()
}

/// The pieces of the incoming page at `t` for a diagonal sweep: a point is revealed once its
/// distance along the diagonal, `(x + y) / 2`, is within `t`, approximated by vertical strips.
fn diagonal(t: f32) -> Vec<Piece> {
    let w = 1.0 / DIAGONAL_STRIPS as f32;
    (0..DIAGONAL_STRIPS)
        .filter_map(|k| {
            let x0 = k as f32 * w;
            let h = (2.0 * t - (x0 + w / 2.0)).clamp(0.0, 1.0);
            (h > 0.0).then(|| at([x0, 0.0, x0 + w, h]))
        })
        .collect()
}

/// The incoming page revealed by a straight sweep in `direction` at `t`.
fn swept(direction: u16, t: f32) -> Vec<Piece> {
    match direction {
        90 => vec![at([0.0, 1.0 - t, 1.0, 1.0])],
        180 => vec![at([1.0 - t, 0.0, 1.0, 1.0])],
        270 => vec![at([0.0, 0.0, 1.0, t])],
        315 => diagonal(t),
        _ => vec![at([0.0, 0.0, t, 1.0])],
    }
}

/// The picture of `effect` at progress `t` (0: the outgoing page alone; 1: the incoming page
/// alone; anything else clamped). Pieces are in paint order and never leave the page box.
pub fn frame(effect: &Effect, t: f32) -> Frame {
    let t = if t.is_finite() { t.clamp(0.0, 1.0) } else { 1.0 };
    if t <= 0.0 {
        return Frame { outgoing: vec![whole(1.0)], incoming: Vec::new() };
    }
    if t >= 1.0 {
        return Frame { outgoing: Vec::new(), incoming: vec![whole(1.0)] };
    }
    let over_old = |incoming: Vec<Piece>| Frame { outgoing: vec![whole(1.0)], incoming };
    match *effect {
        Effect::Replace => Frame { outgoing: Vec::new(), incoming: vec![whole(1.0)] },
        Effect::Fade => Frame { outgoing: vec![whole(1.0)], incoming: vec![whole(t)] },
        Effect::Wipe { direction } => over_old(swept(direction, t)),
        Effect::Split { horizontal, inward } => {
            let h = t / 2.0;
            let bands = match (horizontal, inward) {
                (true, true) => vec![at([0.0, 0.0, 1.0, h]), at([0.0, 1.0 - h, 1.0, 1.0])],
                (true, false) => vec![at([0.0, 0.5 - h, 1.0, 0.5 + h])],
                (false, true) => vec![at([0.0, 0.0, h, 1.0]), at([1.0 - h, 0.0, 1.0, 1.0])],
                (false, false) => vec![at([0.5 - h, 0.0, 0.5 + h, 1.0])],
            };
            over_old(bands)
        }
        Effect::Blinds { horizontal } => {
            let w = 1.0 / BLINDS as f32;
            let strips = (0..BLINDS)
                .map(|k| {
                    let a = k as f32 * w;
                    if horizontal { at([0.0, a, 1.0, a + t * w]) } else { at([a, 0.0, a + t * w, 1.0]) }
                })
                .collect();
            over_old(strips)
        }
        Effect::Box { inward } => {
            let h = t / 2.0;
            let pieces = if inward {
                vec![at([0.0, 0.0, 1.0, h]), at([0.0, 1.0 - h, 1.0, 1.0]), at([0.0, h, h, 1.0 - h]), at([1.0 - h, h, 1.0, 1.0 - h])]
            } else {
                vec![at([0.5 - h, 0.5 - h, 0.5 + h, 0.5 + h])]
            };
            over_old(pieces)
        }
        Effect::Dissolve => {
            let cells = cells_in_order(|col, row| hash((row * GRID + col) as u32) as f32);
            let shown = ((t * cells.len() as f32).round() as usize).min(cells.len());
            over_old(cells.into_iter().take(shown).map(at).collect())
        }
        Effect::Glitter { direction } => {
            // Ordered by how far along the sweep a cell lies, jittered so the band sparkles.
            let along = |col: usize, row: usize| -> f32 {
                let (x, y) = ((col as f32 + 0.5) / GRID as f32, (row as f32 + 0.5) / GRID as f32);
                match direction {
                    90 => 1.0 - y,
                    180 => 1.0 - x,
                    270 => y,
                    315 => (x + y) / 2.0,
                    _ => x,
                }
            };
            let cells = cells_in_order(|col, row| along(col, row) + (hash((row * GRID + col) as u32 + 977) % 1000) as f32 / 4000.0);
            let shown = ((t * cells.len() as f32).round() as usize).min(cells.len());
            over_old(cells.into_iter().take(shown).map(at).collect())
        }
        Effect::Push { direction } => {
            let (dx, dy) = offset(direction, t);
            let (ux, uy) = offset(direction, 1.0);
            Frame { outgoing: shifted(dx, dy).into_iter().collect(), incoming: shifted(dx - ux, dy - uy).into_iter().collect() }
        }
        Effect::Cover { direction } => {
            let (dx, dy) = offset(direction, t);
            let (ux, uy) = offset(direction, 1.0);
            over_old(shifted(dx - ux, dy - uy).into_iter().collect())
        }
        Effect::Uncover { direction } => {
            let (dx, dy) = offset(direction, t);
            Frame { outgoing: shifted(dx, dy).into_iter().collect(), incoming: vec![whole(1.0)] }
        }
        Effect::Fly { direction, inward, scale } => {
            // The moving page: at `k` of its flight (0 off-screen or small, 1 whole and in place).
            // `leaving`: the page moves on in `direction` (flying out); else it comes in from the
            // side it would have left from (flying in).
            let flight = |k: f32, leaving: bool| -> Option<Piece> {
                let s = scale + (1.0 - scale) * k;
                let (dx, dy) = direction.map_or((0.0, 0.0), |d| {
                    let (ox, oy) = offset(d, 1.0 - k);
                    if leaving { (ox, oy) } else { (-ox, -oy) }
                });
                // The page at scale `s`, centred, shifted by (dx, dy), clipped to the page box.
                let half = s / 2.0;
                let dst = [0.5 - half + dx, 0.5 - half + dy, 0.5 + half + dx, 0.5 + half + dy];
                let clip = [dst[0].max(0.0), dst[1].max(0.0), dst[2].min(1.0), dst[3].min(1.0)];
                if clip[2] - clip[0] <= 0.0 || clip[3] - clip[1] <= 0.0 || s <= 0.0 {
                    return None;
                }
                let src = [(clip[0] - dst[0]) / s, (clip[1] - dst[1]) / s, (clip[2] - dst[0]) / s, (clip[3] - dst[1]) / s];
                Some(Piece { src, dst: clip, alpha: 1.0 })
            };
            if inward {
                Frame { outgoing: vec![whole(1.0)], incoming: flight(t, false).into_iter().collect() }
            } else {
                Frame { outgoing: flight(1.0 - t, true).into_iter().collect(), incoming: vec![whole(1.0)] }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(p: &Piece) -> f32 {
        (p.dst[2] - p.dst[0]).max(0.0) * (p.dst[3] - p.dst[1]).max(0.0)
    }

    fn inside(p: &Piece) -> bool {
        p.dst.iter().chain(p.src.iter()).all(|v| (-1e-4..=1.0 + 1e-4).contains(v)) && p.dst[0] <= p.dst[2] && p.dst[1] <= p.dst[3]
    }

    fn all_effects() -> Vec<Effect> {
        let mut v = vec![
            Effect::Replace,
            Effect::Fade,
            Effect::Dissolve,
            Effect::Split { horizontal: true, inward: true },
            Effect::Split { horizontal: false, inward: false },
            Effect::Blinds { horizontal: true },
            Effect::Blinds { horizontal: false },
            Effect::Box { inward: true },
            Effect::Box { inward: false },
        ];
        for d in [0, 90, 180, 270, 315] {
            v.push(Effect::Wipe { direction: d });
            v.push(Effect::Glitter { direction: d });
        }
        for d in [0, 90, 180, 270] {
            v.push(Effect::Push { direction: d });
            v.push(Effect::Cover { direction: d });
            v.push(Effect::Uncover { direction: d });
            v.push(Effect::Fly { direction: Some(d), inward: true, scale: 0.5 });
            v.push(Effect::Fly { direction: Some(d), inward: false, scale: 1.0 });
        }
        v.push(Effect::Fly { direction: None, inward: true, scale: 0.25 });
        v
    }

    #[test]
    fn every_effect_starts_with_the_old_page_ends_with_the_new_and_stays_on_the_page() {
        for e in all_effects() {
            let start = frame(&e, 0.0);
            assert_eq!((start.outgoing.len(), start.incoming.len()), (1, 0), "{e:?} at 0");
            assert_eq!(start.outgoing[0], whole(1.0));
            let end = frame(&e, 1.0);
            assert_eq!((end.outgoing.len(), end.incoming.len()), (0, 1), "{e:?} at 1");
            assert_eq!(end.incoming[0], whole(1.0));
            assert_eq!(frame(&e, f32::NAN), end, "{e:?}: a non-number is the end");
            assert_eq!(frame(&e, -3.0), start);
            for k in 1..20 {
                let f = frame(&e, k as f32 / 20.0);
                for p in f.outgoing.iter().chain(f.incoming.iter()) {
                    assert!(inside(p), "{e:?} at {k}/20: {p:?} leaves the page");
                    assert!((0.0..=1.0).contains(&p.alpha));
                }
            }
        }
    }

    #[test]
    fn reveals_grow_with_time() {
        // For every effect that reveals the new page in place, the revealed area never shrinks.
        for e in all_effects().into_iter().filter(|e| {
            !matches!(e, Effect::Replace | Effect::Fade | Effect::Push { .. } | Effect::Cover { .. } | Effect::Uncover { .. } | Effect::Fly { .. })
        }) {
            let mut last = 0.0;
            for k in 0..=20 {
                let f = frame(&e, k as f32 / 20.0);
                let a: f32 = f.incoming.iter().map(area).sum();
                assert!(a + 1e-4 >= last, "{e:?}: revealed area fell from {last} to {a} at {k}/20");
                last = a;
            }
            assert!((last - 1.0).abs() < 1e-4, "{e:?} ends whole: {last}");
        }
    }

    #[test]
    fn wipes_sweep_in_their_direction() {
        let f = frame(&Effect::Wipe { direction: 0 }, 0.25);
        assert_eq!(f.incoming, vec![at([0.0, 0.0, 0.25, 1.0])], "left to right");
        assert_eq!(frame(&Effect::Wipe { direction: 90 }, 0.25).incoming, vec![at([0.0, 0.75, 1.0, 1.0])], "bottom to top");
        assert_eq!(frame(&Effect::Wipe { direction: 180 }, 0.25).incoming, vec![at([0.75, 0.0, 1.0, 1.0])], "right to left");
        assert_eq!(frame(&Effect::Wipe { direction: 270 }, 0.25).incoming, vec![at([0.0, 0.0, 1.0, 0.25])], "top to bottom");
        // Diagonal: the top-left corner first, the bottom-right last.
        let d = frame(&Effect::Wipe { direction: 315 }, 0.25);
        assert!(!d.incoming.is_empty() && d.incoming[0].dst[0] == 0.0 && d.incoming[0].dst[3] > 0.0, "{:?}", d.incoming);
        assert!(d.incoming.iter().all(|p| p.dst[0] < 0.5), "nothing past the middle yet: {:?}", d.incoming);
    }

    #[test]
    fn split_box_and_blinds_open_where_they_say() {
        let h = frame(&Effect::Split { horizontal: true, inward: true }, 0.5).incoming;
        assert_eq!(h, vec![at([0.0, 0.0, 1.0, 0.25]), at([0.0, 0.75, 1.0, 1.0])], "from the top and bottom edges");
        let v = frame(&Effect::Split { horizontal: false, inward: false }, 0.5).incoming;
        assert_eq!(v, vec![at([0.25, 0.0, 0.75, 1.0])], "a vertical band from the centre");
        let b = frame(&Effect::Box { inward: false }, 0.5).incoming;
        assert_eq!(b, vec![at([0.25, 0.25, 0.75, 0.75])]);
        let bi = frame(&Effect::Box { inward: true }, 0.5).incoming;
        assert_eq!(bi.len(), 4);
        assert!((bi.iter().map(area).sum::<f32>() - 0.75).abs() < 1e-5, "a 0.5 hole is left in the middle");
        let blinds = frame(&Effect::Blinds { horizontal: true }, 0.5).incoming;
        assert_eq!(blinds.len(), BLINDS);
        assert_eq!(blinds[1], at([0.0, 0.125, 1.0, 0.1875]), "each strip opens from its top");
    }

    #[test]
    fn dissolve_and_glitter_show_a_share_of_cells_in_a_fixed_order() {
        let half = frame(&Effect::Dissolve, 0.5).incoming;
        assert_eq!(half.len(), GRID * GRID / 2);
        assert_eq!(frame(&Effect::Dissolve, 0.5).incoming, half, "the same every time");
        let quarter = frame(&Effect::Dissolve, 0.25).incoming;
        assert_eq!(&half[..quarter.len()], &quarter[..], "cells only ever join");
        assert!(half.windows(2).any(|w| w[0].dst[1] != w[1].dst[1] || w[0].dst[0] + 0.125 != w[1].dst[0]), "not in reading order");
        // Glitter sweeps: at a third, every cell shown is in the left half for direction 0.
        let g = frame(&Effect::Glitter { direction: 0 }, 0.3).incoming;
        assert!(!g.is_empty() && g.iter().all(|p| p.dst[0] < 0.5), "{g:?}");
        let g = frame(&Effect::Glitter { direction: 270 }, 0.3).incoming;
        assert!(g.iter().all(|p| p.dst[1] < 0.5), "top first: {g:?}");
    }

    #[test]
    fn push_cover_uncover_and_fly_move_whole_pages() {
        let f = frame(&Effect::Push { direction: 0 }, 0.25);
        assert_eq!(f.outgoing, vec![Piece { src: [0.0, 0.0, 0.75, 1.0], dst: [0.25, 0.0, 1.0, 1.0], alpha: 1.0 }], "the old page moves right");
        assert_eq!(
            f.incoming,
            vec![Piece { src: [0.75, 0.0, 1.0, 1.0], dst: [0.0, 0.0, 0.25, 1.0], alpha: 1.0 }],
            "the new page comes in from the left"
        );
        let f = frame(&Effect::Push { direction: 270 }, 0.5);
        assert_eq!(f.incoming[0].dst, [0.0, 0.0, 1.0, 0.5], "top to bottom");
        let c = frame(&Effect::Cover { direction: 180 }, 0.25);
        assert_eq!(
            (c.outgoing, c.incoming[0].dst),
            (vec![whole(1.0)], [0.75, 0.0, 1.0, 1.0]),
            "the old page stays, the new slides in from the right"
        );
        let u = frame(&Effect::Uncover { direction: 90 }, 0.25);
        assert_eq!((u.incoming, u.outgoing[0].dst), (vec![whole(1.0)], [0.0, 0.0, 1.0, 0.75]), "the old page slides up off the new one");
        // Fly in from the centre: a quarter-size page in the middle at the start, growing.
        let f = frame(&Effect::Fly { direction: None, inward: true, scale: 0.25 }, 0.5);
        let p = f.incoming[0];
        assert_eq!(p.src, FULL, "the whole page, small");
        assert!((p.dst[2] - p.dst[0] - 0.625).abs() < 1e-5 && (p.dst[0] - 0.1875).abs() < 1e-5, "{p:?}");
        // Fly in from the left: off-page at the start, so nothing to paint yet.
        assert!(frame(&Effect::Fly { direction: Some(0), inward: true, scale: 1.0 }, 1e-3).incoming.first().is_some_and(|p| p.dst[2] < 0.01));
        // Fly out: the old page leaves to the right over the new page.
        let f = frame(&Effect::Fly { direction: Some(0), inward: false, scale: 1.0 }, 0.5);
        assert_eq!((f.incoming, f.outgoing[0].dst), (vec![whole(1.0)], [0.5, 0.0, 1.0, 1.0]));
    }

    #[test]
    fn effects_come_from_trans_parts_with_the_defaults() {
        assert_eq!(Effect::from_parts("Wipe", true, true, Some(270), 1.0), Effect::Wipe { direction: 270 });
        assert_eq!(
            Effect::from_parts("Wipe", true, true, Some(45), 1.0),
            Effect::Wipe { direction: 0 },
            "a direction outside the five is the default"
        );
        assert_eq!(Effect::from_parts("Split", false, false, Some(90), 1.0), Effect::Split { horizontal: false, inward: false });
        assert_eq!(Effect::from_parts("Fly", true, true, None, 0.5), Effect::Fly { direction: None, inward: true, scale: 0.5 });
        assert_eq!(
            Effect::from_parts("Fly", true, false, Some(315), f32::NAN),
            Effect::Fly { direction: None, inward: false, scale: 1.0 },
            "Fly has no diagonal; a bad scale is 1"
        );
        assert_eq!(Effect::from_parts("Swirl", true, true, Some(0), 1.0), Effect::Replace);
        assert_eq!(Effect::from_parts("R", true, true, Some(0), 1.0), Effect::Replace);
    }
}
