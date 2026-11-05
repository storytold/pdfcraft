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
    pub(crate) fn area(self) -> usize {
        self.x1.saturating_sub(self.x0) * self.y1.saturating_sub(self.y0)
    }
    pub(crate) fn overlap(self, b: Self) -> usize {
        self.x1.min(b.x1).saturating_sub(self.x0.max(b.x0)) * self.y1.min(b.y1).saturating_sub(self.y0.max(b.y0))
    }
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

struct Budget<'a> {
    cancel: &'a AtomicBool,
    start: Instant,
    visits: usize,
    calls: usize,
}
impl Budget<'_> {
    fn poll(&self) -> Result<(), String> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        if self.start.elapsed() > Duration::from_millis(1500) {
            return Err("deadline".into());
        }
        Ok(())
    }
    fn charge(&mut self, n: usize) -> Result<(), String> {
        self.poll()?;
        self.visits = self.visits.checked_add(n).ok_or("work overflow")?;
        if self.visits > 64 * 1024 * 1024 {
            return Err("pixel visit budget".into());
        }
        Ok(())
    }
    fn call(&mut self) -> Result<(), String> {
        self.poll()?;
        self.calls += 1;
        if self.calls > 4096 {
            return Err("native call budget".into());
        }
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
    fn get(&self, x: usize, y: usize) -> Result<u8, String> {
        if x >= self.w || y >= self.h {
            return Err("pixel outside source".into());
        }
        self.pixels.get(y.checked_mul(self.w).and_then(|v| v.checked_add(x)).ok_or("pixel overflow")?).copied().ok_or("short gray source".into())
    }
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
    fn width(&self) -> usize {
        if self.vertical { self.gray.h } else { self.gray.w }
    }
    fn height(&self) -> usize {
        if self.vertical { self.gray.w } else { self.gray.h }
    }
    fn dark(&self, u: usize, v: usize) -> Result<bool, String> {
        if u >= self.width() || v >= self.height() {
            return Err("view outside source".into());
        }
        let u = if self.reverse { self.width() - 1 - u } else { u };
        let (x, y) = if self.vertical { (v, u) } else { (u, v) };
        self.gray.dark(x, y, self.inverted)
    }
    fn source_rect(&self, r: Rect) -> Rect {
        let r = if self.reverse { Rect { x0: self.width() - r.x1, x1: self.width() - r.x0, ..r } } else { r };
        if self.vertical { Rect { x0: r.y0, y0: r.x0, x1: r.y1, y1: r.x1 } } else { r }
    }
    fn view_rect(&self, r: Rect) -> Rect {
        let r = if self.vertical { Rect { x0: r.y0, y0: r.x0, x1: r.y1, y1: r.x1 } } else { r };
        if self.reverse { Rect { x0: self.width() - r.x1, x1: self.width() - r.x0, ..r } } else { r }
    }
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

/// Revalidate native Code128 framing/checksum for all three start code sets.
fn valid_code128(result: &RXingResult) -> bool {
    let raw = result.getRawBytes();
    let text = result.getText();
    if raw.len() < 4 || raw.len() > 259 || text.is_empty() || text.len() > 256 || !text.bytes().all(|c| (32..127).contains(&c)) {
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
    if !(if ean { valid_ean(result.getText()) } else { valid_code128(result) }) {
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

fn reconstruct_bar(view: &View<'_>, y: usize, bits: &[bool], result: &RXingResult, budget: &mut Budget<'_>) -> Result<Option<Region>, String> {
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
                    let mut bits = Vec::with_capacity(view.width());
                    for x in 0..view.width() {
                        bits.push(view.dark(x, y)?);
                    }
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
                        budget.call()?;
                        let result = MultiFormatOneDReader::new(&hints).decode_row(y as u32, &row, &hints);
                        let Ok(result) = result else {
                            break;
                        };
                        let Some(region) = reconstruct_bar(&view, y, &bits, &result, budget)? else {
                            break;
                        };
                        insert(regions, region)?;
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
#[cfg(target_arch = "wasm32")]
pub(crate) fn detect(_: usize, _: usize, _: &[u8], _: Option<&[u8]>, _: &AtomicBool) -> Result<Vec<Region>, String> {
    Err("barcode regions unavailable on this target".into())
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn detect(width: usize, height: usize, rgba: &[u8], mask: Option<&[u8]>, cancel: &AtomicBool) -> Result<Vec<Region>, String> {
    #[cfg(test)]
    if let Some(result) = TEST_DECODER.with(|hook| hook.borrow_mut().as_mut().and_then(|hook| hook(cancel))) {
        return result;
    }
    let mut budget = Budget { cancel, start: Instant::now(), visits: 0, calls: 0 };
    budget.poll()?;
    let count = checked_pixels(width, height)?;
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
    fn image() -> Vec<u8> {
        vec![255; W * H * 4]
    }
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
type TestDecoder = Box<dyn FnMut(&AtomicBool) -> Option<Result<Vec<Region>, String>>>;

#[cfg(test)]
std::thread_local! {
    static TEST_DECODER: std::cell::RefCell<Option<TestDecoder>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(crate) fn with_test_decoder<T>(hook: impl FnMut(&AtomicBool) -> Option<Result<Vec<Region>, String>> + 'static, run: impl FnOnce() -> T) -> T {
    struct Restore(Option<TestDecoder>);
    impl Drop for Restore {
        fn drop(&mut self) {
            TEST_DECODER.with(|hook| {
                hook.replace(self.0.take());
            });
        }
    }
    let _restore = Restore(TEST_DECODER.with(|slot| slot.replace(Some(Box::new(hook)))));
    run()
}
