//! The DeckCraft renderer: draws slides, layouts and masters with `vello_cpu`.
//!
//! One code path serves the editor canvas, thumbnails, image/PDF-raster export and the slide show:
//! [`render_slide`] draws the background, the master and layout graphics, then the slide's shapes
//! (fills with theme shading, gradients, pictures, outlines with dashes and arrowheads, shadows,
//! glow and soft edges, text, tables, charts, groups), optionally with per-shape animation state.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

mod bevel;
mod camera;
mod chart;
mod chartex;
mod colr;
mod extrude;
mod light;

mod compound;
mod extent;
mod images;
mod metafile;
mod morph_path;
mod morph_text;
mod paint;
mod pattern;

mod pathgrad;
mod placed;
mod radar;
mod ramp;
mod table;
mod transition;
mod warp;

use std::collections::HashMap;
use std::sync::Arc;

use deckcraft_color::Rgba;
use deckcraft_fonts::FontDb;
use deckcraft_geom::preset::{self, FillMode, Geometry, SubPath};
use deckcraft_model::resolve::{self, Ctx};
use deckcraft_model::style::{Effects, Fill, Line};
use deckcraft_model::text::TextBody;
use deckcraft_model::{Geom, PhType, Presentation, Shape, ShapeId, ShapeKind, Slide};
use deckcraft_text::datetime::{self, DateTime};
use deckcraft_text::{Fields, Opts};
use kurbo::{Affine, BezPath, PathEl, Point, Rect, Shape as _, Vec2};
use vello_common::filter_effects::{EdgeMode, Filter, FilterPrimitive};
use vello_cpu::{RenderContext, Resources, peniko};

pub use deckcraft_anim::{transition_frame, transition_layers, uses_slide_parts};
pub use extent::{Region, TILE, layout_extent, pasteboard_region, slide_extent};
pub use images::{decode as decode_image, gif_frame};
pub use morph_path::PathMorph;
pub use morph_text::TextMorph;
pub use pattern::{is_fg as pattern_is_fg, presets as pattern_presets, tile as pattern_tile};
pub use placed::{Placed, PlacedLink, PlacedShape, PlacedText, place_master_texts, place_slide, place_slide_with, text_outline};
pub use table::cell_body as table_cell_body;
pub use table::cell_rects as table_cell_rects;
pub use table::row_heights as table_row_heights;
pub use transition::{SlideParts, composite_transition, render_transition};

mod geometry;
mod opts;
mod raster;
mod renderer;
mod slide;
mod stroke;
mod text_draw;

use geometry::shade;
pub use geometry::{camera_turn, crop_dest, parse_path, shape_geometry};
pub(crate) use geometry::{color, scale_of};
use opts::SlideFields;
pub use opts::{ParaState, RenderOpts, ShapeState};
pub use raster::{Image, MAX_SIDE, dpi_for_scale, image_dpi};
#[cfg(test)]
use renderer::RENDERER;
pub use renderer::Renderer;
pub use slide::{
    SlidePart, needs_filters, panics_caught, render_blend, render_layout, render_shape, render_shapes, render_slide, render_slide_drawn,
    render_slide_part,
};
pub(crate) use stroke::stroke;
#[cfg(test)]
use stroke::{MAX_DASHES, dash_with_caps, head_size, limit_dash_count, trim_ends};
use text_draw::preview_empty_bullets;
pub(crate) use text_draw::{draw_glyph, fill_deco, glyph, glyph_place};
pub use text_draw::{draw_layout, draw_layout_with, draw_text, text_transform, text_warp};

#[cfg(test)]
mod tests;

#[cfg(test)]
mod tests_wild;

#[cfg(test)]
mod tests_effect_holes;

#[cfg(test)]
mod tests_placed_group;

#[cfg(test)]
mod mt_tests {
    use super::*;

    /// Regression: multithreaded rendering of a slide with shadows panicked inside vello_cpu.
    #[test]
    fn effects_render_with_threads_requested() {
        let mut s = deckcraft_model::Presentation::default();
        let sl = std::sync::Arc::make_mut(&mut s.slides[0]);
        let mut sh = deckcraft_model::Shape {
            id: ShapeId(900),
            xfrm: Some(deckcraft_geom::Xfrm::new(10.0, 10.0, 100.0, 100.0)),
            style: Some(deckcraft_model::ShapeStyle::accent(deckcraft_color::SchemeSlot::Accent1)),
            ..Default::default()
        };
        sh.effects = Some(deckcraft_model::Effects { soft_edge: Some(5.0), ..Default::default() });
        sl.shapes.push(sh);
        assert!(needs_filters(&s, &s.slides[0]));
        let img = RENDERER.with(|r| r.borrow_mut().slide(&s, &s.slides[0], 0, &RenderOpts { scale: 0.5, threads: 4, ..Default::default() }));
        assert_eq!(img.width, 480);
        // The shape itself is drawn (not a blank fallback).
        assert_ne!(img.pixel(30, 30), [255, 255, 255, 255]);
    }
}
