// PdfCraft patch (1).
//! Decoding a large tile on several threads.
//!
//! Two stages of a tile spread over threads: decoding the bitplanes of its code-blocks, and the
//! inverse wavelet transform with the store of each component. Every code-block and every
//! component is decoded by the same code as on one thread, from the same inputs, into the same
//! place, so the samples are the same bit for bit. Whatever this module cannot prove is safe to
//! split (a layout it does not expect, a tile too small to be worth it, a single core) it declines,
//! and the caller decodes on its own thread as before.
//!
//! Every decode in the process draws on two shared budgets: helper threads ([`helper_budget`]) and
//! the memory of the extra wavelet-transform buffers ([`IDWT_BUDGET`]). What one decode holds,
//! another cannot take, so render workers decoding at once do not multiply either.

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};
use std::thread;
use std::vec::Vec;

use super::super::bitplane::{self, BitPlaneDecodeBuffers, BitPlaneDecodeContext};
use super::super::build::{CodeBlock, Decomposition, SubBand};
use super::super::codestream::{CodeBlockStyle, Header};
use super::super::tile::Tile;
use super::{
    ComponentData, DecompositionStorage, TileDecodeContext, idwt, store, sub_band_parameters,
};
use crate::error::{DecodeError, Result};

/// The most threads one decode uses.
const MAX_THREADS: usize = 8;

/// Tiles with fewer coefficients than this decode on the calling thread: starting threads would
/// cost more than they save. About a 512 × 512 image of one component.
const MIN_COEFFICIENTS: usize = 1 << 18;

thread_local! {
    /// Set by [`set_decode_threads`]: the number of threads decodes started on this thread use.
    static THREADS: Cell<Option<usize>> = const { Cell::new(None) };
}

/// Use at most `threads` threads for the decodes started on the calling thread (1 decodes
/// everything on it), or the default with `None`. For tests that compare the two.
pub(crate) fn set_decode_threads(threads: Option<usize>) {
    THREADS.with(|t| t.set(threads));
}

fn threads() -> usize {
    THREADS
        .with(Cell::get)
        .unwrap_or_else(|| thread::available_parallelism().map_or(1, |n| n.get()))
        .clamp(1, MAX_THREADS)
}

/// Helper threads running for decodes anywhere in the process. Render workers can decode several
/// images (or tiles of one) at once; together their helpers stay within [`helper_budget`].
static HELPERS: AtomicUsize = AtomicUsize::new(0);

/// The most bytes the wavelet-transform buffers of all decodes that run the transform on several
/// threads hold together. Each of those threads needs buffers of its own (about 5 bytes a pixel of
/// the tile), where a decode on one thread reuses one set, so a large tile runs on fewer threads,
/// or on its calling thread as before, rather than past this.
const IDWT_BUDGET: usize = 256 << 20;

/// The bytes of [`IDWT_BUDGET`] taken.
static IDWT_BYTES: AtomicUsize = AtomicUsize::new(0);

/// The most helper threads all decodes together run: one fewer than the machine has, since every
/// decode also works on its calling thread.
fn helper_budget() -> usize {
    thread::available_parallelism()
        .map_or(1, |n| n.get())
        .saturating_sub(1)
}

/// Part of a process-wide budget, given back when dropped.
struct Reservation {
    pool: &'static AtomicUsize,
    /// What is held, in the pool's measure.
    amount: usize,
    /// How many of the units asked for that is.
    units: usize,
}

