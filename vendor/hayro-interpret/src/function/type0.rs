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
    /// PdfCraft patch (31): how many grid points one step along each input moves.
    strides: SmallVec<[usize; 4]>,
    clamper: Clamper,
    range: TupleVec,
    bits_per_sample: u8,
    encode: TupleVec,
    decode: TupleVec,
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
        // then no point is on it (`entry` checks each coordinate against its size first).
        let mut strides = SmallVec::with_capacity(sizes.len());
        let mut stride = 1usize;
        for size in &sizes {
            strides.push(stride);
            stride = stride.saturating_mul(*size as usize);
        }

        Some(Self {
            sizes,
            clamper,
            range,
            bits_per_sample,
            samples: data,
            strides,
            encode,
            decode,
        })
    }

    /// Evaluate a type 0 function with the given input.
    pub(crate) fn eval(&self, mut input: Values) -> Option<Values> {
        if input.len() != self.sizes.len() {
            warn!("wrong number of arguments for sampled function");

            return None;
        }

        self.clamper.clamp_input(&mut input);

        let mut key = input;

        for (((x, domain), encode), size) in key
            .iter_mut()
            .zip(self.clamper.domain.iter())
            .zip(self.encode.iter())
            .zip(self.sizes.iter())
        {
            *x = interpolate(*x, domain.0, domain.1, encode.0, encode.1);
            *x = x.max(0.0).min(*size as f32 - 1.0);
        }

        let in_prev = key.iter().map(|v| v.floor() as u32).collect::<IntVec>();
        let in_next = key.iter().map(|v| v.ceil() as u32).collect::<IntVec>();
        let mut coord: IntVec = in_prev.clone();

        let interpolated = self.interpolate(&key, &in_prev, &in_next, &mut coord, 0)?;

        // PdfCraft patch (31): `2_u32.pow(32)` overflowed (a panic with overflow checks on).
        let max_sample = ((1u64 << self.bits_per_sample) - 1) as f32;
        let mut out = interpolated
            .iter()
            .zip(self.decode.iter())
            .map(|(x, decode)| interpolate(*x, 0.0, max_sample, decode.0, decode.1))
            .collect::<SmallVec<_>>();

        self.clamper.clamp_output(&mut out);

        Some(out)
    }
}

type FloatVec = SmallVec<[f32; 4]>;
type IntVec = SmallVec<[u32; 4]>;

// See <https://github.com/apache/pdfbox/blob/bb778d4784f354c36ce032e91a0cee2169a4c598/pdfbox/src/main/java/org/apache/pdfbox/pdmodel/common/function/PDFunctionType0.java#L252>
impl Type0 {
    /// PdfCraft patch (31): the value at `input` between the grid points `in_prev` and
    /// `in_next`, interpolating the inputs from `step` on (the earlier ones are placed in
    /// `coord`). The same multilinear interpolation, in the same order, as the hash-map
    /// version, so the results are bit for bit the same; one level of recursion per input,
    /// as before.
    fn interpolate(
        &self,
        input: &[f32],
        in_prev: &[u32],
        in_next: &[u32],
        coord: &mut IntVec,
        step: usize,
    ) -> Option<FloatVec> {
        let (Some(x), Some(prev), Some(next)) =
            (input.get(step), in_prev.get(step), in_next.get(step))
        else {
            // Every input placed: the grid point itself.
            return Some(self.entry(coord)?.iter().map(|n| *n as f32).collect());
        };
        let (x, prev, next) = (*x, *prev, *next);

        *coord.get_mut(step)? = prev;
        let val1 = self.interpolate(input, in_prev, in_next, coord, step + 1)?;
        if prev == next {
            return Some(val1);
        }
        *coord.get_mut(step)? = next;
        let val2 = self.interpolate(input, in_prev, in_next, coord, step + 1)?;

        Some(
            val1.iter()
                .zip(val2.iter())
                .map(|(a, b)| interpolate(x, prev as f32, next as f32, *a, *b))
                .collect(),
        )
    }

    /// PdfCraft patch (31): the samples of grid point `coord`, or `None` where it is off the
    /// grid or the stream ended before it (where the hash map had no entry).
    fn entry(&self, coord: &[u32]) -> Option<&[u32]> {
        let mut index = 0usize;
        for ((c, size), stride) in coord.iter().zip(&self.sizes).zip(&self.strides) {
            if c >= size {
                return None;
            }
            index = index.checked_add((*c as usize).checked_mul(*stride)?)?;
        }
        let n = self.range.len();
        let start = index.checked_mul(n)?;
        self.samples.get(start..start.checked_add(n)?)
    }
}

/// PdfCraft patch: the most sample values a sampled function may have, and the most table
/// entries (one per point of its `/Size` grid). The samples were read into 4-byte values and
/// then into a table entry each, so a stream inflating to 256 MiB of 1-bit samples asked for
/// 8 GB; `/Size` sets how many the table expects. The largest real tables are DeviceN tint
/// transforms: 33 × 33 × 33 × 33 points with 3 outputs is 1.2 million entries and 3.6 million
/// values, 17^5 points with 4 outputs 5.7 million values.
const MAX_SAMPLE_VALUES: usize = 1 << 23;
const MAX_SAMPLE_ENTRIES: usize = 1 << 21;
