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