impl Reservation {
    /// Up to `wanted` units of `unit` each from `pool`, which holds at most `budget`: fewer, or
    /// none, while other decodes hold the rest.
    fn take(pool: &'static AtomicUsize, budget: usize, unit: usize, wanted: usize) -> Self {
        let unit = unit.max(1);
        let mut active = pool.load(Ordering::Acquire);
        loop {
            let units = wanted.min(budget.saturating_sub(active) / unit);
            let amount = units.saturating_mul(unit);
            match pool.compare_exchange_weak(
                active,
                active.saturating_add(amount),
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    return Self {
                        pool,
                        amount,
                        units,
                    };
                }
                Err(now) => active = now,
            }
        }
    }

    /// Up to `wanted` helper threads.
    fn helpers(wanted: usize) -> Self {
        Self::take(&HELPERS, helper_budget(), 1, wanted)
    }

    /// The wavelet-transform buffers of up to `wanted` threads, `per_thread` bytes each.
    fn idwt_memory(per_thread: usize, wanted: usize) -> Self {
        Self::take(&IDWT_BYTES, IDWT_BUDGET, per_thread, wanted)
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        self.pool.fetch_sub(self.amount, Ordering::AcqRel);
    }
}

/// Runs `work` on up to `threads` threads, the calling one included, with as many helpers as the
/// shared budget allows. A thread the system will not start is simply not used: the calling thread
/// does the work that thread would have done.
fn run_on_threads(threads: usize, work: &(impl Fn() + Sync)) {
    // Held until every helper has joined at the end of the scope, panics included.
    let helpers = Reservation::helpers(threads.saturating_sub(1));
    thread::scope(|scope| {
        for _ in 0..helpers.units {
            if thread::Builder::new().spawn_scoped(scope, work).is_err() {
                break;
            }
        }
        work();
    });
    drop(helpers);
}

/// The code-blocks of one sub-band that share their first row. They cover whole rows of the
/// sub-band, which lie in one contiguous run of the coefficients, so a stripe can be written by one
/// thread while others write other stripes.
struct Stripe<'s> {
    /// In the order the serial decode takes them, so that overlapping code-blocks (which a valid
    /// codestream does not have) end as they would there.
    code_blocks: Vec<&'s CodeBlock>,
    sub_band: &'s SubBand,
    style: &'s CodeBlockStyle,
    dequantization_step: f32,
    num_bitplanes: u8,
    /// The stripe's first row, in the coordinates of the sub-band's rectangle.
    first_row: u32,
    /// The stripe's coefficients: `start..end` of `DecompositionStorage::coefficients`.
    start: usize,
    end: usize,
}

/// Decode the bitplanes of every code-block of `tile` on several threads, as
/// `decode_component_tile_bit_planes` does on one. `None` declines: nothing was written.
pub(super) fn decode_bit_planes(
    tile: &Tile<'_>,
    storage: &mut DecompositionStorage<'_>,
    header: &Header<'_>,
) -> Option<Result<()>> {
    let threads = threads();
    if threads < 2 || storage.coefficients.len() < MIN_COEFFICIENTS {
        return None;
    }

    // The code-blocks are read through `storage` while the threads write the coefficients, which
    // the bitplane decoder never reads: they are taken out for that time and always put back.
    let mut coefficients = core::mem::take(&mut storage.coefficients);
    let blocks: &DecompositionStorage<'_> = storage;
    let result = match plan_stripes(tile, blocks, header) {
        None => None,
        Some(Err(e)) => Some(Err(e)),
        Some(Ok(stripes)) => carve(&mut coefficients, stripes)
            .map(|jobs| decode_stripes(jobs, threads, blocks, header.strict)),
    };
    storage.coefficients = coefficients;
    result
}

