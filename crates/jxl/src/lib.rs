//! JPEG XL for PDF images: lossless recompression of a JPEG, and the way back.
//!
//! A JPEG becomes a JPEG XL file from which the original JPEG can be rebuilt byte for byte
//! ([`recompress_jpeg`], [`reconstruct_jpeg`]), about 20% smaller on photographs. The encoder is
//! jixel (pure Rust, BSD-3-Clause OR Apache-2.0), the decoder jxl-oxide (pure Rust, MIT OR
//! Apache-2.0).
//!
//! ISO 32000-2 defines no JPEG XL filter yet, so PDFs written with this carry the placeholder
//! name `JXLDecode` and readers without JPEG XL show nothing for such an image. That is why the
//! optimizer only does this when asked.
//!
//! Neither function panics: every call into jixel and jxl-oxide runs under `catch_unwind`, and the
//! JPEG's declared size is checked against [`MAX_PIXELS`] before the encoder allocates anything.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};

/// Most pixels a JPEG may have to be recompressed (the optimizer's own image limit).
pub const MAX_PIXELS: u64 = 64 << 20;

/// Why a conversion did not happen. The input is then left as it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Not a JPEG, or one this crate does not convert (more than three components, say).
    Unsupported(&'static str),
    /// More than [`MAX_PIXELS`] pixels.
    TooLarge,
    /// The encoder or decoder refused the data, or failed.
    Failed(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Unsupported(why) => write!(f, "not converted: {why}"),
            Error::TooLarge => write!(f, "not converted: the image has more than {MAX_PIXELS} pixels"),
            Error::Failed(why) => write!(f, "JPEG XL conversion failed: {why}"),
        }
    }
}

impl std::error::Error for Error {}

/// A JPEG's frame header: width, height and number of components.
pub fn jpeg_frame(jpeg: &[u8]) -> Option<(u32, u32, u8)> {
    if jpeg.get(..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut at = 2usize;
    // Markers before the first scan: a header has a few dozen, so the walk is bounded.
    for _ in 0..4096 {
        while jpeg.get(at) == Some(&0xFF) && jpeg.get(at.checked_add(1)?) == Some(&0xFF) {
            at += 1;
        }
        if *jpeg.get(at)? != 0xFF {
            return None;
        }
        let marker = *jpeg.get(at.checked_add(1)?)?;
        match marker {
            0x01 | 0xD0..=0xD8 => at = at.checked_add(2)?,
            0xD9 | 0xDA => return None,
            _ => {
                let len = usize::from(u16::from_be_bytes([*jpeg.get(at.checked_add(2)?)?, *jpeg.get(at.checked_add(3)?)?]));
                // Start-of-frame markers: baseline, extended, progressive, lossless and so on.
                if matches!(marker, 0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF) {
                    let h = u32::from(u16::from_be_bytes([*jpeg.get(at.checked_add(5)?)?, *jpeg.get(at.checked_add(6)?)?]));
                    let w = u32::from(u16::from_be_bytes([*jpeg.get(at.checked_add(7)?)?, *jpeg.get(at.checked_add(8)?)?]));
                    return Some((w, h, *jpeg.get(at.checked_add(9)?)?));
                }
                at = at.checked_add(2)?.checked_add(len)?;
            }
        }
    }
    None
}

/// `jpeg` as a JPEG XL file that [`reconstruct_jpeg`] turns back into exactly `jpeg`.
pub fn recompress_jpeg(jpeg: &[u8]) -> Result<Vec<u8>, Error> {
    let (w, h, components) = jpeg_frame(jpeg).ok_or(Error::Unsupported("no JPEG frame header"))?;
    if w == 0 || h == 0 {
        return Err(Error::Unsupported("an empty frame"));
    }
    if !matches!(components, 1 | 3) {
        return Err(Error::Unsupported("only grey and three-component JPEGs"));
    }
    // Before anything is allocated: a failed allocation aborts, and `catch_unwind` cannot stop that.
    if u64::from(w) * u64::from(h) > MAX_PIXELS {
        return Err(Error::TooLarge);
    }
    match catch_unwind(AssertUnwindSafe(|| jixel::encode_jpeg_lossless(jpeg))) {
        Ok(Ok(jxl)) => Ok(jxl),
        Ok(Err(e)) => Err(Error::Failed(e.to_string())),
        Err(_) => Err(Error::Failed("the encoder stopped on this input".into())),
    }
}

/// The JPEG a JPEG XL file was made from, byte for byte; `None` when it carries no reconstruction
/// data (it is not a recompressed JPEG).
pub fn reconstruct_jpeg(jxl: &[u8]) -> Result<Option<Vec<u8>>, Error> {
    let run = || -> Result<Option<Vec<u8>>, Error> {
        let image = jxl_oxide::JxlImage::builder().read(std::io::Cursor::new(jxl)).map_err(|e| Error::Failed(e.to_string()))?;
        if u64::from(image.width()) * u64::from(image.height()) > MAX_PIXELS {
            return Err(Error::TooLarge);
        }
        if image.jpeg_reconstruction_status() != jxl_oxide::JpegReconstructionStatus::Available {
            return Ok(None);
        }
        let mut out = Vec::new();
        image.reconstruct_jpeg(&mut out).map_err(|e| Error::Failed(e.to_string()))?;
        Ok(Some(out))
    };
    catch_unwind(AssertUnwindSafe(run)).unwrap_or_else(|_| Err(Error::Failed("the decoder stopped on this input".into())))
}
