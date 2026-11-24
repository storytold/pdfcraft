//! Slide transition frames without a GPU: the layers of [`deckcraft_anim::transition_layers`]
//! composited on the CPU from two rendered slide images. The slide show draws the same layers with
//! the UI toolkit's meshes; this is the headless twin used by the CLI, comparisons and tests.

use deckcraft_anim::{Layer, Source, layer_mesh, transition_frame, uses_slide_parts};
use deckcraft_model::{Presentation, ShapeId};

use crate::{Image, RenderOpts, ShapeState, SlidePart, render_blend, render_slide, render_slide_part};

/// The two slides of a transition rendered as background and content, for the transitions that
/// move contents over the backgrounds ([`deckcraft_anim::uses_slide_parts`]). `None` parts of the
/// old slide (the show's first slide) are black and nothing.
#[derive(Clone, Debug, Default)]
pub struct SlideParts {
    pub old_back: Option<Image>,
    pub old_content: Option<Image>,
    pub new_back: Image,
    pub new_content: Image,
}

impl SlideParts {
    /// Render the parts of slides `old` and `new`.
    pub fn render(pres: &Presentation, old: Option<usize>, new: usize, opts: &RenderOpts) -> SlideParts {
        SlideParts {
            old_back: old.map(|i| render_slide_part(pres, i, SlidePart::Backdrop, opts)),
            old_content: old.map(|i| render_slide_part(pres, i, SlidePart::Content, opts)),
            new_back: render_slide_part(pres, new, SlidePart::Backdrop, opts),
            new_content: render_slide_part(pres, new, SlidePart::Content, opts),
        }
    }
}

/// Composite transition `layers` over black into an image the size of `new`. `old` is the outgoing
/// slide (`None`: black, as when the show starts); `parts` the slides' backgrounds and contents
/// (without them, a background draws its whole slide and content draws nothing).
pub fn composite_transition(old: Option<&Image>, new: &Image, parts: Option<&SlideParts>, layers: &[Layer]) -> Image {
    let (w, h) = (new.width, new.height);
    let n = (w as usize).saturating_mul(h as usize);
    let mut out = Image { width: w, height: h, pixels: [0u8, 0, 0, 255].repeat(n) };
    for l in layers {
        let src = match (l.source.is_part(), parts) {
            (true, None) => match l.source.whole() {
                Some(s) => s,
                None => continue,
            },
            _ => l.source,
        };
        let tex = match (src, parts) {
            (Source::Old, _) => old.map(Tex::Image).unwrap_or(Tex::Solid([0.0, 0.0, 0.0])),
            (Source::New, _) => Tex::Image(new),
            (Source::Black, _) => Tex::Solid([0.0, 0.0, 0.0]),
            (Source::White, _) => Tex::Solid([255.0, 255.0, 255.0]),
            (Source::OldBack, Some(p)) => p.old_back.as_ref().map(Tex::Image).unwrap_or(Tex::Solid([0.0, 0.0, 0.0])),
            (Source::NewBack, Some(p)) => Tex::Image(&p.new_back),
            (Source::OldContent, Some(p)) => match &p.old_content {
                Some(i) => Tex::Image(i),
                None => continue,
            },
            (Source::NewContent, Some(p)) => Tex::Image(&p.new_content),
            (_, None) => continue,
        };
        draw_layer(&mut out, l, &tex);
    }
    out
}

