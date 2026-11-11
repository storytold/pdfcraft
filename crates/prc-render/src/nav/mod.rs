//! Camera navigation for a model view, without a GUI: an [`Orbit`] turned, panned and zoomed by a
//! viewer's input (or by `model_render`'s moves) gives the [`crate::Camera`] to draw from, and an
//! [`Upright`] says which model axis points up and which way the Front view looks.

mod axes;
mod orbit;

pub use axes::{Axis, Upright};
pub use orbit::{NamedView, Orbit, PITCH_LIMIT, ZOOM_RANGE, reach};
