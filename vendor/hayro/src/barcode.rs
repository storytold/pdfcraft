//! Bounded image-local barcode regions. No page rendering, fixture truth or global cache.
//! Uses rxing 0.9.3's existing row decoder, not a second Code128 payload decoder.
//! #3251 (SandeepSheoran1995, d5076ee) was inspected; no scanner source is copied.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::todo, clippy::unimplemented, clippy::unreachable)]
use rxing::{
    BarcodeFormat, DecodeHints, RXingResult,
    common::BitArray,
    oned::{MultiFormatOneDReader, OneDReader},
};
use std::{
    collections::HashSet,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Rect {
    pub(crate) x0: usize,
    pub(crate) y0: usize,
    pub(crate) x1: usize,
    pub(crate) y1: usize,
}
impl Rect {
    /// Return the area of a validated half-open source rectangle; reversed sides contribute zero.
    pub(crate) fn area(self) -> usize {
        self.x1.saturating_sub(self.x0) * self.y1.saturating_sub(self.y0)
    }
    /// Return the intersection area of two bounded, half-open source rectangles.
    pub(crate) fn overlap(self, b: Self) -> usize {
        self.x1.min(b.x1).saturating_sub(self.x0.max(b.x0)) * self.y1.min(b.y1).saturating_sub(self.y0.max(b.y0))
    }
    /// Check complete half-open containment, allowing coincident outer edges.
    #[cfg(test)]
    pub(crate) fn contains(self, b: Self) -> bool {
        self.x0 <= b.x0 && self.y0 <= b.y0 && self.x1 >= b.x1 && self.y1 >= b.y1
    }
}
#[derive(Clone, Debug)]
pub(crate) struct Region {
    pub(crate) format: String,
    pub(crate) payload: String,
    pub(crate) ink: Rect,
    pub(crate) quiet: Rect,
    pub(crate) pitch: f32,
}

/// Static per-occurrence share of one page's barcode allowance. No remaining-time pool:
/// the same source on a tile, reordered page or cold cache receives the same share.
#[derive(Clone, Copy)]
struct ScanLimits {
    millis: u64,
    visits: usize,
    calls: usize,
}
impl ScanLimits {
    /// Admission fixes time/call shares before drawing. Source-sized headroom avoids spending
    /// the entire share converting a large raster. Base visits sum to 64 Mi; headroom is twice
    /// decoded source pixels (normally at most 16 Mi from page admission), each image <=64 Mi.
    /// Even inconsistent source dimensions cannot exceed 192 Mi visits: sixteen images <=4 Mi.
    fn for_images(images: usize, pixels: usize) -> Result<Self, String> {
        if !(1..=16).contains(&images) || !(1..=4_194_304).contains(&pixels) {
            return Err("page image budget".into());
        }
        Ok(Self {
            millis: 1500 / images as u64,
            visits: (64usize * 1024 * 1024 / images).saturating_add(pixels.saturating_mul(2)).min(64 * 1024 * 1024),
            calls: 4096 / images,
        })
    }
}

struct Budget<'a> {
    cancel: &'a AtomicBool,
    start: Instant,
    limits: ScanLimits,
    visits: usize,
    calls: usize,
}
impl Budget<'_> {
    /// Check cooperative cancellation/deadline between bounded operations; a native call is not preemptible.
    fn poll(&self) -> Result<(), String> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        if self.start.elapsed() > Duration::from_millis(self.limits.millis) {
            return Err("deadline".into());
        }
        Ok(())
    }
    /// Charge inspected source pixels against this occurrence's fixed page share.
    fn charge(&mut self, n: usize) -> Result<(), String> {
        self.poll()?;
        self.visits = self.visits.checked_add(n).ok_or("work overflow")?;
        if self.visits > self.limits.visits {
            return Err("pixel visit budget".into());
        }
        Ok(())
    }
    /// Reserve one native decoder invocation before entering it.
    fn call(&mut self) -> Result<(), String> {
        self.poll()?;
        self.calls += 1;
        if self.calls > self.limits.calls {
            return Err("native call budget".into());
        }
        #[cfg(test)]
        TEST_NATIVE_CALLS.with(|n| n.set(n.get() + 1));
        Ok(())
    }
}
struct Gray {
    w: usize,
    h: usize,
    pixels: Vec<u8>,
    threshold: u8,
}
impl Gray {
    /// Read a source sample with coordinate, checked-offset and storage validation.
    /// Valid reads allocate no error text; malformed storage retains its specific error.
    fn get(&self, x: usize, y: usize) -> Result<u8, String> {
        if x >= self.w || y >= self.h {
            return Err("pixel outside source".into());
        }
        self.pixels.get(y.checked_mul(self.w).and_then(|v| v.checked_add(x)).ok_or("pixel overflow")?).copied().ok_or_else(|| {
            #[cfg(test)]
            TEST_GRAY_ERROR_CONSTRUCTIONS.with(|v| v.set(v.get() + 1));
            "short gray source".into()
        })
    }
    /// Threshold a checked source sample and optionally invert polarity, propagating address errors.
    fn dark(&self, x: usize, y: usize, inverted: bool) -> Result<bool, String> {
        Ok((self.get(x, y)? < self.threshold) ^ inverted)
    }
}

