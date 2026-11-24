use super::*;
use egui::{Rect, TextureId, pos2};

const BLACK: u32 = 0xFF00_0000;

fn solid(color: Color32) -> Texture {
    Texture::new(&egui::ColorImage::new([1, 1], vec![color]), TextureOptions::NEAREST)
}

/// A texture whose texel at (x, y) is the opaque colour (x·40, y·40, 7).
fn numbered(width: usize, height: usize, options: TextureOptions) -> Texture {
    let pixels = (0..height).flat_map(|y| (0..width).map(move |x| Color32::from_rgb((x * 40) as u8, (y * 40) as u8, 7))).collect();
    Texture::new(&egui::ColorImage::new([width, height], pixels), options)
}

fn numbered_pixel(x: u32, y: u32) -> u32 {
    BLACK | (x * 40) << 16 | (y * 40) << 8 | 7
}

struct Canvas {
    pixels: Vec<u32>,
    width: usize,
    height: usize,
}

impl Canvas {
    fn new(width: usize, height: usize) -> Self {
        Self { pixels: vec![BLACK; width * height], width, height }
    }

    fn draw(&mut self, mesh: &Mesh, texture: &Texture, clip: PixelRect, scale: f32) {
        let (width, height) = (self.width, self.height);
        draw_mesh(&mut Target { pixels: &mut self.pixels, width, height }, &MeshJob { mesh, texture, clip, scale });
    }

    fn all(&self) -> PixelRect {
        PixelRect::new(0, 0, self.width as i32, self.height as i32)
    }

    fn at(&self, x: usize, y: usize) -> u32 {
        self.pixels[y * self.width + x]
    }

    /// The pixels that aren't black, as (x, y).
    fn touched(&self) -> Vec<(usize, usize)> {
        (0..self.height).flat_map(|y| (0..self.width).map(move |x| (x, y))).filter(|(x, y)| self.at(*x, *y) != BLACK).collect()
    }
}

fn textured(rect: Rect, uv: Rect, color: Color32) -> Mesh {
    let mut mesh = Mesh::with_texture(TextureId::Managed(1));
    mesh.add_rect_with_uv(rect, uv, color);
    mesh
}

const WHOLE: Rect = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));

#[test]
fn a_plain_rectangle_covers_the_pixels_whose_centres_it_holds() {
    let mut canvas = Canvas::new(6, 6);
    let mut mesh = Mesh::default();
    mesh.add_colored_rect(Rect::from_min_max(pos2(1.0, 2.0), pos2(3.0, 5.0)), Color32::RED);
    canvas.draw(&mesh, &solid(Color32::WHITE), canvas.all(), 1.0);
    assert_eq!(canvas.touched(), vec![(1, 2), (2, 2), (1, 3), (2, 3), (1, 4), (2, 4)]);
    assert_eq!(canvas.at(1, 2), 0xFFFF_0000);

    // Half a pixel in: the left column's centres are on the edge and inside, the right one's
    // on the far edge and outside. And at two pixels a point, each point is four pixels.
    let mut canvas = Canvas::new(6, 6);
    let mut mesh = Mesh::default();
    mesh.add_colored_rect(Rect::from_min_max(pos2(0.25, 0.0), pos2(1.25, 0.5)), Color32::RED);
    canvas.draw(&mesh, &solid(Color32::WHITE), canvas.all(), 2.0);
    assert_eq!(canvas.touched(), vec![(0, 0), (1, 0)], "0.5..2.5 pixels wide, one pixel tall");
}

#[test]
fn triangles_that_share_edges_draw_each_pixel_once() {
    // A square cut into four triangles from its middle, in a see-through colour: a pixel drawn by
    // two of them would be darker, one drawn by none would stay black. The centres on both
    // diagonals lie exactly on the shared edges.
    let color = Color32::from_rgba_premultiplied(64, 0, 0, 128);
    let corners = [pos2(0.0, 0.0), pos2(8.0, 0.0), pos2(8.0, 8.0), pos2(0.0, 8.0)];
    for flip in [false, true] {
        let mut mesh = Mesh::default();
        mesh.vertices.push(Vertex { pos: pos2(4.0, 4.0), uv: egui::epaint::WHITE_UV, color });
        mesh.vertices.extend(corners.iter().map(|pos| Vertex { pos: *pos, uv: egui::epaint::WHITE_UV, color }));
        for side in 0..4_u32 {
            let (from, to) = (1 + side, 1 + (side + 1) % 4);
            mesh.indices.extend(if flip { [0, to, from] } else { [0, from, to] });
        }
        let mut canvas = Canvas::new(10, 10);
        canvas.draw(&mesh, &solid(Color32::WHITE), canvas.all(), 1.0);
        let once = over(BLACK, pack(color));
        for y in 0..10 {
            for x in 0..10 {
                let expected = if x < 8 && y < 8 { once } else { BLACK };
                assert_eq!(canvas.at(x, y), expected, "pixel ({x}, {y}), winding flipped: {flip}");
            }
        }
    }
}

