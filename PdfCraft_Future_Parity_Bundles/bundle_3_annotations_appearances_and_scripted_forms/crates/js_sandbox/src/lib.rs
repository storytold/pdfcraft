//! PdfCraft JS Sandbox: Fuel-limited sandboxed ECMAScript runtime with standard Acrobat DOM host objects.
//!
//! Subsystem covering Milestone M6:
//! - [P0] JavaScript engine with Acrobat object model subset (app, Doc, event, util)
//! - [P0] Built-in Acrobat calculations (AFSimple_Calculate, AFNumber_Format, AFRange_Validate)
//! - [P1] Run-a-JavaScript action execution

pub mod af_routines;
pub mod acrobat_dom;
pub mod context;

pub use af_routines::AfRoutines;
pub use acrobat_dom::{AcrobatEvent, AppObject, DocObject};
pub use context::{JsError, SandboxedJsContext};
