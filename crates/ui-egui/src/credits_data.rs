//! The contributor and model credits the About dialog shows, baked by `build.rs`.

use craft::ui::patterns::form_dialogs::{Contributor, Credits, ModelCredit};

include!(concat!(env!("OUT_DIR"), "/credits.rs"));