#[test]
fn a_stroke_and_its_fringe_leave_no_pixel_between_them() {
    // A line two pixels wide as egui draws it: a rectangle a pixel narrower, and on each side a
    // fringe half a pixel wide that fades out. The centres of both pixel columns are exactly
    // where the rectangle meets a fringe: each column is drawn once, at full colour, whichever
    // side draws it. (Under another rule for the triangles, the right column was nobody's.)
    let gray = Color32::from_rgb(200, 200, 200);
    let point = |x: f32, y: f32, color: Color32| Vertex { pos: pos2(x, y), uv: egui::epaint::WHITE_UV, color };
    let mut mesh = Mesh::default();
    for (x, color) in [(1.0, Color32::TRANSPARENT), (1.5, gray), (2.5, gray), (3.0, Color32::TRANSPARENT)] {
        mesh.vertices.extend([point(x, 0.0, color), point(x, 4.0, color)]);
    }
    for strip in 0..3_u32 {
        let top = strip * 2;
        mesh.indices.extend([top, top + 2, top + 1, top + 1, top + 2, top + 3]);
    }
    let mut canvas = Canvas::new(5, 4);
    canvas.draw(&mesh, &solid(Color32::WHITE), canvas.all(), 1.0);
    for y in 0..4 {
        let row: Vec<u32> = (0..5).map(|x| canvas.at(x, y)).collect();
        assert_eq!(row, vec![BLACK, 0xFFC8_C8C8, 0xFFC8_C8C8, BLACK, BLACK], "row {y}");
    }
}

#[test]
fn a_texture_at_its_own_size_is_copied_texel_for_pixel() {
    for options in [TextureOptions::NEAREST, TextureOptions::LINEAR] {
        let texture = numbered(4, 3, options);
        let mut canvas = Canvas::new(8, 8);
        canvas.draw(&textured(Rect::from_min_max(pos2(2.0, 3.0), pos2(6.0, 6.0)), WHOLE, Color32::WHITE), &texture, canvas.all(), 1.0);
        for y in 0..8 {
            for x in 0..8 {
                let inside = (2..6).contains(&x) && (3..6).contains(&y);
                let expected = if inside { numbered_pixel(x as u32 - 2, y as u32 - 3) } else { BLACK };
                assert_eq!(canvas.at(x, y), expected, "pixel ({x}, {y}) with {options:?}");
            }
        }
    }
}

#[test]
fn the_inside_of_a_feathered_image_is_still_a_copy() {
    // egui fills an image as a fan around its first corner, half a pixel inside the rectangle,
    // with the texture coordinates moved along (`fill_closed_path_with_uv`).
    let texture = numbered(4, 4, TextureOptions::LINEAR);
    let inset = |x: f32, y: f32| Vertex { pos: pos2(x, y), uv: pos2(x / 4.0, y / 4.0), color: Color32::WHITE };
    let mut mesh = Mesh::with_texture(TextureId::Managed(1));
    mesh.vertices.extend([inset(0.5, 0.5), inset(3.5, 0.5), inset(3.5, 3.5), inset(0.5, 3.5)]);
    mesh.indices.extend([1, 0, 2, 2, 0, 3]);
    assert!(quad_of(&mesh, &mesh.indices).is_some(), "the fan of a rectangle is a rectangle");
    let mut canvas = Canvas::new(4, 4);
    canvas.draw(&mesh, &texture, canvas.all(), 1.0);
    for y in 0..3 {
        for x in 0..3 {
            assert_eq!(canvas.at(x, y), numbered_pixel(x as u32, y as u32), "pixel ({x}, {y})");
        }
    }
    assert_eq!(canvas.touched().len(), 9, "the last row and column have their centres on the far edges: the fringe's, not the fill's");

    // Four corners that aren't a rectangle, or two colours, are left to the triangles.
    let mut skewed = mesh.clone();
    skewed.vertices[2].pos.x = 3.0;
    assert!(quad_of(&skewed, &skewed.indices).is_none());
    let mut two_colors = mesh.clone();
    two_colors.vertices[3].color = Color32::RED;
    assert!(quad_of(&two_colors, &two_colors.indices).is_none());
    assert!(quad_of(&mesh, &[1, 0, 2, 2, 0, 2]).is_none(), "a triangle with a corner twice");
}