/// Oriented views change addresses only. No resampling or rotated image allocation.
struct View<'a> {
    gray: &'a Gray,
    vertical: bool,
    reverse: bool,
    inverted: bool,
}
impl View<'_> {
    /// Return the scan width after choosing the original or transposed source axes.
    fn width(&self) -> usize {
        if self.vertical { self.gray.h } else { self.gray.w }
    }
    /// Return the scan height after choosing the original or transposed source axes.
    fn height(&self) -> usize {
        if self.vertical { self.gray.w } else { self.gray.h }
    }
    /// Map a checked oriented scan coordinate to the source and apply the selected polarity.
    fn dark(&self, u: usize, v: usize) -> Result<bool, String> {
        if u >= self.width() || v >= self.height() {
            return Err("view outside source".into());
        }
        let u = if self.reverse { self.width() - 1 - u } else { u };
        let (x, y) = if self.vertical { (v, u) } else { (u, v) };
        self.gray.dark(x, y, self.inverted)
    }
    /// Materialize one threshold row after checking its complete strided source range.
    /// The iterator preserves axis, direction and polarity without repeating coordinate checks per pixel.
    fn row_bits(&self, v: usize) -> Result<Vec<bool>, String> {
        if v >= self.height() {
            return Err("view outside source".into());
        }
        let last = self.width().checked_sub(1).ok_or("view outside source")?;
        let (start, stride) = if self.vertical { (v, self.gray.w) } else { (v.checked_mul(self.gray.w).ok_or("pixel overflow")?, 1) };
        let end = last.checked_mul(stride).and_then(|n| start.checked_add(n)).and_then(|n| n.checked_add(1)).ok_or("pixel overflow")?;
        // The vertical stride is positive because v < height() == gray.w; horizontal stride is one.
        let samples = self.gray.pixels.get(start..end).ok_or("short gray source")?.iter().step_by(stride);
        let dark = |pixel: &u8| (*pixel < self.gray.threshold) ^ self.inverted;
        Ok(if self.reverse { samples.rev().map(dark).collect() } else { samples.map(dark).collect() })
    }
    /// Map an in-bounds view rectangle back to image-local coordinates without resampling.
    fn source_rect(&self, r: Rect) -> Rect {
        let r = if self.reverse { Rect { x0: self.width() - r.x1, x1: self.width() - r.x0, ..r } } else { r };
        if self.vertical { Rect { x0: r.y0, y0: r.x0, x1: r.y1, y1: r.x1 } } else { r }
    }
    /// Map a bounded image-local rectangle into this transposed/reversed scan view.
    fn view_rect(&self, r: Rect) -> Rect {
        let r = if self.vertical { Rect { x0: r.y0, y0: r.x0, x1: r.y1, y1: r.x1 } } else { r };
        if self.reverse { Rect { x0: self.width() - r.x1, x1: self.width() - r.x0, ..r } } else { r }
    }
    /// Validate the quiet ring outside the ink rectangle, allowing at most 0.2% dark speckle.
    /// Charge each inspected row and propagate cancellation, deadline and visit-limit failures.
    fn ring_clear(&self, ink: Rect, quiet: Rect, budget: &mut Budget<'_>) -> Result<bool, String> {
        let mut dark = 0usize;
        let mut count = 0usize;
        for y in quiet.y0..quiet.y1 {
            budget.charge(quiet.x1 - quiet.x0)?;
            for x in quiet.x0..quiet.x1 {
                if x >= ink.x0 && x < ink.x1 && y >= ink.y0 && y < ink.y1 {
                    continue;
                }
                count += 1;
                dark += usize::from(self.dark(x, y)?);
            }
        }
        // Permit sparse scanner speckle, never a text stroke or clipped neighboring symbol.
        Ok(count > 0 && dark.saturating_mul(1000) <= count.saturating_mul(2))
    }
}

/// Deduplicate overlapping same-format/payload regions while preserving separate locations.
/// Refuse a seventeenth distinct region so the caller can retain conservative smoothing.
fn insert(regions: &mut Vec<Region>, region: Region) -> Result<(), String> {
    if regions
        .iter()
        .any(|r| r.format == region.format && r.payload == region.payload && r.ink.overlap(region.ink) * 2 >= r.ink.area().min(region.ink.area()))
    {
        return Ok(());
    }
    if regions.len() >= 16 {
        return Err("region budget; discard all".into());
    }
    regions.push(region);
    Ok(())
}

/// Require a supported printable payload as well as a valid native Code128 frame.
fn valid_code128(result: &RXingResult) -> bool {
    let text = result.getText();
    !text.is_empty() && text.len() <= 256 && text.bytes().all(|c| (32..127).contains(&c)) && valid_code128_framing(result)
}

/// Validate bounded framing/checksum independently of payload policy, so rejected payloads can be skipped safely.
fn valid_code128_framing(result: &RXingResult) -> bool {
    let raw = result.getRawBytes();
    if raw.len() < 4 || raw.len() > 259 {
        return false;
    }
    let Some((&start, tail)) = raw.split_first() else {
        return false;
    };
    if !(103..=105).contains(&start) || tail.last() != Some(&106) {
        return false;
    }
    let Some(check_index) = raw.len().checked_sub(2) else {
        return false;
    };
    let Some(&check) = raw.get(check_index) else {
        return false;
    };
    let Some(body) = raw.get(1..check_index) else {
        return false;
    };
    if body.iter().any(|c| *c > 102) || check > 102 {
        return false;
    }
    // At most 256*102*258, bounded by the metadata limit above.
    let sum = body.iter().enumerate().fold(usize::from(start), |s, (i, c)| s + usize::from(*c) * (i + 1));
    sum % 103 == usize::from(check)
}

/// Validate ASCII UPC-A/EAN-13 digits and their alternating-weight check digit.
fn valid_ean(text: &str) -> bool {
    if !matches!(text.len(), 12 | 13) || !text.bytes().all(|c| c.is_ascii_digit()) {
        return false;
    }
    let Some((&last, body)) = text.as_bytes().split_last() else {
        return false;
    };
    let sum: u32 = body.iter().rev().enumerate().map(|(i, c)| u32::from(*c - b'0') * if i % 2 == 0 { 3 } else { 1 }).sum();
    (10 - sum % 10) % 10 == u32::from(last - b'0')
}

