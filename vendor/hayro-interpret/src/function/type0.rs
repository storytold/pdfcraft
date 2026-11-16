use crate::function::{Clamper, TupleVec, Values, interpolate};
use hayro_syntax::bit_reader::BitReader;
use hayro_syntax::object::Array;
use hayro_syntax::object::Stream;
use hayro_syntax::object::dict::keys::{BITS_PER_SAMPLE, DECODE, ENCODE, SIZE};
use smallvec::SmallVec;

/// A type 0 function (sampled function).
#[derive(Debug)]
pub(crate) struct Type0 {
    sizes: IntVec,
    /// PdfCraft patch (31): the samples in stream order, `range.len()` to a grid point, the
    /// first input varying fastest. They were a hash map keyed by each grid point, built again
    /// every time a shading using the function was painted.
    samples: Vec<u32>,
    /// PdfCraft patch: how many `samples` elements one step along each input moves (`stride * range.len()`).
    elem_strides: SmallVec<[usize; 4]>,
    clamper: Clamper,
    range: TupleVec,
    encode: TupleVec,
    /// PdfCraft patch: precomputed `2^bits_per_sample - 1` and `(decode_min, decode_max - decode_min)`.
    max_sample: f32,
    decode_params: SmallVec<[(f32, f32); 4]>,
}

impl Type0 {
    /// Create a new type 0 function.
    pub(crate) fn new(stream: &Stream<'_>) -> Option<Self> {
        let dict = stream.dict();
        let bits_per_sample = dict.get::<u8>(BITS_PER_SAMPLE)?;

        if !matches!(bits_per_sample, 1 | 2 | 4 | 8 | 16 | 24 | 32) {
            error!("invalid bits per sample: {bits_per_sample}");

            return None;
        }

        let clamper = Clamper::new(dict)?;
        let range = clamper.range.clone()?;

        if range.is_empty() {
            warn!("encountered Type0 function with invalid range length 0.");

            return None;
        }

        let sizes = dict
            .get::<Array<'_>>(SIZE)?
            .iter::<u32>()
            .collect::<IntVec>();

        let encode = dict.get::<TupleVec>(ENCODE).unwrap_or(
            sizes
                .iter()
                .map(|s| (0.0, s.saturating_sub(1) as f32))
                .collect(),
        );

        // PdfCraft patch: see `MAX_SAMPLE_VALUES`.
        let Some(num_expected_entries) = sizes
            .iter()
            .try_fold(1usize, |n, s| n.checked_mul(*s as usize))
            .filter(|entries| *entries <= MAX_SAMPLE_ENTRIES)
            .and_then(|entries| entries.checked_mul(range.len()))
            .filter(|n| *n <= MAX_SAMPLE_VALUES)
        else {
            warn!("Type0 function has too many samples");

            return None;
        };

        let decode = dict.get::<TupleVec>(DECODE).unwrap_or(range.clone());
        // PdfCraft patch (31): `2_u32.pow(32)` overflowed (a panic with overflow checks on).
        let max_sample = ((1u64 << bits_per_sample) - 1) as f32;
        let decode_params = decode
            .iter()
            .take(range.len())
            .map(|&(d0, d1)| (d0, d1 - d0))
            .collect();

        let mut data = {
            let decoded = stream.decoded().ok()?;
            let mut buf = vec![];
            let mut reader = BitReader::new(&decoded);

            while buf.len() < num_expected_entries
                && let Some(data) = reader.read(bits_per_sample)
            {
                buf.push(data);
            }

            buf
        };

        if data.len() != num_expected_entries {
            warn!("Type0 function didn't have the expected number of sample entries.");
            data.truncate(num_expected_entries);
        }

        // Saturating: the grid has at most `MAX_SAMPLE_ENTRIES` points unless a size is 0, and
        // `elem_strides` is used only after verifying every size > 0.
        let mut elem_strides = SmallVec::with_capacity(sizes.len());
        let mut stride = range.len();
        for size in &sizes {
            elem_strides.push(stride);
            stride = stride.saturating_mul(*size as usize);
        }

        Some(Self {
            sizes,
            clamper,
            range,
            samples: data,
            elem_strides,
            encode,
            max_sample,
            decode_params,
        })
    }

    #[inline]
    fn decode_and_clamp(&self, channel: usize, val: f32) -> f32 {
        let (d0, span) = self.decode_params[channel];
        let (r_min, r_max) = self.range[channel];
        (d0 + val * span / self.max_sample).min(r_max).max(r_min)
    }

    /// Evaluate a type 0 function with the given input.
    pub(crate) fn eval(&self, input: Values) -> Option<Values> {
        let mut out = Values::new();
        self.eval_into(&input, &mut out)?;
        Some(out)
    }

