fn main() {
    println!("cargo:rerun-if-changed=exports.def");
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        // COM loads these by name; keep them out of the import library (MSVC LNK4104).
        let exports = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("exports.def");
        println!("cargo:rustc-cdylib-link-arg=/DEF:{}", exports.display());
    }
}
