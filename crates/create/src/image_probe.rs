//! Header-only JPEG and JPEG 2000 embedding probes without re-encoding.
//!
//! Used by image embedding (`lib.rs`), EXIF/XMP/JFIF inspection (`upright_image.rs`), and PDF
//! image extraction (`extract.rs`) so marker walking, density parsing, ICC reassembly, and
//! Adobe CMYK detection share one implementation.

use std::ops::Range;

/// Signature of a JP2 file box stream.
pub(crate) const JP2_SIGNATURE: &[u8] = &[0, 0, 0, 0x0C, b'j', b'P', b' ', b' ', 0x0D, 0x0A, 0x87, 0x0A];

/// Signature of a raw JPEG 2000 codestream (SOC + SIZ).
pub(crate) const J2K_CODESTREAM_SIGNATURE: &[u8] = &[0xFF, 0x4F, 0xFF, 0x51];

/// Whether `bytes` begins with a JP2 file signature or a raw JPEG 2000 codestream signature.
pub(crate) fn is_jpx(bytes: &[u8]) -> bool {
    bytes.starts_with(JP2_SIGNATURE) || bytes.starts_with(J2K_CODESTREAM_SIGNATURE)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum JpegVisit {
    Continue,
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum JpegProbeError {
    NotJpeg,
    TruncatedMarker,
    TruncatedSegmentLength,
    InvalidSegmentLength,
    SegmentLengthOverflow,
    TruncatedSegment,
    BadFrameHeader,
    NoImageSize,
    UnsupportedComponents(u8),
}

impl JpegProbeError {
    pub(crate) fn message(self) -> String {
        match self {
            Self::NotJpeg => "not a JPEG file".into(),
            Self::TruncatedMarker => "truncated JPEG marker".into(),
            Self::TruncatedSegmentLength => "truncated JPEG segment length".into(),
            Self::InvalidSegmentLength => "invalid JPEG segment length".into(),
            Self::SegmentLengthOverflow => "JPEG segment length overflow".into(),
            Self::TruncatedSegment => "truncated JPEG segment".into(),
            Self::BadFrameHeader => "bad frame header".into(),
            Self::NoImageSize => "no image size".into(),
            Self::UnsupportedComponents(n) => format!("{n} colour components are not supported"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct JpegProbe {
    pub width: u32,
    pub height: u32,
    pub components: u8,
    pub color_space: &'static str,
    pub invert_cmyk: bool,
    pub dpi: (f64, f64),
    pub icc: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct JpxProbe {
    pub width: u32,
    pub height: u32,
    pub dpi: (f64, f64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum JpxProbeError {
    MissingHeader,
    MissingImageSize,
    BadImageHeader,
    DamagedCodestream,
    UnusualImageSize,
}

impl JpxProbeError {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::MissingHeader => "a JPEG 2000 file without a header",
            Self::MissingImageSize => "a JPEG 2000 file without image size",
            Self::BadImageHeader => "bad image header",
            Self::DamagedCodestream => "a damaged JPEG 2000 codestream",
            Self::UnusualImageSize => "unusual JPEG 2000 image size",
        }
    }
}

pub(crate) fn be32(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 4).map(|x| u32::from_be_bytes([x[0], x[1], x[2], x[3]]))
}

pub(crate) fn be16(b: &[u8], at: usize) -> Option<u16> {
    b.get(at..at + 2).map(|x| u16::from_be_bytes([x[0], x[1]]))
}

/// Walk JPEG marker segments before SOS (`0xDA`) or EOI (`0xD9`).
///
/// Calls `visit(marker, segment_span, payload)` for each marker segment with a length field, where
/// `segment_span` covers the `0xFF` marker byte through the end of the segment payload.
pub(crate) fn walk_jpeg_segments<'a>(
    bytes: &'a [u8],
    mut visit: impl FnMut(u8, Range<usize>, &'a [u8]) -> Result<JpegVisit, JpegProbeError>,
) -> Result<(), JpegProbeError> {
    if bytes.len() < 4 || bytes[0] != 0xFF || bytes[1] != 0xD8 {
        return Err(JpegProbeError::NotJpeg);
    }
    let mut i = 2usize;
    while let Some(&byte) = bytes.get(i) {
        if byte != 0xFF {
            i += 1;
            continue;
        }
        i += 1;
        // Markers may be preceded by any number of 0xFF fill bytes.
        while bytes.get(i) == Some(&0xFF) {
            i += 1;
        }
        let marker_pos = i;
        let marker = *bytes.get(i).ok_or(JpegProbeError::TruncatedMarker)?;
        i += 1;
        // Metadata ends at the first scan; entropy-coded bytes are not segments.
        if marker == 0xDA || marker == 0xD9 {
            break;
        }
        if marker == 0xD8 || marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            continue;
        }
        let len = usize::from(be16(bytes, i).ok_or(JpegProbeError::TruncatedSegmentLength)?);
        if len < 2 {
            return Err(JpegProbeError::InvalidSegmentLength);
        }
        let end = i.checked_add(len).ok_or(JpegProbeError::SegmentLengthOverflow)?;
        let seg = bytes.get(i + 2..end).ok_or(JpegProbeError::TruncatedSegment)?;
        let span = marker_pos.saturating_sub(1)..end;
        if matches!(visit(marker, span, seg)?, JpegVisit::Stop) {
            break;
        }
        i = end;
    }
    Ok(())
}

/// Parse JFIF APP0 density `(x_dpi, y_dpi)` when unit is inches (1) or centimetres (2) and both
/// axes are positive.
pub(crate) fn jfif_dpi(seg: &[u8]) -> Option<(f64, f64)> {
    if !seg.starts_with(b"JFIF\0") || seg.len() < 12 {
        return None;
    }
    let (unit, x, y) = (seg[7], u16::from_be_bytes([seg[8], seg[9]]) as f64, u16::from_be_bytes([seg[10], seg[11]]) as f64);
    if x > 0.0 && y > 0.0 {
        match unit {
            1 => Some((x, y)),
            2 => Some((x * 2.54, y * 2.54)),
            _ => None,
        }
    } else {
        None
    }
}

/// A JPEG's ICC profile from its APP2 `ICC_PROFILE` chunks: (sequence number, chunk count,
/// data). `None` unless every chunk from 1 to the count is there exactly once.
fn jpeg_icc(mut chunks: Vec<(u8, u8, &[u8])>) -> Option<Vec<u8>> {
    let count = chunks.first()?.1;
    chunks.sort_by_key(|c| c.0);
    let complete = usize::from(count) == chunks.len() && chunks.iter().zip(1..=count).all(|(c, seq)| c.0 == seq && c.1 == count);
    complete.then(|| chunks.iter().flat_map(|c| c.2.iter().copied()).collect())
}

/// Whether `marker` is a JPEG SOFn frame marker (excluding DHT `0xC4`, JPG `0xC8`, DAC `0xCC`).
pub(crate) fn is_sof_marker(marker: u8) -> bool {
    matches!(marker, 0xC0..=0xCF) && marker != 0xC4 && marker != 0xC8 && marker != 0xCC
}

/// Probe a JPEG for PDF `/DCTDecode` embedding without decoding or re-encoding pixels.
pub(crate) fn probe_jpeg(bytes: &[u8]) -> Result<JpegProbe, JpegProbeError> {
    let (mut size, mut comps, mut dpi, mut adobe) = (None, 0u8, (72.0, 72.0), false);
    let mut icc_chunks = Vec::new();
    walk_jpeg_segments(bytes, |marker, _span, seg| {
        match marker {
            // APP0 JFIF density.
            0xE0 => {
                if let Some(d) = jfif_dpi(seg) {
                    dpi = d;
                }
            }
            0xEE if seg.starts_with(b"Adobe") => adobe = true,
            // APP2 ICC profile, possibly split over several chunks.
            0xE2 => {
                if let Some(([seq, count], data)) = seg.strip_prefix(b"ICC_PROFILE\0").and_then(<[u8]>::split_first_chunk::<2>) {
                    icc_chunks.push((*seq, *count, data));
                }
            }
            _ if is_sof_marker(marker) && size.is_none() => {
                if seg.len() < 6 {
                    return Err(JpegProbeError::BadFrameHeader);
                }
                size = Some((u16::from_be_bytes([seg[3], seg[4]]) as u32, u16::from_be_bytes([seg[1], seg[2]]) as u32));
                comps = seg[5];
            }
            _ => {}
        }
        Ok(JpegVisit::Continue)
    })?;
    let (width, height) = size.filter(|(w, h)| *w > 0 && *h > 0).ok_or(JpegProbeError::NoImageSize)?;
    let (color_space, invert_cmyk) = match comps {
        1 => ("DeviceGray", false),
        3 => ("DeviceRGB", false),
        4 => ("DeviceCMYK", adobe),
        n => return Err(JpegProbeError::UnsupportedComponents(n)),
    };
    let icc = jpeg_icc(icc_chunks);
    Ok(JpegProbe { width, height, components: comps, color_space, invert_cmyk, dpi, icc })
}

/// The boxes of a JP2 box sequence: (type, payload).
fn jp2_boxes(b: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + 8 <= b.len() {
        let len = be32(b, i).unwrap_or(0) as u64;
        let ty = [b[i + 4], b[i + 5], b[i + 6], b[i + 7]];
        let (head, len) = match len {
            0 => (8, (b.len() - i) as u64),
            1 => match b.get(i + 8..i + 16) {
                Some(x) => (16, u64::from_be_bytes([x[0], x[1], x[2], x[3], x[4], x[5], x[6], x[7]])),
                None => break,
            },
            n => (8, n),
        };
        // `len as usize` would truncate on 32-bit targets and put the end before the payload.
        let end = i.saturating_add(usize::try_from(len).unwrap_or(usize::MAX)).min(b.len());
        if len < head as u64 || end <= i {
            break;
        }
        out.push((ty, &b[i + head..end]));
        i = end;
    }
    out
}

/// Probe a JPEG 2000 image (JP2 file or raw codestream) for PDF `/JPXDecode` embedding without
/// decoding or re-encoding pixels.
pub(crate) fn probe_jpx(bytes: &[u8]) -> Result<JpxProbe, JpxProbeError> {
    let mut dpi = (72.0, 72.0);
    let (width, height) = if bytes.starts_with(JP2_SIGNATURE) {
        let header = jp2_boxes(bytes).into_iter().find(|(t, _)| t == b"jp2h").map(|(_, p)| p).ok_or(JpxProbeError::MissingHeader)?;
        let inner = jp2_boxes(header);
        let ihdr = inner.iter().find(|(t, _)| t == b"ihdr").map(|(_, p)| *p).ok_or(JpxProbeError::MissingImageSize)?;
        // Capture resolution (pixels per metre), when given.
        if let Some((_, res)) = inner.iter().find(|(t, _)| t == b"res ")
            && let Some((_, r)) = jp2_boxes(res).into_iter().find(|(t, _)| t == b"resc" || t == b"resd")
            && r.len() >= 10
        {
            let part = |n: u16, d: u16, e: i8| if d == 0 { 0.0 } else { n as f64 / d as f64 * 10f64.powi(e as i32) * 0.0254 };
            let (v, hz) = (
                part(be16(r, 0).unwrap_or(0), be16(r, 2).unwrap_or(0), r[8] as i8),
                part(be16(r, 4).unwrap_or(0), be16(r, 6).unwrap_or(0), r[9] as i8),
            );
            if v > 1.0 && hz > 1.0 {
                dpi = (hz, v);
            }
        }
        (be32(ihdr, 4).ok_or(JpxProbeError::BadImageHeader)?, be32(ihdr, 0).ok_or(JpxProbeError::BadImageHeader)?)
    } else {
        // SIZ: Lsiz Rsiz Xsiz Ysiz XOsiz YOsiz …
        let (x, y, xo, yo) = (be32(bytes, 8), be32(bytes, 12), be32(bytes, 16), be32(bytes, 20));
        match (x, y, xo, yo) {
            (Some(x), Some(y), Some(xo), Some(yo)) if x > xo && y > yo => (x - xo, y - yo),
            _ => return Err(JpxProbeError::DamagedCodestream),
        }
    };
    if width == 0 || height == 0 || width > 100_000 || height > 100_000 {
        return Err(JpxProbeError::UnusualImageSize);
    }
    Ok(JpxProbe { width, height, dpi })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jpeg_probe_preserves_error_diagnostics() {
        assert_eq!(probe_jpeg(b"\x89PNG").unwrap_err(), JpegProbeError::NotJpeg);
        assert_eq!(probe_jpeg(&[0xFF, 0xD8, 0xFF, 0xFF]).unwrap_err(), JpegProbeError::TruncatedMarker);
        assert_eq!(probe_jpeg(&[0xFF, 0xD8, 0xFF, 0xE0]).unwrap_err(), JpegProbeError::TruncatedSegmentLength);
        assert_eq!(probe_jpeg(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 1]).unwrap_err(), JpegProbeError::InvalidSegmentLength);
        assert_eq!(probe_jpeg(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 5, 1]).unwrap_err(), JpegProbeError::TruncatedSegment);
        assert_eq!(probe_jpeg(&[0xFF, 0xD8, 0xFF, 0xC0, 0, 4, 8, 0]).unwrap_err(), JpegProbeError::BadFrameHeader);
        assert_eq!(probe_jpeg(&[0xFF, 0xD8, 0xFF, 0xD9]).unwrap_err(), JpegProbeError::NoImageSize);
        let two_comp = [0xFF, 0xD8, 0xFF, 0xC0, 0, 11, 8, 0, 2, 0, 3, 2, 1, 0x11, 0, 2, 0x11, 0];
        assert_eq!(probe_jpeg(&two_comp).unwrap_err(), JpegProbeError::UnsupportedComponents(2));
    }

    #[test]
    fn jpx_probe_preserves_bounds_and_diagnostics() {
        let bx = |ty: &[u8], payload: &[u8]| {
            let mut v = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
            v.extend_from_slice(ty);
            v.extend_from_slice(payload);
            v
        };
        assert_eq!(probe_jpx(JP2_SIGNATURE).unwrap_err(), JpxProbeError::MissingHeader);
        let no_ihdr = [JP2_SIGNATURE.to_vec(), bx(b"jp2h", &[])].concat();
        assert_eq!(probe_jpx(&no_ihdr).unwrap_err(), JpxProbeError::MissingImageSize);
        let bad_ihdr = [JP2_SIGNATURE.to_vec(), bx(b"jp2h", &bx(b"ihdr", &[0, 0, 0, 1]))].concat();
        assert_eq!(probe_jpx(&bad_ihdr).unwrap_err(), JpxProbeError::BadImageHeader);
        assert_eq!(probe_jpx(&[0xFF, 0x4F, 0xFF, 0x51, 0, 41]).unwrap_err(), JpxProbeError::DamagedCodestream);

        let mut huge = vec![0xFF, 0x4F, 0xFF, 0x51, 0, 41, 0, 0];
        for v in [100_001u32, 10, 0, 0] {
            huge.extend_from_slice(&v.to_be_bytes());
        }
        assert_eq!(probe_jpx(&huge).unwrap_err(), JpxProbeError::UnusualImageSize);

        // Codestream with non-zero XOsiz/YOsiz offsets.
        let mut offset_siz = vec![0xFF, 0x4F, 0xFF, 0x51, 0, 41, 0, 0];
        for v in [100u32, 80, 20, 10] {
            offset_siz.extend_from_slice(&v.to_be_bytes());
        }
        let p = probe_jpx(&offset_siz).unwrap();
        assert_eq!((p.width, p.height, p.dpi), (80, 70, (72.0, 72.0)));
    }
}
