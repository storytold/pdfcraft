//! W28: `zwp_tablet_v2` (stable/tablet v2) client bindings, generated in the crate
//! from the vendored protocol XML.
//!
//! `wayland-protocols` 0.32, what this fork builds against, excludes
//! `tablet-unstable-v2.xml` from its published package (see the `!` entries in
//! its `include` list), so the bindings are generated here from the same
//! protocol source: `protocols/tablet-v2.xml`, from
//! `https://gitlab.freedesktop.org/wayland/wayland-protocols` (`stable/tablet`),
//! MIT licensed, copyright Stephen "Lyude" Chandler Paul and Red Hat, Inc.
//! The file keeps its own copyright block; `wayland-scanner` 0.31 is already in
//! the dependency graph through `wayland-client`.
//!
//! The module shape mirrors `wayland_protocol!` in `wayland-protocols`; only the
//! client side exists here.

#[allow(dead_code, non_camel_case_types, unused_unsafe, unused_variables)]
#[allow(non_upper_case_globals, non_snake_case, unused_imports)]
#[allow(clippy::all)]
pub mod tablet_v2 {
    //! Client-side API of the tablet-v2 protocol.

    use wayland_client;
    use wayland_client::protocol::*;

    pub mod __interfaces {
        use wayland_client::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!(
            "src/platform_impl/linux/wayland/seat/tablet/protocols/tablet-v2.xml"
        );
    }
    use self::__interfaces::*;

    wayland_scanner::generate_client_code!(
        "src/platform_impl/linux/wayland/seat/tablet/protocols/tablet-v2.xml"
    );
}