/// The frame of slide `index`'s transition at progress `t` (0..1): the previous slide (black for
/// the first) going out, slide `index` coming in, at `opts`' scale and size. Morph frames are
/// rendered shape by shape like the show does.
pub fn render_transition(pres: &Presentation, index: usize, t: f64, opts: &RenderOpts) -> Image {
    let Some(slide) = pres.slides.get(index) else { return Image::default() };
    let (kind, option) = slide.transition.as_ref().map(|tr| (tr.kind.as_str(), tr.option.as_str())).unwrap_or(("none", ""));
    let prev = index.checked_sub(1).filter(|i| pres.slides.get(*i).is_some());
    if kind == "morph"
        && let Some(from) = prev
        && let Some(a) = pres.slides.get(from)
    {
        let tt = if t.is_finite() { t.clamp(0.0, 1.0) } else { 0.0 };
        if tt < 1.0 {
            let m = deckcraft_anim::morph_frame(pres, a, slide, tt, &|_| false);
            let f = |id: ShapeId| ShapeState { opacity: m.opacity_of(id), ..Default::default() };
            let o = RenderOpts { state: Some(&f), ..copy_opts(opts) };
            let text: Vec<crate::TextMorph> =
                m.text.iter().map(|x| crate::TextMorph { old: x.old, new: x.new, from: x.from, to: x.to, chars: x.chars, t: m.mix }).collect();
            let paths: Vec<crate::PathMorph> = m
                .paths
                .iter()
                .map(|x| crate::PathMorph { id: x.id, from: x.from.clone(), from_box: x.from_box, to_box: x.to_box, t: m.mix })
                .collect();
            return render_blend(pres, (from, index), m.mix, &m.shapes, &text, &paths, &o);
        }
    }
    let new = render_slide(pres, index, opts);
    let old = prev.map(|i| render_slide(pres, i, opts));
    let parts = uses_slide_parts(kind, option).then(|| SlideParts::render(pres, prev, index, opts));
    let aspect = pres.slide_size.width / pres.slide_size.height.max(1.0);
    composite_transition(old.as_ref(), &new, parts.as_ref(), &transition_frame(kind, option, t, aspect))
}

fn copy_opts<'a>(o: &RenderOpts<'a>) -> RenderOpts<'a> {
    RenderOpts {
        scale: o.scale,
        edit: o.edit,
        show_hidden: o.show_hidden,
        state: o.state,
        skip: o.skip,
        skip_text: o.skip_text,
        editing_text: o.editing_text,
        gif_times: o.gif_times,
        threads: o.threads,
        offset: o.offset,
        size: o.size,
        clear: o.clear,
        date_text: o.date_text.clone(),
        now: o.now,
        transparent_background: o.transparent_background,
    }
}

enum Tex<'a> {
    Image(&'a Image),
    /// Straight RGB, opaque.
    Solid([f32; 3]),
}

impl Tex<'_> {
    /// Premultiplied RGBA (0..255) at texture coordinate (u, v), bilinear, clamped to the edge.
    fn sample(&self, u: f64, v: f64) -> [f32; 4] {
        match self {
            Tex::Solid(c) => [c[0], c[1], c[2], 255.0],
            Tex::Image(img) => {
                let (w, h) = (img.width as usize, img.height as usize);
                if w == 0 || h == 0 {
                    return [0.0; 4];
                }
                let x = (u * w as f64 - 0.5).clamp(0.0, (w - 1) as f64);
                let y = (v * h as f64 - 0.5).clamp(0.0, (h - 1) as f64);
                let (x0, y0) = (x.floor() as usize, y.floor() as usize);
                let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
                let (fx, fy) = ((x - x0 as f64) as f32, (y - y0 as f64) as f32);
                let px = |xx: usize, yy: usize| -> [f32; 4] {
                    let i = (yy * w + xx) * 4;
                    match img.pixels.get(i..i + 4) {
                        Some(p) => [p[0] as f32, p[1] as f32, p[2] as f32, p[3] as f32],
                        None => [0.0; 4],
                    }
                };
                let (a, b, c, d) = (px(x0, y0), px(x1, y0), px(x0, y1), px(x1, y1));
                let mut o = [0.0f32; 4];
                for k in 0..4 {
                    let top = a[k] + (b[k] - a[k]) * fx;
                    let bot = c[k] + (d[k] - c[k]) * fx;
                    o[k] = top + (bot - top) * fy;
                }
                o
            }
        }
    }
}