/// The stripes of every sub-band the serial decode visits, sorted by where their coefficients lie.
/// `None` when the layout is not one this module splits; `Some(Err)` for an error the serial decode
/// would also stop with.
fn plan_stripes<'s>(
    tile: &'s Tile<'_>,
    storage: &'s DecompositionStorage<'_>,
    header: &Header<'_>,
) -> Option<Result<Vec<Stripe<'s>>>> {
    let mut stripes: Vec<Stripe<'s>> = Vec::new();
    for (component_idx, component_info) in tile.component_infos.iter().enumerate() {
        let resolutions = component_info
            .num_resolution_levels()
            .checked_sub(header.skipped_resolution_levels)?;
        let tile_decompositions = storage.tile_decompositions.get(component_idx)?;
        for resolution in 0..resolutions {
            if resolution > 0
                && storage
                    .decompositions
                    .get(tile_decompositions.decompositions.clone())?
                    .get(resolution as usize - 1)
                    .is_none()
            {
                return None;
            }
            for sub_band_idx in
                tile_decompositions.sub_band_iter(resolution, &storage.decompositions)
            {
                let sub_band = storage.sub_bands.get(sub_band_idx)?;
                let (dequantization_step, num_bitplanes) =
                    match sub_band_parameters(sub_band, resolution, component_info) {
                        Ok(parameters) => parameters,
                        Err(e) => return Some(Err(e)),
                    };
                let width = sub_band.rect.width() as usize;
                let first = stripes.len();
                for precinct_idx in sub_band.precincts.clone() {
                    let precinct = storage.precincts.get(precinct_idx)?;
                    for code_block_idx in precinct.code_blocks.clone() {
                        let code_block = storage.code_blocks.get(code_block_idx)?;
                        let (r, s) = (&code_block.rect, &sub_band.rect);
                        if r.x0 < s.x0
                            || r.x1 > s.x1
                            || r.y0 < s.y0
                            || r.y1 > s.y1
                            || r.x0 > r.x1
                            || r.y0 > r.y1
                        {
                            return None;
                        }
                        let (first_row, last_row) = (r.y0 - s.y0, r.y1 - s.y0);
                        match stripes[first..]
                            .iter_mut()
                            .find(|st| st.first_row == first_row)
                        {
                            Some(stripe) => {
                                let end = sub_band
                                    .coefficients
                                    .start
                                    .checked_add((last_row as usize).checked_mul(width)?)?;
                                stripe.end = stripe.end.max(end);
                                stripe.code_blocks.push(code_block);
                            }
                            None => stripes.push(Stripe {
                                code_blocks: alloc::vec![code_block],
                                sub_band,
                                style: &component_info.coding_style.parameters.code_block_style,
                                dequantization_step,
                                num_bitplanes,
                                first_row,
                                start: sub_band
                                    .coefficients
                                    .start
                                    .checked_add((first_row as usize).checked_mul(width)?)?,
                                end: sub_band
                                    .coefficients
                                    .start
                                    .checked_add((last_row as usize).checked_mul(width)?)?,
                            }),
                        }
                    }
                }
                // Every stripe stays inside its sub-band's coefficients.
                if stripes[first..]
                    .iter()
                    .any(|st| st.end > sub_band.coefficients.end)
                {
                    return None;
                }
            }
        }
    }

    // No two stripes may share a coefficient, or two threads would write it.
    stripes.sort_by_key(|st| (st.start, st.end));
    if stripes.windows(2).any(|pair| pair[0].end > pair[1].start) {
        return None;
    }
    Some(Ok(stripes))
}

/// Pairs each stripe with its own slice of `coefficients`. `None` if a stripe lies outside them.
fn carve<'s, 'c>(
    coefficients: &'c mut [f32],
    stripes: Vec<Stripe<'s>>,
) -> Option<Vec<(Stripe<'s>, &'c mut [f32])>> {
    let mut jobs = Vec::with_capacity(stripes.len());
    let mut rest = coefficients;
    let mut consumed = 0;
    for stripe in stripes {
        let (_, tail) = rest.split_at_mut_checked(stripe.start.checked_sub(consumed)?)?;
        let (out, tail) = tail.split_at_mut_checked(stripe.end.checked_sub(stripe.start)?)?;
        rest = tail;
        consumed = stripe.end;
        jobs.push((stripe, out));
    }
    // The largest stripes first, so that no thread is left with a large one at the end.
    jobs.sort_by_key(|(stripe, _)| core::cmp::Reverse(stripe.end - stripe.start));
    Some(jobs)
}

