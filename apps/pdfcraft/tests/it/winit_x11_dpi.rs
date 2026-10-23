#[test]
fn winit_carries_the_x11_server_dpi_fallback() {
    // The behavior tests live beside the policy in winit (util::randr::tests).
    // This guards PdfCraft's vendored copy, which workspace tests do not test itself.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let randr = std::fs::read_to_string(root.join("vendor/winit/src/platform_impl/linux/x11/util/randr.rs")).unwrap();
    assert!(randr.contains("let screen = self.default_root();"));
    assert!(randr.contains("default_dpi_factor(\n                    self.get_xft_dpi(),"));
    assert!(randr.contains("screen.width_in_millimeters.into()"));
    assert!(randr.contains("calc_dpi_factor(screen_pixels, screen_mm)"));
    assert!(randr.contains("server_dpi_wins_over_the_panels_physical_density"));
    let desktop = std::fs::read_to_string(root.join("packaging/linux/ai.storyteller.pdfcraft.desktop")).unwrap();
    assert!(!desktop.contains("WINIT_X11_SCALE_FACTOR"));
}