fn draw_layer(out: &mut Image, l: &Layer, tex: &Tex) {
    if !(l.alpha.is_finite() && l.alpha > 0.0) {
        return;
    }
    let (verts, tris) = layer_mesh(l);
    let (w, h) = (out.width as f64, out.height as f64);
    for t in tris {
        let (Some(a), Some(b), Some(c)) = (verts.get(t[0] as usize), verts.get(t[1] as usize), verts.get(t[2] as usize)) else { continue };
        let p = [[a.pos[0] * w, a.pos[1] * h], [b.pos[0] * w, b.pos[1] * h], [c.pos[0] * w, c.pos[1] * h]];
        let uv = [a.uv, b.uv, c.uv];
        let al = [a.a, b.a, c.a].map(|x| if x.is_finite() { x.clamp(0.0, 1.0) } else { 0.0 });
        if al.iter().all(|x| *x <= 0.0) {
            continue;
        }
        fill_triangle(out, p, uv, al, tex);
    }
}

/// Rasterise one textured triangle: pixel centres inside it take the texture's colour at the
/// barycentric texture coordinate, at the interpolated opacity, blended source-over.
fn fill_triangle(out: &mut Image, p: [[f64; 2]; 3], uv: [[f64; 2]; 3], al: [f64; 3], tex: &Tex) {
    let area = (p[1][0] - p[0][0]) * (p[2][1] - p[0][1]) - (p[2][0] - p[0][0]) * (p[1][1] - p[0][1]);
    if !area.is_finite() || area.abs() < 1e-12 {
        return;
    }
    let (w, h) = (out.width as i64, out.height as i64);
    let minx = p.iter().map(|q| q[0]).fold(f64::INFINITY, f64::min).floor().max(0.0) as i64;
    let maxx = (p.iter().map(|q| q[0]).fold(f64::NEG_INFINITY, f64::max).ceil() as i64).min(w);
    let miny = p.iter().map(|q| q[1]).fold(f64::INFINITY, f64::min).floor().max(0.0) as i64;
    let maxy = (p.iter().map(|q| q[1]).fold(f64::NEG_INFINITY, f64::max).ceil() as i64).min(h);
    if minx >= maxx || miny >= maxy {
        return;
    }
    let edge = |a: [f64; 2], b: [f64; 2], x: f64, y: f64| (b[0] - a[0]) * (y - a[1]) - (b[1] - a[1]) * (x - a[0]);
    let width = out.width as usize;
    for y in miny..maxy {
        // Sample a hair off the pixel centre so a point on an edge shared by two triangles falls
        // in exactly one of them (no double-blended seams).
        let fy = y as f64 + 0.5 + 2.31e-6;
        for x in minx..maxx {
            let fx = x as f64 + 0.5 + 1.17e-6;
            let w0 = edge(p[1], p[2], fx, fy) / area;
            let w1 = edge(p[2], p[0], fx, fy) / area;
            let w2 = edge(p[0], p[1], fx, fy) / area;
            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                continue;
            }
            let u = w0 * uv[0][0] + w1 * uv[1][0] + w2 * uv[2][0];
            let v = w0 * uv[0][1] + w1 * uv[1][1] + w2 * uv[2][1];
            let alpha = (w0 * al[0] + w1 * al[1] + w2 * al[2]) as f32;
            if alpha <= 0.0 {
                continue;
            }
            let s = tex.sample(u, v);
            let i = (y as usize * width + x as usize) * 4;
            let Some(d) = out.pixels.get_mut(i..i + 4) else { continue };
            let sa = s[3] * alpha / 255.0;
            for k in 0..4 {
                let src = s[k] * alpha;
                let v = src + d[k] as f32 * (1.0 - sa);
                d[k] = v.round().clamp(0.0, 255.0) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, c: [u8; 4]) -> Image {
        Image { width: w, height: h, pixels: c.repeat((w * h) as usize) }
    }

    #[test]
    fn layers_composite_in_order() {
        let old = solid(40, 20, [255, 0, 0, 255]);
        let new = solid(40, 20, [0, 0, 255, 255]);
        let img = composite_transition(Some(&old), &new, None, &deckcraft_anim::transition_layers("cover", "l", 0.0));
        assert_eq!(img.pixel(20, 10), [255, 0, 0, 255]);
        let img = composite_transition(Some(&old), &new, None, &deckcraft_anim::transition_layers("cover", "l", 1.0));
        assert_eq!(img.pixel(20, 10), [0, 0, 255, 255]);
    }

    #[test]
    fn missing_old_slide_is_black() {
        let new = solid(10, 10, [0, 255, 0, 255]);
        let img = composite_transition(None, &new, None, &[Layer::full(Source::Old, 1.0)]);
        assert_eq!(img.pixel(5, 5), [0, 0, 0, 255]);
    }

    #[test]
    fn half_alpha_blends() {
        let old = solid(10, 10, [200, 0, 0, 255]);
        let new = solid(10, 10, [0, 0, 200, 255]);
        let l = [Layer::full(Source::Old, 1.0), Layer::full(Source::New, 0.5)];
        let p = composite_transition(Some(&old), &new, None, &l).pixel(5, 5);
        assert!((p[0] as i32 - 100).abs() <= 1 && (p[2] as i32 - 100).abs() <= 1, "{p:?}");
    }

    // ---------- golden-ish frames on synthetic slides ----------

    const W: u32 = 160;
    const H: u32 = 90;
    const OLD: [u8; 4] = [220, 40, 40, 255];
    const NEW: [u8; 4] = [30, 60, 200, 255];

    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Is {
        Old,
        New,
        Black,
        White,
        /// Something between (a blend).
        Mix,
    }

    fn classify(p: [u8; 4]) -> Is {
        let d = |c: [u8; 4]| (p[0] as i32 - c[0] as i32).abs() + (p[1] as i32 - c[1] as i32).abs() + (p[2] as i32 - c[2] as i32).abs();
        let near = 40;
        if d(OLD) < near {
            Is::Old
        } else if d(NEW) < near {
            Is::New
        } else if d([0, 0, 0, 255]) < near {
            Is::Black
        } else if d([255, 255, 255, 255]) < near {
            Is::White
        } else {
            // A face shaded in 3-D: a darker copy of one of the slides.
            let shaded = |c: [u8; 4]| {
                let dot = |a: [u8; 4], b: [u8; 4]| (0..3).map(|i| a[i] as f64 * b[i] as f64).sum::<f64>();
                let k = dot(p, c) / dot(c, c);
                (0.3..=1.05).contains(&k) && (0..3).all(|i| (p[i] as f64 - k * c[i] as f64).abs() < 14.0)
            };
            if shaded(OLD) {
                Is::Old
            } else if shaded(NEW) {
                Is::New
            } else {
                Is::Mix
            }
        }
    }

    fn frame(kind: &str, option: &str, t: f64) -> Image {
        let old = solid(W, H, OLD);
        let new = solid(W, H, NEW);
        composite_transition(Some(&old), &new, None, &transition_frame(kind, option, t, 16.0 / 9.0))
    }

    fn at(img: &Image, x: f64, y: f64) -> Is {
        let px = ((x * W as f64) as u32).min(W - 1);
        let py = ((y * H as f64) as u32).min(H - 1);
        classify(img.pixel(px, py))
    }

    /// (kind, option, t, x, y, what PowerPoint shows there), read off PowerPoint's own frames.
    const PROBES: &[(&str, &str, f64, f64, f64, Is)] = &[
        ("fade", "smoothly", 0.1, 0.5, 0.5, Is::Old),
        ("fade", "smoothly", 0.5, 0.5, 0.5, Is::Mix),
        ("fade", "smoothly", 0.9, 0.5, 0.5, Is::New),
        ("fade", "throughBlack", 0.5, 0.5, 0.5, Is::Black),
        ("cut", "throughBlack", 0.3, 0.5, 0.5, Is::Black),
        ("cut", "", 0.3, 0.5, 0.5, Is::New),
        // Push / Cover / Uncover: "l" moves things toward the left; the new slide comes from the right.
        ("push", "l", 0.25, 0.97, 0.5, Is::New),
        ("push", "l", 0.25, 0.1, 0.5, Is::Old),
        ("push", "r", 0.25, 0.03, 0.5, Is::New),
        ("push", "u", 0.25, 0.5, 0.97, Is::New),
        ("push", "d", 0.25, 0.5, 0.03, Is::New),
        ("cover", "l", 0.5, 0.75, 0.5, Is::New),
        ("cover", "l", 0.5, 0.25, 0.5, Is::Old),
        ("cover", "rd", 0.5, 0.05, 0.05, Is::New),
        ("cover", "rd", 0.2, 0.97, 0.97, Is::Old),
        ("uncover", "l", 0.5, 0.25, 0.5, Is::Old),
        ("uncover", "l", 0.5, 0.75, 0.5, Is::New),
        ("uncover", "lu", 0.5, 0.5, 0.5, Is::New),
        // Wipe "r" sweeps rightward with a soft edge; "u" upward; diagonals from a corner.
        ("wipe", "r", 0.5, 0.05, 0.5, Is::New),
        ("wipe", "r", 0.5, 0.97, 0.5, Is::Old),
        ("wipe", "l", 0.5, 0.97, 0.5, Is::New),
        ("wipe", "u", 0.5, 0.5, 0.97, Is::New),
        ("wipe", "u", 0.5, 0.5, 0.03, Is::Old),
        ("wipe", "d", 0.5, 0.5, 0.03, Is::New),
        ("wipe", "ru", 0.5, 0.03, 0.97, Is::New),
        ("wipe", "ru", 0.5, 0.97, 0.03, Is::Old),
        ("wipe", "r", 0.5, 0.55, 0.5, Is::Mix),
        // Split: Out opens from the middle, In closes from the edges.
        ("split", "horzOut", 0.5, 0.5, 0.5, Is::New),
        ("split", "horzOut", 0.5, 0.5, 0.02, Is::Old),
        ("split", "horzIn", 0.5, 0.5, 0.02, Is::New),
        ("split", "horzIn", 0.5, 0.5, 0.5, Is::Old),
        ("split", "vertOut", 0.5, 0.5, 0.5, Is::New),
        ("split", "vertOut", 0.5, 0.02, 0.5, Is::Old),
        ("split", "vertIn", 0.5, 0.02, 0.5, Is::New),
        // Shape: grows from the centre; In shrinks the old slide over the first half, Out grows
        // the new one over the second.
        ("shape", "circle", 0.5, 0.5, 0.5, Is::New),
        ("shape", "circle", 0.5, 0.02, 0.03, Is::Old),
        ("shape", "diamond", 0.5, 0.5, 0.5, Is::New),
        ("shape", "diamond", 0.5, 0.02, 0.03, Is::Old),
        ("shape", "plus", 0.5, 0.5, 0.05, Is::New),
        ("shape", "plus", 0.5, 0.05, 0.05, Is::Old),
        ("shape", "in", 0.25, 0.5, 0.5, Is::Old),
        ("shape", "in", 0.25, 0.05, 0.05, Is::New),
        ("shape", "in", 0.75, 0.5, 0.5, Is::New),
        ("shape", "out", 0.25, 0.5, 0.5, Is::Old),
        ("shape", "out", 0.75, 0.5, 0.5, Is::New),
        ("shape", "out", 0.75, 0.05, 0.05, Is::Old),
        // Flash: white in the middle.
        ("flash", "", 0.3, 0.5, 0.5, Is::White),
        // Clock: a hand sweeping from 12 o'clock.
        ("clock", "clockwise", 0.25, 0.6, 0.15, Is::New),
        ("clock", "clockwise", 0.25, 0.4, 0.15, Is::Old),
        ("clock", "counterClockwise", 0.25, 0.4, 0.15, Is::New),
        ("clock", "counterClockwise", 0.25, 0.6, 0.15, Is::Old),
        ("clock", "wedge", 0.5, 0.5, 0.05, Is::New),
        ("clock", "wedge", 0.5, 0.5, 0.95, Is::Old),
        // Ripple: from the centre, or from the corner opposite the option.
        ("ripple", "center", 0.5, 0.5, 0.5, Is::New),
        ("ripple", "center", 0.5, 0.02, 0.03, Is::Old),
        ("ripple", "ru", 0.4, 0.03, 0.97, Is::New),
        ("ripple", "ru", 0.4, 0.97, 0.03, Is::Old),
        // Comb: the old slide's bands slide off, the bottom band first.
        ("comb", "horz", 0.5, 0.5, 0.97, Is::New),
        ("comb", "horz", 0.5, 0.5, 0.03, Is::Old),
        // Glitter "l": the sweep starts on the left.
        ("glitter", "l", 0.45, 0.03, 0.5, Is::New),
        ("glitter", "l", 0.45, 0.97, 0.5, Is::Old),
        ("glitter", "r", 0.45, 0.97, 0.5, Is::New),
        // Vortex "l": the old slide whirls off toward the left, its right side first.
        ("vortex", "l", 0.25, 0.03, 0.5, Is::Old),
        ("vortex", "l", 0.97, 0.03, 0.5, Is::New),
        // Cube / Box "l": the new face comes in from the right.
        ("cube", "l", 0.5, 0.85, 0.5, Is::New),
        ("cube", "l", 0.5, 0.15, 0.5, Is::Old),
        ("cube", "r", 0.5, 0.15, 0.5, Is::New),
        ("box", "l", 0.25, 0.97, 0.5, Is::New),
        ("box", "l", 0.25, 0.3, 0.5, Is::Old),
        ("cube", "u", 0.5, 0.5, 0.9, Is::New),
        ("cube", "d", 0.5, 0.5, 0.1, Is::New),
        // Switch "l": the old card is pulled out to the left, the new one to the right.
        ("switch", "l", 0.25, 0.97, 0.5, Is::New),
        ("switch", "l", 0.25, 0.3, 0.5, Is::Old),
        ("switch", "r", 0.25, 0.03, 0.5, Is::New),
        // Gallery "l": the slides travel left along the wall.
        ("gallery", "l", 0.6, 0.9, 0.5, Is::New),
        // Flip: the old face, then the new one.
        ("flip", "l", 0.2, 0.5, 0.5, Is::Old),
        ("flip", "l", 0.8, 0.5, 0.5, Is::New),
        // Doors open in the middle, the new slide coming forward between them.
        ("doors", "vert", 0.9, 0.5, 0.5, Is::New),
        // Fall Over: the old slide tips backward, uncovering the new one at the top first.
        ("fallOver", "l", 0.6, 0.5, 0.03, Is::New),
        ("fallOver", "l", 0.6, 0.5, 0.97, Is::Old),
        // Peel Off "l": from the bottom-right corner.
        ("peelOff", "l", 0.4, 0.98, 0.98, Is::New),
        ("peelOff", "l", 0.4, 0.03, 0.03, Is::Old),
        ("peelOff", "r", 0.4, 0.02, 0.98, Is::New),
        // Page Curl: the far half rolls up first.
        ("pageCurlDouble", "l", 0.5, 0.9, 0.5, Is::New),
        ("pageCurlDouble", "l", 0.5, 0.1, 0.5, Is::Old),
        ("pageCurlDouble", "r", 0.5, 0.1, 0.5, Is::New),
        ("pageCurlDouble", "singleL", 0.6, 0.95, 0.5, Is::New),
        ("pageCurlDouble", "singleL", 0.6, 0.05, 0.5, Is::Old),
        // Airplane "l": a dart with its nose to the right, hanging low.
        ("airplane", "l", 0.5, 0.15, 0.1, Is::New),
        ("airplane", "l", 0.5, 0.8, 0.68, Is::Old),
        ("airplane", "r", 0.5, 0.2, 0.68, Is::Old),
        // Origami "l": the bird flies off to the left.
        ("origami", "l", 0.75, 0.97, 0.5, Is::New),
        // Fracture: shards fall, the top first.
        ("fracture", "", 0.6, 0.5, 0.05, Is::New),
        // Crush: the old slide crumples toward the centre.
        ("crush", "", 0.5, 0.03, 0.03, Is::New),
        ("crush", "", 0.5, 0.45, 0.5, Is::Old),
        // Curtains part in the middle.
        ("curtains", "", 0.4, 0.5, 0.5, Is::New),
        ("curtains", "", 0.4, 0.02, 0.5, Is::Old),
        // Zoom and Rotate: the new slide spins in from the centre over the old one.
        ("zoom", "rotate", 0.2, 0.03, 0.03, Is::Old),
        ("zoom", "out", 0.8, 0.5, 0.5, Is::New),
        ("dissolve", "", 0.02, 0.5, 0.5, Is::Old),
        ("dissolve", "", 0.99, 0.5, 0.5, Is::New),
    ];

    #[test]
    fn frames_match_powerpoint_probes() {
        let mut bad = Vec::new();
        for (kind, option, t, x, y, want) in PROBES {
            let got = at(&frame(kind, option, *t), *x, *y);
            if got != *want {
                bad.push(format!("{kind}/{option} t={t} ({x},{y}): want {want:?}, got {got:?}"));
            }
        }
        assert!(bad.is_empty(), "{}", bad.join("\n"));
    }

    #[test]
    fn every_transition_starts_old_and_ends_new() {
        for (kind, _, _, _, opts) in deckcraft_model::anim::TRANSITIONS {
            let mut options: Vec<&str> = opts.to_vec();
            options.push("");
            for o in options {
                assert_eq!(at(&frame(kind, o, 0.0), 0.5, 0.5), Is::Old, "{kind}/{o}");
                assert_eq!(at(&frame(kind, o, 1.0), 0.5, 0.5), Is::New, "{kind}/{o}");
                for t in [0.25, 0.5, 0.75] {
                    let img = frame(kind, o, t);
                    assert_eq!((img.width, img.height), (W, H));
                }
            }
        }
    }

    #[test]
    fn dynamic_content_moves_contents_over_backgrounds() {
        // Old: red background with a white content square in the middle; new: blue background
        // with a yellow square.
        let content = |c: [u8; 4]| {
            let mut img = solid(W, H, [0, 0, 0, 0]);
            for y in 35..55u32 {
                for x in 70..90u32 {
                    let i = ((y * W + x) * 4) as usize;
                    if let Some(p) = img.pixels.get_mut(i..i + 4) {
                        p.copy_from_slice(&c);
                    }
                }
            }
            img
        };
        let parts = SlideParts {
            old_back: Some(solid(W, H, OLD)),
            old_content: Some(content([255, 255, 255, 255])),
            new_back: solid(W, H, NEW),
            new_content: content([255, 220, 0, 255]),
        };
        let old = solid(W, H, OLD);
        let new = solid(W, H, NEW);
        let f = |kind: &str, o: &str, t: f64| composite_transition(Some(&old), &new, Some(&parts), &transition_frame(kind, o, t, 16.0 / 9.0));
        // Pan left at the middle: the backgrounds blend, the old square has moved left.
        let img = f("pan", "l", 0.5);
        let bg = img.pixel(5, 5);
        assert!(bg[0] > 60 && bg[2] > 60, "{bg:?}");
        assert_eq!(img.pixel(80, 45), bg, "the middle shows background only");
        // Pan right: the new square arrives from the left.
        let img = f("pan", "r", 0.6);
        assert!((0..80).any(|x| {
            let p = img.pixel(x, 45);
            p[0] > 200 && p[1] > 180 && p[2] < 80
        }));
        // Reveal: the new content is in place near the end.
        let p = f("reveal", "smoothlyLeft", 0.97).pixel(80, 45);
        assert!(p[0] > 200 && p[1] > 180, "{p:?}");
        // Without parts the dynamic transitions still draw whole slides.
        let img = composite_transition(Some(&old), &new, None, &transition_frame("pan", "l", 0.5, 16.0 / 9.0));
        assert_ne!(img.pixel(80, 45), [0, 0, 0, 255]);
    }
}
