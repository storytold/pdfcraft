use super::*;

#[test]
fn a_part_windows_asks_for_again_is_presented_though_nothing_in_it_changed() {
    // Uncovered, or the Remote Desktop session reconnected: no tile changed, but the window lost
    // that part of its picture. Without presenting it again, it stays blank.
    let uncovered = PixelRect::new(10, 20, 300, 400);
    assert_eq!(presented(&[], Some(uncovered)), vec![uncovered]);
}

#[test]
fn a_frame_like_the_last_presents_nothing_unless_windows_asked() {
    assert!(presented(&[], None).is_empty());
    assert!(presented(&[], Some(PixelRect::default())).is_empty());
    let redrawn = PixelRect::new(0, 0, 64, 64);
    assert_eq!(presented(&[redrawn], None), vec![redrawn]);
    let uncovered = PixelRect::new(100, 0, 200, 50);
    assert_eq!(presented(&[redrawn], Some(uncovered)), vec![redrawn, uncovered]);
}

/// What a frame of `mesh` redraws after the frame before it in `damage`.
fn redrawn(damage: &mut Damage, mesh: &egui::Mesh) -> Vec<PixelRect> {
    let clip = PixelRect::new(0, 0, 128, 128);
    damage.begin(128, 128, 7);
    mark(damage, mesh, clip, 1.0, 11);
    damage.finish()
}

/// A rectangle as egui draws one: four corners, two triangles sharing a diagonal.
fn rectangle(mesh: &mut egui::Mesh, min: egui::Pos2, max: egui::Pos2) {
    mesh.add_colored_rect(egui::Rect::from_min_max(min, max), egui::Color32::RED);
}

#[test]
fn the_same_triangles_drawn_another_way_are_redrawn_and_shifted_indices_are_not() {
    let (min, max) = (egui::pos2(10.0, 10.0), egui::pos2(20.5, 20.5));
    let mut quad = egui::Mesh::default();
    rectangle(&mut quad, min, max);
    // The same two triangles, each with corners of its own: drawn as triangles, not a rectangle.
    let mut apart = egui::Mesh::default();
    for triangle in quad.indices.as_chunks::<3>().0 {
        let first = apart.vertices.len() as u32;
        apart.vertices.extend(triangle.iter().map(|i| quad.vertices[*i as usize]));
        apart.indices.extend([first, first + 1, first + 2]);
    }
    let mut damage = Damage::default();
    redrawn(&mut damage, &quad);
    assert!(redrawn(&mut damage, &quad).is_empty(), "the same frame again redraws nothing");
    assert!(!redrawn(&mut damage, &apart).is_empty(), "the same triangles drawn another way may blend a pixel differently");

    // Something earlier in the mesh shifts every index after it; what is drawn after it is the same.
    let mut one = egui::Mesh::default();
    rectangle(&mut one, egui::pos2(0.0, 0.0), egui::pos2(4.0, 4.0));
    rectangle(&mut one, egui::pos2(80.0, 80.0), egui::pos2(100.0, 100.0));
    let mut two = egui::Mesh::default();
    rectangle(&mut two, egui::pos2(0.0, 0.0), egui::pos2(2.0, 4.0));
    rectangle(&mut two, egui::pos2(2.0, 0.0), egui::pos2(4.0, 4.0));
    rectangle(&mut two, egui::pos2(80.0, 80.0), egui::pos2(100.0, 100.0));
    redrawn(&mut damage, &one);
    let rects = redrawn(&mut damage, &two);
    assert!(!rects.is_empty());
    assert!(rects.iter().all(|rect| rect.intersect(PixelRect::new(64, 64, 128, 128)).is_empty()), "only the tile that changed: {rects:?}");
}
