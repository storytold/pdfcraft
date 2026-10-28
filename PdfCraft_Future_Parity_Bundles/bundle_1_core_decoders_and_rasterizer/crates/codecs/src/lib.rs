//! PdfCraft Codecs: Image decoders and stream filter pipeline for PDF.
//!
//! Subsystem covering Milestone M1 & M2:
//! - [P0] Image codecs in filters crate (DCT, JPX, JBIG2, CCITT Fax)
//! - [P0] Decompression bomb safety limits and bounded streaming

pub mod ccitt;
pub mod jbig2;
pub mod dct;
pub mod jpx;
pub mod pipeline;

pub use ccitt::{decode_ccitt, BiLevelBitmap, CcittError, CcittParams};
pub use jbig2::{Jbig2Decoder, Jbig2Error, Jbig2Segment, PageInfo};
pub use dct::{decode_dct, AdobeColorTransform, DctError, DctImage};
pub use jpx::{decode_jpx, JpxError, JpxImage, JpxImageHeader};
pub use pipeline::{FilterPipeline, FilterType, PipelineError, PipelineOutput};