#[test]
fn nothing_is_drawn_outside_the_clip_rectangle() {
    let texture = numbered(6, 6, TextureOptions::NEAREST);
    let mut canvas = Canvas::new(6, 6);
    let clip = PixelRect::new(2, 1, 4, 3);
    canvas.draw(&textured(Rect::from_min_max(pos2(0.0, 0.0), pos2(6.0, 6.0)), WHOLE, Color32::WHITE), &texture, clip, 1.0);
    assert_eq!(canvas.touched(), vec![(2, 1), (3, 1), (2, 2), (3, 2)]);
    assert_eq!(canvas.at(3, 2), numbered_pixel(3, 2));

    // A clip larger than the window is the window.
    let mut canvas = Canvas::new(6, 6);
    canvas.draw(
        &textured(Rect::from_min_max(pos2(-50.0, -50.0), pos2(50.0, 50.0)), WHOLE, Color32::WHITE),
        &solid(Color32::GREEN),
        PixelRect::new(-9, -9, 99, 99),
        1.0,
    );
    assert_eq!(canvas.touched().len(), 36);
}

#[test]
fn an_enlarged_texture_is_sampled_by_its_filter() {
    let texture = numbered(2, 2, TextureOptions::NEAREST);
    let mut canvas = Canvas::new(4, 4);
    canvas.draw(&textured(Rect::from_min_max(pos2(0.0, 0.0), pos2(4.0, 4.0)), WHOLE, Color32::WHITE), &texture, canvas.all(), 1.0);
    for y in 0..4 {
        for x in 0..4 {
            assert_eq!(canvas.at(x, y), numbered_pixel(x as u32 / 2, y as u32 / 2), "pixel ({x}, {y})");
        }
    }
    // Linear: between two texels a pixel is between their colours.
    let texture = numbered(2, 1, TextureOptions::LINEAR);
    let mut canvas = Canvas::new(8, 1);
    canvas.draw(&textured(Rect::from_min_max(pos2(0.0, 0.0), pos2(8.0, 1.0)), WHOLE, Color32::WHITE), &texture, canvas.all(), 1.0);
    let red = |x: usize| (canvas.at(x, 0) >> 16) & 0xFF;
    assert_eq!((red(0), red(7)), (0, 40), "the ends are the texels themselves");
    assert!((1..8).all(|x| red(x) >= red(x - 1)) && red(3) > 0 && red(4) < 40, "{:?}", (0..8).map(red).collect::<Vec<_>>());
}

#[test]
fn a_glyph_is_its_coverage_times_the_text_colour() {
    // The font atlas holds coverage as premultiplied white.
    let coverage = |a: u8| Color32::from_rgba_premultiplied(a, a, a, a);
    let texture = Texture::new(&egui::ColorImage::new([3, 1], vec![coverage(0), coverage(128), coverage(255)]), TextureOptions::LINEAR);
    let mut canvas = Canvas::new(3, 1);
    canvas.draw(&textured(Rect::from_min_max(pos2(0.0, 0.0), pos2(3.0, 1.0)), WHOLE, Color32::from_rgb(200, 100, 0)), &texture, canvas.all(), 1.0);
    assert_eq!(canvas.at(0, 0), BLACK, "no coverage, nothing drawn");
    assert_eq!(canvas.at(2, 0), 0xFFC8_6400, "full coverage is the colour");
    let half = canvas.at(1, 0);
    assert!((half >> 16 & 0xFF).abs_diff(100) <= 1 && (half >> 8 & 0xFF).abs_diff(50) <= 1, "{half:08x}");
}

