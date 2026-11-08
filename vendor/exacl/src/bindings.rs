//! Rust bindings to system C API; exported via `sys`.

#![allow(
    dead_code,
    non_camel_case_types,
    non_upper_case_globals,
    clippy::unseparated_literal_suffix,
    clippy::unreadable_literal,
    deref_nullptr,  // https://github.com/rust-lang/rust-bindgen/issues/1651
    clippy::too_many_lines,
    clippy::borrow_as_ptr,
    clippy::struct_field_names
)]

include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