/// Locate the complete first/last bar from measured runs, not scanline point-box padding.
fn row_span(bits: &[bool], result: &RXingResult) -> Option<(usize, usize, f32)> {
    let ean = matches!(result.getBarcodeFormat(), BarcodeFormat::EAN_13 | BarcodeFormat::UPC_A);
    if !(if ean { valid_ean(result.getText()) } else { valid_code128_framing(result) }) {
        return None;
    }
    let point = result.getPoints().first()?;
    if !point.x.is_finite() || point.x < 0.0 || point.x >= bits.len() as f32 {
        return None;
    }
    let mut runs = Vec::new();
    let mut start = 0;
    for i in 1..=bits.len() {
        if i == bits.len() || bits.get(i) != bits.get(start) {
            runs.push((start, i, *bits.get(start)?));
            start = i;
        }
    }
    let groups = if ean { 0 } else { result.getRawBytes().len().checked_sub(1)? };
    let count = if ean { 59 } else { groups.checked_mul(6)?.checked_add(7)? };
    for (i, &(x0, _, dark)) in runs.iter().enumerate() {
        if !dark {
            continue;
        }
        let start_end = runs.get(i.checked_add(if ean { 2 } else { 5 })?)?.1;
        if (((x0 + start_end) as f32 * 0.5) - point.x).abs() > 1.0 {
            continue;
        }
        let pitch = (start_end - x0) as f32 / if ean { 3.0 } else { 11.0 };
        if !(1.0..=32.0).contains(&pitch) {
            continue;
        }
        let span = runs.get(i..i.checked_add(count)?)?;
        if ean {
            if (((span.last()?.1 - x0) as f32) - 95.0 * pitch).abs() > pitch * 1.5 {
                continue;
            }
            let guards = span.get(..3)?.iter().chain(span.get(27..32)?.iter()).chain(span.get(56..59)?.iter());
            if guards.into_iter().any(|(a, b, _)| (((b - a) as f32) - pitch).abs() > pitch * 0.4) {
                continue;
            }
            if span.iter().all(|(a, b, _)| {
                let m = (b - a) as f32 / pitch;
                (0.6..=4.4).contains(&m) && (m - m.round()).abs() <= 0.4
            }) {
                return Some((x0, span.last()?.1, pitch));
            }
            continue;
        }
        let mut valid = true;
        for group in span.get(..groups * 6)?.as_chunks::<6>().0 {
            let first = group.first()?;
            let last = group.last()?;
            if (((last.1 - first.0) as f32) - 11.0 * pitch).abs() > pitch * 0.8 {
                valid = false;
                break;
            }
            for &(a, b, _) in group {
                let modules = (b - a) as f32 / pitch;
                if (modules - modules.round()).abs() > 0.40 || !(0.6..=4.4).contains(&modules) {
                    valid = false;
                    break;
                }
            }
        }
        let stop = span.get(groups * 6..)?;
        for (&(a, b, _), modules) in stop.iter().zip([2.0, 3.0, 3.0, 1.0, 1.0, 1.0, 2.0]) {
            if (((b - a) as f32) - modules * pitch).abs() > pitch * 0.65 {
                valid = false;
            }
        }
        if valid {
            return Some((x0, span.last()?.1, pitch));
        }
    }
    None
}