#[test]
fn colours_are_interpolated_across_a_triangle() {
    let vertex = |x: f32, y: f32, color: Color32| Vertex { pos: pos2(x, y), uv: egui::epaint::WHITE_UV, color };
    let mut mesh = Mesh::default();
    mesh.vertices.extend([vertex(0.0, 0.0, Color32::RED), vertex(16.0, 0.0, Color32::GREEN), vertex(0.0, 16.0, Color32::BLUE)]);
    mesh.indices.extend([0, 1, 2]);
    let mut canvas = Canvas::new(16, 16);
    canvas.draw(&mesh, &solid(Color32::WHITE), canvas.all(), 1.0);
    let channels = |p: u32| ((p >> 16) & 0xFF, (p >> 8) & 0xFF, p & 0xFF);
    let (r, g, b) = channels(canvas.at(0, 0));
    assert!(r > 230 && g < 20 && b < 20, "the red corner: {r} {g} {b}");
    let (r, g, b) = channels(canvas.at(14, 0));
    assert!(g > 210 && r < 40 && b < 20, "the green corner: {r} {g} {b}");
    let (r, g, b) = channels(canvas.at(5, 5));
    assert!(r.abs_diff(g) < 20 && g.abs_diff(b) < 20 && (r + g + b).abs_diff(255) < 12, "the middle is a third of each: {r} {g} {b}");
    assert_eq!(canvas.at(12, 12), BLACK, "outside the triangle");
}

#[test]
fn a_mesh_of_nonsense_draws_what_it_can_and_never_panics() {
    let texture = numbered(2, 2, TextureOptions::LINEAR);
    let vertex = |x: f32, y: f32| Vertex { pos: pos2(x, y), uv: pos2(x, y), color: Color32::from_rgba_premultiplied(9, 9, 9, 9) };
    let wild = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, f32::MAX, f32::MIN, -1e30, 1e30, 0.0, 3.5];
    let mut mesh = Mesh::with_texture(TextureId::Managed(1));
    for x in wild {
        for y in wild {
            mesh.vertices.push(vertex(x, y));
        }
    }
    let count = mesh.vertices.len() as u32;
    // Every vertex with its neighbours, then indices that point nowhere and a ragged tail.
    mesh.indices.extend((0..count * 3).map(|i| (i * 7 + i / 3) % count));
    mesh.indices.extend([count, u32::MAX, 0, 1, 2]);
    for scale in [1.0, 2.5, 1e30, 0.0, -1.0, f32::NAN] {
        let mut canvas = Canvas::new(7, 5);
        canvas.draw(&mesh, &texture, PixelRect::new(i32::MIN, i32::MIN, i32::MAX, i32::MAX), scale);
        assert_eq!(canvas.pixels.len(), 35);
    }
    // A texture with no pixels, and one that claims more rows than it has.
    let empty = Texture::new(&egui::ColorImage::new([0, 0], Vec::new()), TextureOptions::LINEAR);
    let mut canvas = Canvas::new(7, 5);
    canvas.draw(&textured(Rect::from_min_max(pos2(0.0, 0.0), pos2(7.0, 5.0)), WHOLE, Color32::WHITE), &empty, canvas.all(), 1.0);
    let short =
        Texture::new(&egui::ColorImage { size: [2, 400], source_size: egui::Vec2::ZERO, pixels: vec![Color32::RED; 3] }, TextureOptions::NEAREST);
    assert_eq!((short.width(), short.height()), (2, 1));
    canvas.draw(&textured(Rect::from_min_max(pos2(0.0, 0.0), pos2(7.0, 5.0)), WHOLE, Color32::WHITE), &short, canvas.all(), 1.0);
}

#[test]
fn a_patch_replaces_part_of_a_texture_and_what_falls_outside_is_dropped() {
    let mut texture = numbered(4, 4, TextureOptions::NEAREST);
    assert!(texture.opaque);
    let patch = egui::ColorImage::new([3, 2], vec![Color32::from_rgba_premultiplied(1, 2, 3, 4); 6]);
    texture.patch([2, 3], &patch);
    assert_eq!(texture.texel(2, 3), 0x0401_0203);
    assert_eq!(texture.texel(3, 3), 0x0401_0203);
    assert_eq!(texture.texel(1, 3), numbered_pixel(1, 3), "left of the patch");
    assert_eq!(texture.texel(2, 2), numbered_pixel(2, 2), "above it");
    assert!(!texture.opaque, "a see-through patch ends copying without blending");
    texture.patch([9, 0], &patch);
    texture.patch([0, usize::MAX], &patch);
    texture.patch([0, 0], &egui::ColorImage::new([0, 0], Vec::new()));
    assert_eq!(texture.texel(0, 0), numbered_pixel(0, 0));
}

