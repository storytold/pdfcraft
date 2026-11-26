//! Contributor-original pixel goldens for #1596 and the review of #1626.
//! The fixed samples cover background, hole/centre, ring, visible halos and reflected coverage.

use super::{Fill, Image, RenderOpts, Rgba, render_slide};
use deckcraft_model::style::{ColorRef, Effects, Glow, Line, Reflection, Shadow};
use deckcraft_model::{Geom, Presentation, Shape, ShapeId, Xfrm};

const PROBES: [(u32, u32); 9] = [
    (20, 20),   // Clear background.
    (130, 130), // Clear donut hole; filled ellipse centre.
    (130, 70),  // Original ring/ellipse fill.
    (130, 47),  // Visible shadow/glow above the outer edge.
    (130, 213), // Visible shadow/glow below the outer edge.
    (130, 93),  // Shadow/glow along the inner rim, inside the hole.
    (130, 230), // Reflected upper ring.
    (130, 300), // Reflected hole/ellipse centre.
    (70, 300),  // Reflected ring at the same fade as the hole.
];

#[derive(Clone, Copy, Debug)]
enum Effect {
    Shadow,
    Glow,
    Reflection,
}

fn scene(shape: &str, effect: Effect) -> Presentation {
    let mut p = Presentation::default();
    let mut effects = Effects::default();
    match effect {
        Effect::Shadow => {
            effects.outer_shadow = Some(Shadow {
                color: ColorRef::rgb(Rgba::BLACK),
                blur: 8.0,
                dist: 0.0,
                dir: 45.0,
                inner: false,
                sx: 1.0,
                sy: 1.0,
                kx: 0.0,
                ky: 0.0,
                align: String::new(),
                rotate_with_shape: false,
            })
        }
        Effect::Glow => effects.glow = Some(Glow { color: ColorRef::rgb(Rgba::rgb(255, 0, 0)), radius: 8.0 }),
        Effect::Reflection => {
            effects.reflection = Some(Reflection { blur: 0.0, start_alpha: 1.0, end_alpha: 0.0, end_pos: 1.0, dist: 10.0, ..Default::default() })
        }
    }
    let slide = std::sync::Arc::make_mut(&mut p.slides[0]);
    slide.background = Some(deckcraft_model::Background::Fill { fill: Fill::solid(ColorRef::rgb(Rgba::WHITE)) });
    slide.shapes = vec![Shape {
        id: ShapeId(1596),
        xfrm: Some(Xfrm::new(50.0, 50.0, 160.0, 160.0)),
        geom: Geom::preset(shape),
        fill: Some(Fill::solid(ColorRef::rgb(Rgba::rgb(32, 96, 224)))),
        line: Some(Line::none()),
        effects: Some(effects),
        ..Default::default()
    }];
    p
}

fn draw(shape: &str, effect: Effect) -> Image {
    let p = scene(shape, effect);
    let image = render_slide(&p, 0, &RenderOpts { scale: 1.0, ..Default::default() });
    if let Ok(dir) = std::env::var("DECKCRAFT_EFFECT_HOLES_QA") {
        // Overwrite, so a QA directory can be reused across runs.
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(std::path::Path::new(&dir).join(format!("{shape}-{effect:?}.png")), image.to_png()).unwrap();
    }
    image
}

// Fixed contributor-original RGBA goldens; tolerate one final quantization step.
// The nonwhite halo/reflection samples also fail if an effect is disabled outright.
fn assert_golden(shape: &str, effect: Effect, expected: [[u8; 4]; 9]) {
    let image = draw(shape, effect);
    for ((x, y), want) in PROBES.into_iter().zip(expected) {
        let got = image.pixel(x, y);
        assert!(got.iter().zip(want).all(|(a, b)| (*a as i32 - b as i32).abs() <= 1), "{shape} {effect:?} at {x},{y}: {got:?} vs golden {want:?}");
    }
}

#[test]
fn donut_shadow_matches_golden() {
    assert_golden(
        "donut",
        Effect::Shadow,
        [
            [255, 255, 255, 255],
            [255, 255, 255, 255],
            [32, 96, 224, 255],
            [214, 214, 214, 255],
            [233, 233, 233, 255],
            [227, 227, 227, 255],
            [255, 255, 255, 255],
            [255, 255, 255, 255],
            [255, 255, 255, 255],
        ],
    );
}

#[test]
fn donut_glow_matches_golden() {
    assert_golden(
        "donut",
        Effect::Glow,
        [
            [255, 255, 255, 255],
            [255, 255, 255, 255],
            [32, 96, 224, 255],
            [255, 49, 49, 255],
            [255, 111, 111, 255],
            [255, 96, 96, 255],
            [255, 255, 255, 255],
            [255, 255, 255, 255],
            [255, 255, 255, 255],
        ],
    );
}

#[test]
fn donut_reflection_matches_golden() {
    assert_golden(
        "donut",
        Effect::Reflection,
        [
            [255, 255, 255, 255],
            [255, 255, 255, 255],
            [32, 96, 224, 255],
            [255, 255, 255, 255],
            [255, 255, 255, 255],
            [255, 255, 255, 255],
            [130, 166, 238, 255],
            [255, 255, 255, 255],
            [189, 208, 246, 255],
        ],
    );
}

#[test]
fn ellipse_shadow_matches_golden() {
    assert_golden(
        "ellipse",
        Effect::Shadow,
        [
            [255, 255, 255, 255],
            [32, 96, 224, 255],
            [32, 96, 224, 255],
            [214, 214, 214, 255],
            [233, 233, 233, 255],
            [32, 96, 224, 255],
            [255, 255, 255, 255],
            [255, 255, 255, 255],
            [255, 255, 255, 255],
        ],
    );
}

#[test]
fn ellipse_glow_matches_golden() {
    assert_golden(
        "ellipse",
        Effect::Glow,
        [
            [255, 255, 255, 255],
            [32, 96, 224, 255],
            [32, 96, 224, 255],
            [255, 49, 49, 255],
            [255, 111, 111, 255],
            [32, 96, 224, 255],
            [255, 255, 255, 255],
            [255, 255, 255, 255],
            [255, 255, 255, 255],
        ],
    );
}

#[test]
fn ellipse_reflection_matches_golden() {
    assert_golden(
        "ellipse",
        Effect::Reflection,
        [
            [255, 255, 255, 255],
            [32, 96, 224, 255],
            [32, 96, 224, 255],
            [255, 255, 255, 255],
            [255, 255, 255, 255],
            [32, 96, 224, 255],
            [130, 166, 238, 255],
            [189, 208, 246, 255],
            [189, 208, 246, 255],
        ],
    );
}
