//! `cargo run -p pdfcraft-mobile-ffi --features cli --bin uniffi-bindgen -- generate --library
//! <path to the built cdylib> --language swift|kotlin --out-dir <dir>` — library mode, so there
//! is no `.udl` to keep in sync with `src/lib.rs`.

fn main() {
    uniffi::uniffi_bindgen_main()
}
