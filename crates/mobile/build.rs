fn main() {
    println!("cargo:rerun-if-changed=src/lib.rs");
    #[cfg(feature = "swift")]
    {
        // Cargo always supplies OUT_DIR to a build script.
        let out = std::env::var("OUT_DIR").expect("Cargo must supply OUT_DIR for Swift binding generation");
        swift_bridge_build::parse_bridges(["src/lib.rs"]).write_all_concatenated(out, "PdfCraftMobile");
    }
}