    /// Evaluate a type 0 function into `out` without recursion or heap allocation.
    fn eval_into(&self, input: &[f32], out: &mut Values) -> Option<()> {
        let n_dims = self.sizes.len();
        if input.len() != n_dims {
            warn!("wrong number of arguments for sampled function");

            return None;
        }

        let n_out = self.range.len();
        let out_len = self.decode_params.len();
        if self.sizes.contains(&0) {
            return None;
        }
        if self.clamper.domain.len() != n_dims {
            warn!("the domain of the function didn't match the input arguments");
        }

        let mut base_off = 0usize;
        let mut active: SmallVec<[(usize, f32); 4]> = SmallVec::new();

        for d in 0..n_dims {
            let size = self.sizes[d];
            let mut x = input[d];
            if let Some(&(d_min, d_max)) = self.clamper.domain.get(d) {
                x = x.min(d_max).max(d_min);
                if let Some(&(e_min, e_max)) = self.encode.get(d) {
                    x = interpolate(x, d_min, d_max, e_min, e_max);
                    x = x.max(0.0).min(size as f32 - 1.0);
                }
            }

            let prev = x.floor() as u32;
            let next = x.ceil() as u32;
            if prev >= size || next >= size {
                return None;
            }

            let elem_stride = self.elem_strides[d];
            base_off = base_off.checked_add((prev as usize).checked_mul(elem_stride)?)?;
            if prev != next {
                active.push((elem_stride, x - prev as f32));
            }
        }

        out.clear();
        match active.as_slice() {
            [] => {
                let s = self.samples.get(base_off..base_off.checked_add(n_out)?)?;
                for c in 0..out_len {
                    out.push(self.decode_and_clamp(c, s[c] as f32));
                }
            }
            &[(d0, t0)] => {
                let off1 = base_off.checked_add(d0)?;
                if off1.checked_add(n_out)? > self.samples.len() {
                    return None;
                }
                let s0 = &self.samples[base_off..base_off + out_len];
                let s1 = &self.samples[off1..off1 + out_len];
                for c in 0..out_len {
                    let v0 = s0[c] as f32;
                    let v1 = s1[c] as f32;
                    out.push(self.decode_and_clamp(c, v0 + t0 * (v1 - v0)));
                }
            }
            &[(d0, t0), (d1, t1)] => {
                let off01 = base_off.checked_add(d1)?;
                let off10 = base_off.checked_add(d0)?;
                let off11 = off10.checked_add(d1)?;
                if off11.checked_add(n_out)? > self.samples.len() {
                    return None;
                }
                let s00 = &self.samples[base_off..base_off + out_len];
                let s01 = &self.samples[off01..off01 + out_len];
                let s10 = &self.samples[off10..off10 + out_len];
                let s11 = &self.samples[off11..off11 + out_len];
                for c in 0..out_len {
                    let c00 = s00[c] as f32;
                    let c01 = s01[c] as f32;
                    let c10 = s10[c] as f32;
                    let c11 = s11[c] as f32;
                    let v0 = c00 + t1 * (c01 - c00);
                    let v1 = c10 + t1 * (c11 - c10);
                    out.push(self.decode_and_clamp(c, v0 + t0 * (v1 - v0)));
                }
            }
            axes => {
                let k = axes.len();
                let corners = 1usize.checked_shl(u32::try_from(k).ok()?)?;
                let mut stack_vals: SmallVec<[f32; 32]> = SmallVec::new();
                let mut stack_levels: SmallVec<[usize; 8]> = SmallVec::new();

                for corner in 0..corners {
                    let mut off = base_off;
                    for (b, &(stride, _)) in axes.iter().enumerate() {
                        if (corner >> (k - 1 - b)) & 1 != 0 {
                            off = off.checked_add(stride)?;
                        }
                    }
                    let grid = self.samples.get(off..off.checked_add(n_out)?)?;
                    stack_vals.extend(grid[..out_len].iter().map(|&v| v as f32));
                    stack_levels.push(k);

                    while stack_levels.len() >= 2
                        && stack_levels[stack_levels.len() - 1]
                            == stack_levels[stack_levels.len() - 2]
                    {
                        let level = stack_levels.pop()?;
                        let t = axes[level - 1].1;
                        let split = stack_vals.len() - out_len;
                        let (head, right) = stack_vals.split_at_mut(split);
                        let left_start = split - out_len;
                        let left = &mut head[left_start..];
                        for (a, b) in left.iter_mut().zip(right.iter()) {
                            *a += t * (*b - *a);
                        }
                        stack_vals.truncate(split);
                        *stack_levels.last_mut()? = level - 1;
                    }
                }

                for (c, &val) in stack_vals.iter().enumerate() {
                    out.push(self.decode_and_clamp(c, val));
                }
            }
        }

        if out_len != n_out {
            warn!("the range of the function didn't match the output arguments");
        }

        Some(())
    }
}

type IntVec = SmallVec<[u32; 4]>;

/// PdfCraft patch: the most sample values a sampled function may have, and the most table
/// entries (one per point of its `/Size` grid). The samples were read into 4-byte values and
/// then into a table entry each, so a stream inflating to 256 MiB of 1-bit samples asked for
/// 8 GB; `/Size` sets how many the table expects. The largest real tables are DeviceN tint
/// transforms: 33 × 33 × 33 × 33 points with 3 outputs is 1.2 million entries and 3.6 million
/// values, 17^5 points with 4 outputs 5.7 million values.
const MAX_SAMPLE_VALUES: usize = 1 << 23;
const MAX_SAMPLE_ENTRIES: usize = 1 << 21;