#[test]
fn blending_is_premultiplied_and_saturates() {
    assert_eq!(over(0xFF12_3456, 0xFFAB_CDEF), 0xFFAB_CDEF, "opaque replaces");
    assert_eq!(over(0xFF12_3456, 0), 0xFF12_3456, "nothing leaves it");
    assert!((over(0xFFFF_FFFF, pack(Color32::from_rgba_premultiplied(0, 0, 0, 128))) & 0xFF).abs_diff(127) <= 1, "half black over white");
    assert_eq!(over(0xFFF0_F0F0, pack(Color32::from_rgba_premultiplied(200, 200, 200, 0))), 0xFFFF_FFFF, "an additive colour stops at white");
    assert_eq!(modulate(0xFF80_4020, 0xFFFF_FFFF), 0xFF80_4020);
    assert_eq!(modulate(0x8080_8080, 0xFFFF_0000), 0x8080_0000);
    assert_eq!((lerp(0xFF00_0000, 0xFFFF_FFFF, 0), lerp(0xFF00_0000, 0xFFFF_FFFF, 256)), (0xFF00_0000, 0xFFFF_FFFF));
}

#[test]
fn a_rectangle_with_a_texture_coordinate_out_of_range_is_sampled_not_copied() {
    // A rectangle as wide as the copy tolerance, with one huge texture coordinate across it: it
    // passes for a texture at its own size, and the texel column it would be copied from is past
    // what an `i64` holds. Adding it to a pixel column overflowed.
    let texture = numbered(4, 4, TextureOptions::NEAREST);
    let far = 1e20 / 4.0;
    let mesh = textured(Rect::from_min_max(pos2(1.49, 0.0), pos2(1.51, 2.0)), Rect::from_min_max(pos2(far, 0.0), pos2(far, 0.5)), Color32::WHITE);
    let mut canvas = Canvas::new(4, 4);
    canvas.draw(&mesh, &texture, canvas.all(), 1.0);
    assert!(canvas.touched().iter().all(|(x, _)| *x == 1), "only the column the rectangle holds the centre of");
}

#[test]
fn a_corner_snapped_onto_a_pixel_centre_still_lets_one_triangle_draw_it() {
    // Two triangles share an edge a 1024th of a pixel right of the centres of column 1. Snapped
    // to the subpixel grid, the edge runs through those centres, and the right triangle owns them.
    // Its bounds came from the corners before snapping and started at column 2: the column was
    // nobody's.
    let edge = 1.5 + 1.0 / 1024.0;
    let color = Color32::from_rgba_premultiplied(64, 0, 0, 128);
    let point = |x: f32, y: f32| Vertex { pos: pos2(x, y), uv: egui::epaint::WHITE_UV, color };
    let mut mesh = Mesh::default();
    mesh.vertices.extend([point(edge, 0.0), point(edge, 8.0), point(-8.0, 4.0), point(10.0, 4.0)]);
    mesh.indices.extend([0, 1, 2, 0, 1, 3]);
    let mut canvas = Canvas::new(10, 8);
    canvas.draw(&mesh, &solid(Color32::WHITE), canvas.all(), 1.0);
    let once = over(BLACK, pack(color));
    for y in 0..8 {
        assert_eq!(canvas.at(1, y), once, "pixel (1, {y}) is drawn by exactly one triangle");
    }
}

#[test]
fn a_texture_shrunk_one_way_is_filtered_as_shrunk_whichever_way_it_grows() {
    // A 4×8 texture over 8×2 pixels: enlarged across, shrunk eight times down. With linear
    // shrinking, the magnification filter must not matter: it was chosen from the across
    // direction alone, so nearest enlarging picked single texel rows.
    let triangle = |texture: &Texture| {
        let corner = |x: f32, y: f32, u: f32, v: f32| Vertex { pos: pos2(x, y), uv: pos2(u, v), color: Color32::WHITE };
        let mut mesh = Mesh::with_texture(TextureId::Managed(1));
        mesh.vertices.extend([corner(0.0, 0.0, 0.0, 0.0), corner(8.0, 0.0, 1.0, 0.0), corner(0.0, 2.0, 0.0, 1.0)]);
        mesh.indices.extend([0, 1, 2]);
        let mut canvas = Canvas::new(8, 2);
        canvas.draw(&mesh, texture, canvas.all(), 1.0);
        canvas.pixels
    };
    let nearest_enlarging = TextureOptions { magnification: TextureFilter::Nearest, ..TextureOptions::LINEAR };
    let pixels = |options| {
        let pixels = (0..8).flat_map(|y| (0..4).map(move |x| Color32::from_rgb((x * 60) as u8, (y * 30) as u8, 7))).collect();
        Texture::new(&egui::ColorImage::new([4, 8], pixels), options)
    };
    assert_eq!(triangle(&pixels(nearest_enlarging)), triangle(&pixels(TextureOptions::LINEAR)));
}