fn decode_stripes(
    jobs: Vec<(Stripe<'_>, &mut [f32])>,
    threads: usize,
    storage: &DecompositionStorage<'_>,
    strict: bool,
) -> Result<()> {
    let queue = Mutex::new(jobs.into_iter());
    let failure: Mutex<Option<DecodeError>> = Mutex::new(None);
    let stop = AtomicBool::new(false);

    run_on_threads(threads, &|| {
        let mut ctx = BitPlaneDecodeContext::default();
        let mut buffers = BitPlaneDecodeBuffers::default();
        while !stop.load(Ordering::Relaxed) {
            // A poisoned lock means another thread panicked; the scope passes that panic on.
            let Some((stripe, out)) = queue.lock().ok().and_then(|mut q| q.next()) else {
                break;
            };
            if let Err(e) = decode_stripe(&stripe, out, &mut ctx, &mut buffers, storage, strict) {
                stop.store(true, Ordering::Relaxed);
                if let Ok(mut failure) = failure.lock() {
                    failure.get_or_insert(e);
                }
            }
        }
    });

    // Only a panicking thread poisons the lock, and the scope has passed its panic on already.
    failure
        .into_inner()
        .unwrap_or_else(PoisonError::into_inner)
        .map_or(Ok(()), Err)
}

/// Decodes the code-blocks of `stripe` into `out`, its coefficients, as
/// `decode_sub_band_bitplanes` decodes them into the whole sub-band.
fn decode_stripe(
    stripe: &Stripe<'_>,
    out: &mut [f32],
    ctx: &mut BitPlaneDecodeContext,
    buffers: &mut BitPlaneDecodeBuffers,
    storage: &DecompositionStorage<'_>,
    strict: bool,
) -> Result<()> {
    let sub_band = stripe.sub_band;
    let width = sub_band.rect.width() as usize;
    for code_block in &stripe.code_blocks {
        bitplane::decode_with(
            code_block,
            sub_band.sub_band_type,
            stripe.num_bitplanes,
            stripe.style,
            ctx,
            buffers,
            storage,
            strict,
        )?;

        // Turn the signs and magnitudes into singular coefficients and copy them into the stripe.
        // `plan_stripes` checked that the code-block lies inside the sub-band and its rows inside
        // the stripe.
        let x_offset = (code_block.rect.x0 - sub_band.rect.x0) as usize;
        let y_offset = (code_block.rect.y0 - sub_band.rect.y0 - stripe.first_row) as usize;
        let mut base_idx = y_offset * width + x_offset;

        for coefficients in ctx.coefficient_rows() {
            let Some(out_row) = out.get_mut(base_idx..) else {
                break;
            };

            for (output, coefficient) in out_row.iter_mut().zip(coefficients.iter().copied()) {
                *output = coefficient.get() as f32;
                *output *= stripe.dequantization_step;
            }

            base_idx += width;
        }
    }
    Ok(())
}

/// The bytes of the wavelet-transform buffers one thread holds for component `idx`: the output at
/// the size of the last decomposition decoded and the scratch buffer at the one before, as
/// `idwt::apply` reserves them. `None` if the decompositions are not where the storage says.
fn idwt_bytes(
    storage: &DecompositionStorage<'_>,
    header: &Header<'_>,
    idx: usize,
) -> Option<usize> {
    let tile_decompositions = storage.tile_decompositions.get(idx)?;
    let decompositions = storage
        .decompositions
        .get(tile_decompositions.decompositions.clone())?;
    let decompositions = decompositions.get(
        ..decompositions
            .len()
            .saturating_sub(header.skipped_resolution_levels as usize),
    )?;
    // Sub-bands can be shifted by one, so `idwt::apply` pads each side by one.
    let size = |d: &Decomposition| {
        (d.rect.width() as usize)
            .checked_add(1)?
            .checked_mul((d.rect.height() as usize).checked_add(1)?)
    };
    let samples = match decompositions {
        [] => storage
            .sub_bands
            .get(tile_decompositions.first_ll_sub_band)?
            .coefficients
            .len(),
        [last] => size(last)?,
        [.., before, last] => size(last)?.checked_add(size(before)?)?,
    };
    samples.checked_mul(size_of::<f32>())
}

/// The inverse wavelet transform and the store of each component of `tile`, one component per
/// thread, as `decode_tile` runs them one after another. `false` declines: nothing was written.
pub(super) fn idwt_and_store(
    tile: &Tile<'_>,
    header: &Header<'_>,
    storage: &DecompositionStorage<'_>,
    channel_data: &mut [ComponentData],
) -> bool {
    let components = header.component_infos.len();
    let threads = threads().min(components);
    let tile_area = (tile.rect.width() as usize).saturating_mul(tile.rect.height() as usize);
    if threads < 2
        || tile_area < MIN_COEFFICIENTS
        || channel_data.len() != components
        || storage.tile_decompositions.len() < components
    {
        return false;
    }

    // A thread keeps its buffers for every component it takes, at the size of the largest.
    let Some(per_thread) = (0..components).try_fold(0, |most: usize, idx| {
        Some(most.max(idwt_bytes(storage, header, idx)?))
    }) else {
        return false;
    };
    let memory = Reservation::idwt_memory(per_thread, threads);
    let threads = memory.units;
    if threads < 2 {
        return false;
    }

    let queue = Mutex::new(
        header
            .component_infos
            .iter()
            .zip(channel_data.iter_mut())
            .enumerate(),
    );
    run_on_threads(threads, &|| {
        let mut tile_ctx = TileDecodeContext::default();
        // A poisoned lock means another thread panicked; the scope passes that panic on.
        while let Some((idx, (component_info, channel))) =
            queue.lock().ok().and_then(|mut q| q.next())
        {
            idwt::apply(
                storage,
                &mut tile_ctx,
                idx,
                header,
                component_info.wavelet_transform(),
            );
            store(tile, header, &mut tile_ctx, channel, component_info);
        }
    });
    drop(memory);
    true
}

#[cfg(test)]
mod tests {
    use super::{IDWT_BUDGET, Reservation, helper_budget};

    /// Concurrent decodes share one budget of helper threads: what one holds, another cannot
    /// take, and it is free again once dropped.
    #[test]
    fn decodes_share_one_helper_budget() {
        let budget = helper_budget();
        let first = Reservation::helpers(budget + 5);
        assert_eq!(first.units, budget);
        assert_eq!(Reservation::helpers(3).units, 0);
        drop(first);
        assert_eq!(Reservation::helpers(3).units, 3.min(budget));
    }

    /// The transform buffers of all decodes share one memory budget: a tile whose threads would
    /// pass it gets fewer, and what one decode holds, another cannot take until it is dropped.
    #[test]
    fn decodes_share_one_idwt_memory_budget() {
        // A 3320 x 4676 tile (about 78 MB a thread) keeps three threads for three components.
        let scan = (3321 * 4677 + 1661 * 2339) * size_of::<f32>();
        assert_eq!(Reservation::idwt_memory(scan, 3).units, 3);

        // A 7016 x 9933 tile (about 350 MB a thread) gets none: it decodes on one thread.
        let poster = (7017 * 9934 + 3509 * 4968) * size_of::<f32>();
        assert_eq!(Reservation::idwt_memory(poster, 3).units, 0);

        let first = Reservation::idwt_memory(IDWT_BUDGET / 4, 8);
        assert_eq!(first.units, 4);
        assert_eq!(Reservation::idwt_memory(1, 8).units, 0);
        drop(first);
        assert_eq!(Reservation::idwt_memory(IDWT_BUDGET / 4, 2).units, 2);
    }
}