/// Compare one complete bar pattern with a neighboring source row under the pixel-work budget.
/// Propagate invalid addresses or exhausted/cancelled work rather than extending uncertain ink.
fn same_row(view: &View<'_>, y: usize, x0: usize, pattern: &[bool], budget: &mut Budget<'_>) -> Result<bool, String> {
    budget.charge(pattern.len())?;
    let mut errors = 0usize;
    for (i, &expected) in pattern.iter().enumerate() {
        errors += usize::from(view.dark(x0 + i, y)? != expected);
        if errors > pattern.len() / 100 {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Extend a checksum-valid scanline through the actual bar pixels, then validate a complete quiet ring.
fn reconstruct_bar(view: &View<'_>, y: usize, bits: &[bool], result: &RXingResult, budget: &mut Budget<'_>) -> Result<Option<Region>, String> {
    if matches!(result.getBarcodeFormat(), BarcodeFormat::CODE_128) && !valid_code128(result) {
        return Ok(None);
    }
    let Some((x0, x1, pitch)) = row_span(bits, result) else {
        return Ok(None);
    };
    let pattern = bits.get(x0..x1).ok_or("bar span")?;
    let mut y0 = y;
    let mut y1 = y + 1;
    while y0 > 0 && same_row(view, y0 - 1, x0, pattern, budget)? {
        y0 -= 1;
    }
    while y1 < view.height() && same_row(view, y1, x0, pattern, budget)? {
        y1 += 1;
    }
    if y1 - y0 < 8 {
        return Ok(None);
    }
    if matches!(result.getBarcodeFormat(), BarcodeFormat::EAN_13 | BarcodeFormat::UPC_A) {
        // Retail guards can extend below the data band (UPC also extends its outside digits).
        // Accept only that measured sub-pattern, never arbitrary ink in the surrounding ring.
        let upc = *result.getBarcodeFormat() == BarcodeFormat::UPC_A || result.getText().starts_with('0');
        let extension: Vec<bool> = pattern
            .iter()
            .enumerate()
            .map(|(x, black)| {
                let module = (x as f32 / pitch).floor() as usize;
                *black && (module < 3 || (45..50).contains(&module) || module >= 92 || (upc && !(10..85).contains(&module)))
            })
            .collect();
        let limit = (8.0 * pitch).ceil() as usize;
        for _ in 0..limit {
            if y0 == 0 || !same_row(view, y0 - 1, x0, &extension, budget)? {
                break;
            }
            y0 -= 1;
        }
        for _ in 0..limit {
            if y1 >= view.height() || !same_row(view, y1, x0, &extension, budget)? {
                break;
            }
            y1 += 1;
        }
    }
    let margin = (12.0 * pitch).ceil() as usize;
    let guard = (pitch.ceil() as usize + 1).max(4);
    let Some(qx0) = x0.checked_sub(margin) else {
        return Ok(None);
    };
    let Some(qy0) = y0.checked_sub(guard) else {
        return Ok(None);
    };
    let ink = Rect { x0, y0, x1, y1 };
    let quiet = Rect { x0: qx0, y0: qy0, x1: x1 + margin, y1: y1 + guard };
    if quiet.x1 > view.width() || quiet.y1 > view.height() || !view.ring_clear(ink, quiet, budget)? {
        return Ok(None);
    }
    let (format, payload) = match result.getBarcodeFormat() {
        BarcodeFormat::EAN_13 if result.getText().starts_with('0') => ("UPC_A", result.getText().strip_prefix('0').ok_or("UPC prefix")?),
        BarcodeFormat::EAN_13 => ("EAN_13", result.getText()),
        BarcodeFormat::UPC_A => ("UPC_A", result.getText()),
        _ => ("CODE_128", result.getText()),
    };
    Ok(Some(Region { format: format.into(), payload: payload.into(), ink: view.source_rect(ink), quiet: view.source_rect(quiet), pitch }))
}

/// Search both axes, directions and polarities with bounded calls, excluding accepted and rejected physical row spans.
/// At the native-call cap, return the fully validated subset; cancellation and other failures remain errors.
fn scan_bars(gray: &Gray, regions: &mut Vec<Region>, budget: &mut Budget<'_>) -> Result<(), String> {
    let hints = DecodeHints {
        PossibleFormats: Some(HashSet::from([BarcodeFormat::CODE_128, BarcodeFormat::EAN_13, BarcodeFormat::UPC_A])),
        ..DecodeHints::default()
    };
    for inverted in [false, true] {
        for vertical in [false, true] {
            for reverse in [false, true] {
                let view = View { gray, vertical, reverse, inverted };
                for y in (0..view.height()).step_by(8) {
                    budget.charge(view.width())?;
                    let mut bits = view.row_bits(y)?;
                    // Already validated physical regions may be excluded from subsequent row searches.
                    // This removes only that location; identical payloads elsewhere remain searchable.
                    for _ in 0..16 {
                        for known in regions.iter() {
                            let r = view.view_rect(known.ink);
                            if y >= r.y0 && y < r.y1 {
                                bits.get_mut(r.x0..r.x1).ok_or("known bar span")?.fill(false);
                            }
                        }
                        if bits.windows(2).filter(|p| p.first() != p.get(1)).count() < 24 {
                            break;
                        }
                        let mut row = BitArray::with_size(bits.len());
                        for (x, &black) in bits.iter().enumerate() {
                            if black {
                                row.set(x);
                            }
                        }
                        // Only complete, checksum/geometry/quiet-zone-validated regions enter this list.
                        // Exhausting further search calls must not erase that validated subset. Other
                        // failures (including cancellation/deadline) still discard the image's result.
                        if budget.calls >= budget.limits.calls && !regions.is_empty() {
                            budget.poll()?;
                            return Ok(());
                        }
                        budget.call()?;
                        let result = MultiFormatOneDReader::new(&hints).decode_row(y as u32, &row, &hints);
                        let Ok(result) = result else {
                            break;
                        };
                        if let Some(region) = reconstruct_bar(&view, y, &bits, &result, budget)? {
                            insert(regions, region)?;
                        } else if let Some((x0, x1, _)) = row_span(&bits, &result) {
                            // A checksum-valid row may fail complete-region/quiet-zone validation.
                            // Exclude only its measured bar span from this row's bounded search;
                            // it is never inserted as a protected region.
                            bits.get_mut(x0..x1).ok_or("rejected bar span")?.fill(false);
                        } else {
                            break;
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// Check a finder against pixel-derived module centres; missing corner varies with orientation.
fn finder(view: &View<'_>, ink: Rect, pitch: usize, right: bool, bottom: bool) -> Result<bool, String> {
    let x0 = if right { ink.x1 - 7 * pitch } else { ink.x0 };
    let y0 = if bottom { ink.y1 - 7 * pitch } else { ink.y0 };
    let mut errors = 0;
    for y in 0..7 {
        for x in 0..7 {
            let expected = x == 0 || y == 0 || x == 6 || y == 6 || ((2..=4).contains(&x) && (2..=4).contains(&y));
            errors += usize::from(view.dark(x0 + x * pitch + pitch / 2, y0 + y * pitch + pitch / 2)? != expected);
        }
    }
    Ok(errors <= 1)
}

/// Count pixels that disagree with their fitted QR module center, charging every inspected row.
fn grid_error(view: &View<'_>, ink: Rect, pitch: usize, budget: &mut Budget<'_>) -> Result<usize, String> {
    let mut errors = 0;
    for y in ink.y0..ink.y1 {
        budget.charge(ink.x1 - ink.x0)?;
        let cy = ink.y0 + ((y - ink.y0) / pitch) * pitch + pitch / 2;
        for x in ink.x0..ink.x1 {
            let cx = ink.x0 + ((x - ink.x0) / pitch) * pitch + pitch / 2;
            errors += usize::from(view.dark(x, y)? != view.dark(cx, cy)?);
        }
    }
    Ok(errors)
}

/// Fit a bounded QR module grid around native seed points; never use the point box as protection.
/// Require three finders, a consistent pixel grid and a complete four-module quiet zone.
/// Return None for uncertain geometry/payloads and propagate cancellation or exhausted work.
fn reconstruct_qr(gray: &Gray, result: &RXingResult, budget: &mut Budget<'_>) -> Result<Option<Region>, String> {
    let points = result.getPoints();
    if points.len() != 4 || result.getText().is_empty() || result.getText().len() > 256 {
        return Ok(None);
    }
    let mut x0 = gray.w;
    let mut y0 = gray.h;
    let mut x1 = 0;
    let mut y1 = 0;
    for point in points {
        if !point.x.is_finite() || !point.y.is_finite() || point.x < 0.0 || point.y < 0.0 || point.x >= gray.w as f32 || point.y >= gray.h as f32 {
            return Ok(None);
        }
        x0 = x0.min(point.x.floor() as usize);
        y0 = y0.min(point.y.floor() as usize);
        x1 = x1.max(point.x.ceil() as usize + 1);
        y1 = y1.max(point.y.ceil() as usize + 1);
    }
    if x1 <= x0 || y1 <= y0 || x0 == 0 || y0 == 0 {
        return Ok(None);
    }
    // Result points seed a bounded pixel inspection; their box is never the final rectangle.
    let mut best: Option<(usize, Region)> = None;
    for inverted in [false, true] {
        let view = View { gray, vertical: false, reverse: false, inverted };
        for version in 1..=40 {
            let modules = 17 + 4 * version;
            let pitch = ((x1 - x0) as f32 / modules as f32).round() as usize;
            if pitch == 0 || pitch > 32 {
                continue;
            }
            let side = pitch * modules;
            if side.abs_diff(x1 - x0) > 4 || side.abs_diff(y1 - y0) > 4 {
                continue;
            }
            for dy in [0isize, -1, 1, -2, 2] {
                for dx in [0isize, -1, 1, -2, 2] {
                    let (Some(ix), Some(iy)) = (x0.checked_add_signed(dx), y0.checked_add_signed(dy)) else {
                        continue;
                    };
                    let ink = Rect { x0: ix, y0: iy, x1: ix + side, y1: iy + side };
                    if ink.x1 > gray.w || ink.y1 > gray.h {
                        continue;
                    }
                    let mut finders = 0;
                    for (right, bottom) in [(false, false), (true, false), (false, true), (true, true)] {
                        finders += usize::from(finder(&view, ink, pitch, right, bottom)?);
                    }
                    if finders != 3 {
                        continue;
                    }
                    let errors = grid_error(&view, ink, pitch, budget)?;
                    if errors * 100 > ink.area() {
                        continue;
                    }
                    let margin = 4 * pitch;
                    let (Some(qx0), Some(qy0)) = (ink.x0.checked_sub(margin), ink.y0.checked_sub(margin)) else {
                        continue;
                    };
                    let quiet = Rect { x0: qx0, y0: qy0, x1: ink.x1 + margin, y1: ink.y1 + margin };
                    if quiet.x1 > gray.w || quiet.y1 > gray.h || !view.ring_clear(ink, quiet, budget)? {
                        continue;
                    }
                    let region = Region { format: "QR_CODE".into(), payload: result.getText().into(), ink, quiet, pitch: pitch as f32 };
                    if errors == 0 {
                        return Ok(Some(region));
                    }
                    if best.as_ref().is_none_or(|(prior, _)| errors < *prior) {
                        best = Some((errors, region));
                    }
                }
            }
        }
    }
    Ok(best.map(|(_, region)| region))
}

// std::time::Instant has no clock on bare wasm32-unknown-unknown. Keep the established
// renderer behavior there until a supported cancellable clock is supplied.
/// Decline detection on bare wasm, preserving ordinary smoothing without an unsupported clock.
#[cfg(target_arch = "wasm32")]
pub(crate) fn detect_with_divisor(_: usize, _: usize, _: &[u8], _: Option<&[u8]>, _: &AtomicBool, _: usize) -> Result<Vec<Region>, String> {
    Err("barcode regions unavailable on this target".into())
}

#[cfg(not(target_arch = "wasm32"))]
/// Decode one image within its deterministic share of the page's cooperative time/work limits.
pub(crate) fn detect_with_divisor(
    width: usize,
    height: usize,
    rgba: &[u8],
    mask: Option<&[u8]>,
    cancel: &AtomicBool,
    divisor: usize,
) -> Result<Vec<Region>, String> {
    let count = checked_pixels(width, height)?;
    let limits = ScanLimits::for_images(divisor, count)?;
    #[cfg(test)]
    let limits = ScanLimits { millis: TEST_DEADLINE.with(|v| v.get()).unwrap_or(limits.millis), ..limits };
    #[cfg(test)]
    TEST_SCAN_MILLIS.with(|v| v.set(limits.millis));
    #[cfg(test)]
    if let Some(result) = TEST_DECODER.with(|hook| hook.borrow_mut().as_mut().and_then(|hook| hook(cancel))) {
        return result;
    }
    let mut budget = Budget { cancel, start: Instant::now(), limits, visits: 0, calls: 0 };
    budget.poll()?;
    if rgba.len() != count.checked_mul(4).ok_or("RGBA overflow")? {
        return Err("RGBA length".into());
    }
    if let Some(mask) = mask {
        if mask.len() != count {
            return Err("mask length".into());
        }
        for block in mask.chunks(8192) {
            budget.charge(block.len())?;
            if block.iter().any(|a| *a != 255) {
                return Err("nonopaque mask".into());
            }
        }
    }
    let mut pixels = Vec::new();
    pixels.try_reserve_exact(count).map_err(|e| e.to_string())?;
    let (mut low, mut high) = (255u8, 0u8);
    for block in rgba.chunks(4 * 8192) {
        budget.charge(block.len() / 4)?;
        for p in block.as_chunks::<4>().0 {
            if p.get(3) != Some(&255) {
                return Err("nonopaque alpha".into());
            }
            let (r, g, b) = (*p.first().ok_or("red")?, *p.get(1).ok_or("green")?, *p.get(2).ok_or("blue")?);
            let value = ((77 * u32::from(r) + 150 * u32::from(g) + 29 * u32::from(b) + 128) >> 8) as u8;
            low = low.min(value);
            high = high.max(value);
            pixels.push(value);
        }
    }
    if high.saturating_sub(low) < 96 {
        return Err("low contrast".into());
    }
    let gray = Gray { w: width, h: height, pixels, threshold: ((u16::from(low) + u16::from(high)) / 2) as u8 };
    let mut regions = Vec::new();
    let mut hints = DecodeHints {
        PossibleFormats: Some(HashSet::from([BarcodeFormat::QR_CODE])),
        TryHarder: Some(false),
        AlsoInverted: Some(true),
        ..DecodeHints::default()
    };
    budget.call()?;
    match rxing::helpers::detect_multiple_in_luma_with_hints(gray.pixels.clone(), width as u32, height as u32, &mut hints) {
        Ok(results) => {
            if results.len() > 16 {
                return Err("native result budget".into());
            }
            for result in results {
                budget.poll()?;
                if let Some(region) = reconstruct_qr(&gray, &result, &mut budget)? {
                    insert(&mut regions, region)?;
                }
            }
        }
        Err(rxing::Exceptions::NotFoundException(_)) => {}
        Err(error) => return Err(format!("native QR error: {error:?}")),
    }
    scan_bars(&gray, &mut regions, &mut budget)?;
    budget.poll()?;
    Ok(regions)
}

/// Standalone geometry tests use the single-image page allowance.
#[cfg(test)]
pub(crate) fn detect(width: usize, height: usize, rgba: &[u8], mask: Option<&[u8]>, cancel: &AtomicBool) -> Result<Vec<Region>, String> {
    detect_with_divisor(width, height, rgba, mask, cancel, 1)
}

/// Validate nonzero sides up to 4096 and an area at most 4,194,304 using checked multiplication.
fn checked_pixels(w: usize, h: usize) -> Result<usize, String> {
    if w == 0 || h == 0 || w > 4096 || h > 4096 {
        return Err("dimensions".into());
    }
    w.checked_mul(h).filter(|p| *p <= 4_194_304).ok_or("pixel budget".into())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    const W: usize = 800;
    const H: usize = 800;
    // Code128-B "A": start 104, data 33, checksum 34, stop 106. Contributor-original
    // test raster generated from the standard's symbol bits, with independently known bounds.
    const SYMBOL: &str = "1101001000010100011000100010110001100011101011";
    /// Allocate the white synthetic source used by independent Code128 geometry tests.
    fn image() -> Vec<u8> {
        vec![255; W * H * 4]
    }
    /// Paint contributor-original Code128-B A at known coordinates and return its independent ink bounds.
    fn bars(data: &mut [u8], x: usize, y: usize) -> Rect {
        for row in y..y + 80 {
            for (column, bit) in SYMBOL.bytes().enumerate() {
                if bit == b'1' {
                    for dx in 0..3 {
                        let at = (row * W + x + column * 3 + dx) * 4;
                        data[at..at + 3].fill(0);
                    }
                }
            }
        }
        Rect { x0: x, y0: y, x1: x + SYMBOL.len() * 3, y1: y + 80 }
    }
    /// Verify identical payloads retain both complete physical regions through axis/direction changes.
    #[test]
    fn identical_payloads_keep_two_complete_locations_in_both_axes() {
        let mut data = image();
        let a = bars(&mut data, 100, 200);
        let b = bars(&mut data, 450, 200);
        let found = detect(W, H, &data, None, &AtomicBool::new(false)).unwrap();
        assert_eq!(found.len(), 2);
        for ink in [a, b] {
            let region = found.iter().find(|r| r.ink == ink).unwrap();
            assert_eq!(region.payload, "A");
            assert!(region.quiet.contains(Rect { x0: ink.x0 - 36, y0: ink.y0 - 4, x1: ink.x1 + 36, y1: ink.y1 + 4 }));
        }
        let mut rotated = image();
        for y in 0..H {
            for x in 0..W {
                let from = (y * W + x) * 4;
                let to = (x * W + (H - 1 - y)) * 4;
                rotated[to..to + 4].copy_from_slice(&data[from..from + 4]);
            }
        }
        let found = detect(H, W, &rotated, None, &AtomicBool::new(false)).unwrap();
        assert_eq!(found.len(), 2);
        for ink in [a, b] {
            assert!(found.iter().any(|r| r.ink == Rect { x0: H - ink.y1, y0: ink.x0, x1: H - ink.y0, y1: ink.x1 }));
        }
    }
    /// Ensure obstructed quiet zones and nonopaque masks never acquire barcode protection.
    #[test]
    fn quiet_zone_obstruction_and_mask_ambiguity_are_declined() {
        let mut data = image();
        bars(&mut data, 100, 200);
        for y in 210..270 {
            for x in 70..78 {
                let at = (y * W + x) * 4;
                data[at..at + 3].fill(0);
            }
        }
        assert!(detect(W, H, &data, None, &AtomicBool::new(false)).unwrap().is_empty());
        assert!(detect(W, H, &data, Some(&vec![128; W * H]), &AtomicBool::new(false)).is_err());
        assert!(detect(W, H, &data, None, &AtomicBool::new(true)).is_err());
    }
    /// Reject a synthetic Code128 checksum mutation instead of protecting a plausible bar pattern.
    #[test]
    fn corrupt_check_digit_does_not_create_a_region() {
        let mut data = image();
        let invalid = "1101001000010100011000100010001101100011101011";
        for y in 200..280 {
            for (x, bit) in invalid.bytes().enumerate() {
                if bit == b'1' {
                    for dx in 0..3 {
                        let at = (y * W + 100 + x * 3 + dx) * 4;
                        data[at..at + 3].fill(0);
                    }
                }
            }
        }
        assert!(detect(W, H, &data, None, &AtomicBool::new(false)).unwrap().is_empty());
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod review_regressions {
    use super::*;
    /// Verify EAN/UPC guard extensions and their full quiet rectangles against independent bounds.
    #[test]
    fn extended_retail_guards_are_inside_complete_regions() {
        for upc in [false, true] {
            let (data, ink, quiet) = crate::barcode_render_tests::retail_fixture(upc, true);
            let rgba = data.iter().flat_map(|v| [*v, *v, *v, 255]).collect::<Vec<_>>();
            let regions = detect(700, 1000, &rgba, None, &AtomicBool::new(false)).unwrap();
            assert_eq!(regions.len(), 1, "upc={upc}: {regions:?}");
            assert_eq!(regions[0].ink, ink);
            assert!(regions[0].quiet.contains(quiet));
        }
    }
    /// Check bounded row searching continues past rejected outer regions to a complete middle symbol.
    #[test]
    fn rejected_outer_symbols_do_not_hide_a_valid_middle_symbol() {
        const W: usize = 1000;
        const H: usize = 400;
        let bits = b"1101001000010100011000100010110001100011101011";
        let mut data = vec![255u8; W * H * 4];
        for x0 in [50, 350, 650] {
            for y in 200..280 {
                for (i, b) in bits.iter().enumerate() {
                    if *b == b'1' {
                        for x in x0 + i * 3..x0 + i * 3 + 3 {
                            data[(y * W + x) * 4..(y * W + x) * 4 + 3].fill(0);
                        }
                    }
                }
            }
        }
        for y in 200..280 {
            for x in [25, 810] {
                data[(y * W + x) * 4..(y * W + x) * 4 + 3].fill(0);
            }
        }
        let regions = detect(W, H, &data, None, &AtomicBool::new(false)).unwrap();
        assert_eq!(regions.len(), 1, "{regions:?}");
        assert_eq!(regions[0].ink.x0, 350);
    }
}

#[cfg(test)]
type TestDecoder = Box<dyn FnMut(&AtomicBool) -> Option<Result<Vec<Region>, String>>>;

#[cfg(test)]
std::thread_local! {
    static TEST_DECODER: std::cell::RefCell<Option<TestDecoder>> = const { std::cell::RefCell::new(None) };
}

/// Install a thread-local decoder fault hook for a test and restore the prior hook on unwind.
#[cfg(test)]
pub(crate) fn with_test_decoder<T>(hook: impl FnMut(&AtomicBool) -> Option<Result<Vec<Region>, String>> + 'static, run: impl FnOnce() -> T) -> T {
    struct Restore(Option<TestDecoder>);
    impl Drop for Restore {
        /// Restore the previous thread-local fault hook even when the test body unwinds.
        fn drop(&mut self) {
            TEST_DECODER.with(|hook| {
                hook.replace(self.0.take());
            });
        }
    }
    let _restore = Restore(TEST_DECODER.with(|slot| slot.replace(Some(Box::new(hook)))));
    run()
}

#[cfg(test)]
std::thread_local! {
    static TEST_SCAN_MILLIS: std::cell::Cell<u64> = const { std::cell::Cell::new(1500) };
    static TEST_RASTERS: std::cell::RefCell<Vec<(u32,u32)>> = const { std::cell::RefCell::new(Vec::new()) };
}
/// Expose the most recent test scan allowance without changing production time limits.
#[cfg(test)]
pub(crate) fn test_scan_millis() -> u64 {
    TEST_SCAN_MILLIS.with(std::cell::Cell::get)
}
/// Record a test detection raster size to compare whole-page and tile decode resolution.
#[cfg(test)]
pub(crate) fn observe_raster(width: u32, height: u32) {
    TEST_RASTERS.with(|v| v.borrow_mut().push((width, height)));
}
/// Drain thread-local raster observations so neighboring tests cannot inherit stale evidence.
#[cfg(test)]
pub(crate) fn take_rasters() -> Vec<(u32, u32)> {
    TEST_RASTERS.with(|v| std::mem::take(&mut *v.borrow_mut()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod page_limit_tests {
    use super::*;
    /// Verify a maximum-size source retains geometry work after its charged grayscale conversion.
    #[test]
    fn maximum_raster_keeps_work_after_preprocessing() {
        let stop = AtomicBool::new(false);
        let limits = ScanLimits::for_images(16, 4 * 1024 * 1024).unwrap();
        let mut b = Budget { cancel: &stop, start: Instant::now(), limits, visits: 0, calls: 0 };
        b.charge(4 * 1024 * 1024).unwrap();
        assert!(b.charge(1).is_ok(), "preprocessing cannot consume all geometry work");
    }
    /// Assert deterministic page shares and exact time/work/call refusals without timing-dependent sleeps.
    #[test]
    fn shared_page_time_visits_and_calls_are_enforced_without_sleeping() {
        let stop = AtomicBool::new(false);
        let limits = ScanLimits::for_images(16, 512 * 1024).unwrap();
        assert!(limits.millis * 16 <= 1500);
        assert!(limits.visits * 16 <= 80 * 1024 * 1024);
        assert!(limits.calls * 16 <= 4096);
        let mut b = Budget { cancel: &stop, start: Instant::now() - Duration::from_millis(limits.millis + 1), limits, visits: 0, calls: 0 };
        assert_eq!(b.poll(), Err("deadline".into()));
        b.start = Instant::now();
        assert_eq!(b.charge(limits.visits + 1), Err("pixel visit budget".into()));
        b.calls = limits.calls;
        assert_eq!(b.call(), Err("native call budget".into()));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod rejected_payload_regression {
    use super::*;
    /// Verify checksum-valid but unsupported outer payloads do not hide a supported middle location.
    #[test]
    fn unsupported_outer_payloads_do_not_hide_a_supported_middle() {
        const W: usize = 1000;
        const H: usize = 400;
        let outer = b"1101001000010111101000101111000101100011101011";
        let middle = b"1101001000010100011000100010110001100011101011";
        let mut rgba = vec![255; W * H * 4];
        for (x0, bits) in [(50, outer), (350, middle), (650, outer)] {
            for y in 200..280 {
                for (i, bit) in bits.iter().enumerate() {
                    if *bit == b'1' {
                        for x in x0 + i * 3..x0 + i * 3 + 3 {
                            rgba[(y * W + x) * 4..(y * W + x) * 4 + 3].fill(0);
                        }
                    }
                }
            }
        }
        let found = detect(W, H, &rgba, None, &AtomicBool::new(false)).unwrap();
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].ink.x0, 350);
    }
}

// These thread-local probes isolate deterministic call exhaustion from machine speed.
// Production limits and cancellation remain unchanged.
#[cfg(test)]
std::thread_local! {
    static TEST_DEADLINE: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
    static TEST_NATIVE_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
/// Drain the test native-call counter, excluding reservations that failed before decoder entry.
#[cfg(test)]
pub(crate) fn take_native_calls() -> usize {
    TEST_NATIVE_CALLS.with(|n| n.replace(0))
}
/// Temporarily disable only the test deadline to isolate deterministic call exhaustion.
/// Restore the prior override on unwind; cancellation and count/visit limits stay active.
#[cfg(test)]
pub(crate) fn without_test_deadline<T>(run: impl FnOnce() -> T) -> T {
    struct Restore(Option<u64>);
    impl Drop for Restore {
        /// Restore the prior test clock override without weakening subsequent tests' production limits.
        fn drop(&mut self) {
            TEST_DEADLINE.with(|v| v.set(self.0));
        }
    }
    let _restore = Restore(TEST_DEADLINE.with(|v| v.replace(Some(u64::MAX))));
    run()
}
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod call_exhaustion_tests {
    use super::*;
    use crate::barcode_render_tests::call_limit_fixture;
    /// Retain complete regions at the call cap while rejecting invalid candidates and other work failures.
    #[test]
    fn call_exhaustion_retains_only_fully_validated_regions() {
        let stop = AtomicBool::new(false);
        for kind in [0, 1, 2, 3] {
            let (pixels, ink, quiet) = call_limit_fixture(0, kind);
            let gray = Gray { w: 1400, h: 1400, pixels, threshold: 127 };
            let limits = ScanLimits { millis: u64::MAX, ..ScanLimits::for_images(4, 1400 * 1400).unwrap() };
            let mut b = Budget { cancel: &stop, start: Instant::now(), limits, visits: 0, calls: 1 };
            let mut regions = Vec::new();
            take_native_calls();
            let result = scan_bars(&gray, &mut regions, &mut b);
            let calls = take_native_calls();
            println!("kind={kind} result={result:?} validated={} calls={calls} quota={} visits={}", regions.len(), limits.calls, b.visits);
            assert!(calls <= 1023);
            if kind == 0 {
                assert_eq!(regions.len(), 1);
                assert_eq!(regions[0].ink, ink);
                assert_eq!(regions[0].quiet, quiet);
                assert!(result.is_ok(), "later call exhaustion discarded a complete region: {result:?}");
                // A validated prefix never bypasses cancellation, deadline or pixel limits.
                for failure in 0..3 {
                    let mut prefix = regions.clone();
                    let mut b = Budget { cancel: &stop, start: Instant::now(), limits, visits: 0, calls: limits.calls };
                    if failure == 0 {
                        stop.store(true, Ordering::Relaxed);
                    }
                    if failure == 1 {
                        b.limits.millis = 0;
                        b.start -= Duration::from_millis(1);
                    }
                    if failure == 2 {
                        b.visits = limits.visits;
                    }
                    assert!(scan_bars(&gray, &mut prefix, &mut b).is_err());
                    stop.store(false, Ordering::Relaxed);
                }
            } else {
                assert!(regions.is_empty(), "invalid candidate was protected: {regions:?}");
                assert_eq!(result, Err("native call budget".into()));
            }
        }
    }
}

#[cfg(test)]
std::thread_local! { static TEST_GRAY_ERROR_CONSTRUCTIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod gray_error_regression {
    use super::*;
    /// Valid pixel reads must not build failure strings; malformed storage retains exact checked errors.
    #[test]
    fn successful_gray_reads_do_not_construct_errors() {
        TEST_GRAY_ERROR_CONSTRUCTIONS.with(|v| v.set(0));
        let gray = Gray { w: 8, h: 8, pixels: vec![123; 64], threshold: 128 };
        for y in 0..8 {
            for x in 0..8 {
                assert_eq!(gray.get(x, y).unwrap(), 123);
            }
        }
        let count = TEST_GRAY_ERROR_CONSTRUCTIONS.with(|v| v.get());
        println!("successful_reads=64 error_string_constructions={count}");
        assert_eq!(count, 0, "valid pixel reads eagerly construct and drop an error String");
        let short = Gray { pixels: vec![123; 63], ..gray };
        assert_eq!(short.get(7, 7), Err("short gray source".into()));
        assert_eq!(TEST_GRAY_ERROR_CONSTRUCTIONS.with(|v| v.get()), 1);
        assert_eq!(short.get(8, 0), Err("pixel outside source".into()));
        let overflow = Gray { w: usize::MAX, h: 2, pixels: Vec::new(), threshold: 128 };
        assert_eq!(overflow.get(1, 1), Err("pixel overflow".into()));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod row_materialization_tests {
    use super::*;
    /// Match checked single-pixel reads for every orientation/polarity and non-square scan dimension.
    #[test]
    fn strided_rows_match_checked_pixel_reads() {
        for (w, h) in [(1, 1), (1, 19), (17, 1), (13, 19), (32, 7)] {
            for threshold in [0, 1, 127, 128, 254, 255] {
                let gray = Gray { w, h, pixels: (0..w * h).map(|i| ((i * 73 + 31) % 256) as u8).collect(), threshold };
                for inverted in [false, true] {
                    for vertical in [false, true] {
                        for reverse in [false, true] {
                            let view = View { gray: &gray, vertical, reverse, inverted };
                            for y in 0..view.height() {
                                let expected = (0..view.width()).map(|x| view.dark(x, y)).collect::<Result<Vec<_>, _>>().unwrap();
                                assert_eq!(view.row_bits(y).unwrap(), expected);
                            }
                            assert_eq!(view.row_bits(view.height()), Err("view outside source".into()));
                        }
                    }
                }
            }
        }
    }
    /// Refuse truncated backing storage and overflowing addresses without a panic or partial row.
    #[test]
    fn invalid_row_storage_is_rejected() {
        for vertical in [false, true] {
            for reverse in [false, true] {
                let gray = Gray { w: 13, h: 7, pixels: vec![255; 90], threshold: 127 };
                let view = View { gray: &gray, vertical, reverse, inverted: false };
                assert_eq!(view.row_bits(view.height() - 1), Err("short gray source".into()));
            }
        }
        let gray = Gray { w: usize::MAX, h: 2, pixels: Vec::new(), threshold: 127 };
        let view = View { gray: &gray, vertical: false, reverse: false, inverted: false };
        assert_eq!(view.row_bits(1), Err("pixel overflow".into()));
        let gray = Gray { w: 0, h: 1, pixels: Vec::new(), threshold: 127 };
        assert!(View { gray: &gray, vertical: false, reverse: false, inverted: false }.row_bits(0).is_err());
        assert!(View { gray: &gray, vertical: true, reverse: false, inverted: false }.row_bits(0).is_err());
    }
}
