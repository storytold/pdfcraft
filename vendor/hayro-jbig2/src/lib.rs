/*!
A memory-safe, pure-Rust JBIG2 decoder.

`hayro-jbig2` decodes JBIG2 images as specified in ITU-T T.88 (also known as
ISO/IEC 14492). JBIG2 is a bi-level image compression standard commonly used
in PDF documents for compressing scanned text documents.

The crate is `no_std` compatible but requires an allocator to be available.

# Safety
This crate forbids unsafe code via a crate-level attribute.
*/

#![cfg_attr(not(feature = "std"), no_std)]
#![forbid(unsafe_code)]
#![allow(missing_docs)]

extern crate alloc;

use alloc::vec::Vec;

use crate::arithmetic_decoder::ArithmeticDecoderContext;

/// A reusable context for decoding JBIG2 images.
#[derive(Default)]
pub struct DecoderContext {
    pub(crate) page_state: PageState,
    pub(crate) scratch_buffers: ScratchBuffers,
    pub(crate) page_bitmap: Bitmap,
}

#[derive(Default)]
pub(crate) struct ScratchBuffers {
    pub(crate) contexts: Vec<ArithmeticDecoderContext>,
}

/// A decoder for JBIG2 images.
pub trait Decoder {
    /// Push a single pixel to the output.
    fn push_pixel(&mut self, black: bool);
    /// Push multiple chunks of 8 pixels of the same color.
    ///
    /// The `chunk_count` parameter indicates how many 8-pixel chunks to push.
    /// For example, if this method is called with `white = true` and
    /// `chunk_count = 10`, 80 white pixels are pushed (10 × 8 = 80).
    ///
    /// You can assume that this method is only called if the number of already
    /// pushed pixels is a multiple of 8 (i.e. byte-aligned).
    fn push_pixel_chunk(&mut self, black: bool, chunk_count: u32);
    /// Called when a row has been completed.
    fn next_line(&mut self);
}

#[cfg(feature = "image")]
pub mod integration;

mod arithmetic_decoder;
mod bitmap;
mod decode;
mod error;
mod file;
mod gray_scale;
mod huffman_table;
mod integer_decoder;
mod lazy;
mod page_info;
mod reader;
mod segment;
mod simd;
mod symbol_id_decoder;

use error::bail;
pub use error::{
    DecodeError, FormatError, HuffmanError, OverflowError, ParseError, RegionError, Result,
    SegmentError, SymbolError, TemplateError,
};

use crate::file::{SegmentCount, parse_segments_sequential};
use bitmap::Bitmap;
use decode::CombinationOperator;
use decode::generic;
use decode::generic_refinement;
use decode::halftone;
use decode::pattern;
use decode::pattern::PatternDictionary;
use decode::symbol;
use decode::symbol::SymbolDictionary;
use decode::text;
use file::parse_file;
use huffman_table::{HuffmanTable, StandardHuffmanTables};
use page_info::{PageInformation, parse_page_information};
use reader::Reader;
use segment::SegmentType;

/// A JBIG2 image.
pub struct Image<'a> {
    /// The parsed segments.
    segments: Vec<segment::Segment<'a>>,
    /// The width of the image in pixels.
    width: u32,
    /// The height of the image in pixels.
    height: u32,
    /// The height determined from `EndOfStripe` segments, if applicable.
    height_from_stripes: Option<u32>,
}

impl<'a> Image<'a> {
    /// Parse a JBIG2 file from the given data.
    ///
    /// The file is expected to use the sequential or random-access organization,
    /// as defined in Annex D.1 and D.2.
    pub fn new(data: &'a [u8]) -> Result<Self> {
        let file = parse_file(data)?;
        Self::from_segments(file.segments)
    }

    /// Parse an embedded JBIG2 image with optional global segments.
    ///
    /// The file is expected to use the embedded organization defined in
    /// Annex D.3.
    pub fn new_embedded(data: &'a [u8], globals: Option<&'a [u8]>) -> Result<Self> {
        let mut segments = Vec::new();
        let mut count = SegmentCount::default();
        if let Some(globals_data) = globals {
            let mut reader = Reader::new(globals_data);
            parse_segments_sequential(&mut reader, &mut segments, &mut count)?;
        };

        let mut reader = Reader::new(data);
        parse_segments_sequential(&mut reader, &mut segments, &mut count)?;

        segments.sort_by_key(|seg| seg.header.segment_number);

        Self::from_segments(segments)
    }

    fn from_segments(segments: Vec<segment::Segment<'a>>) -> Result<Self> {
        // Pre-scan for stripe height from EndOfStripe segments.
        let height_from_stripes = segments
            .iter()
            .filter(|seg| seg.header.segment_type == SegmentType::EndOfStripe)
            .filter_map(|seg| u32::from_be_bytes(seg.data.try_into().ok()?).checked_add(1))
            .max();

        // Find and parse page information to extract dimensions.
        let page_info_seg = segments
            .iter()
            .find(|s| s.header.segment_type == SegmentType::PageInformation)
            .ok_or(FormatError::MissingPageInfo)?;

        let mut reader = Reader::new(page_info_seg.data);
        let page_info = parse_page_information(&mut reader)?;

        // "A page's bitmap height may be declared in its page information segment
        // to be unknown (by specifying a height of 0xFFFFFFFF). In this case, the
        // page must be striped." (7.4.8.2)
        let height = if page_info.height == 0xFFFF_FFFF {
            height_from_stripes.ok_or(FormatError::UnknownPageHeight)?
        } else {
            page_info.height
        };

        if page_info.width == 0 || height == 0 {
            bail!(FormatError::EmptyPage);
        }
        bitmap::check_page_size(page_info.width, height)?;

        Ok(Self {
            segments,
            width: page_info.width,
            height,
            height_from_stripes,
        })
    }

    /// The width of the image in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// The height of the image in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Decode the image data through the given [`Decoder`].
    pub fn decode<D: Decoder>(&self, decoder: &mut D) -> Result<()> {
        let mut ctx = DecoderContext::default();

        self.decode_with(decoder, &mut ctx)
    }

    /// Decode the image data through the given [`Decoder`] and [`DecoderContext`].
    ///
    /// This is useful in case you want to convert multiple JBIG2 images,
    /// as it allows `hayro-jbig2` to reuse allocations during decoding.
    pub fn decode_with<D: Decoder>(&self, decoder: &mut D, ctx: &mut DecoderContext) -> Result<()> {
        decode_segments(&self.segments, self.height_from_stripes, ctx)?;
        emit_bitmap(&ctx.page_bitmap, decoder);

        Ok(())
    }
}

fn emit_bitmap<D: Decoder>(bitmap: &Bitmap, decoder: &mut D) {
    let width = bitmap.width;
    let bytes_per_row = width.div_ceil(8) as usize;

    for row in bitmap.data.chunks_exact(bitmap.stride as usize) {
        let mut x = 0_u32;
        let mut chunk_byte: Option<u8> = None;
        let mut chunk_count = 0_u32;

        let bytes = row.iter().flat_map(|w| w.to_be_bytes()).take(bytes_per_row);

        for byte in bytes {
            let remaining = width - x;

            if remaining >= 8 && (byte == 0x00 || byte == 0xFF) {
                // Continue the previous chunk.
                if chunk_byte == Some(byte) {
                    chunk_count += 1;
                    x += 8;
                    continue;
                }

                // Flush previous chunk if any, then start new one.
                if let Some(b) = chunk_byte {
                    decoder.push_pixel_chunk(b == 0xFF, chunk_count);
                }

                chunk_byte = Some(byte);
                chunk_count = 1;
                x += 8;

                continue;
            }

            // Can't continue/start chunk, flush any existing chunk first.
            if let Some(b) = chunk_byte.take() {
                decoder.push_pixel_chunk(b == 0xFF, chunk_count);
                chunk_count = 0;
            }

            // Emit individual pixels.
            let count = remaining.min(8);
            for i in 0..count {
                decoder.push_pixel((byte >> (7 - i)) & 1 != 0);
            }
            x += count;
        }

        // Flush any remaining chunk at end of row.
        if let Some(b) = chunk_byte {
            decoder.push_pixel_chunk(b == 0xFF, chunk_count);
        }

        decoder.next_line();
    }
}

/// PdfCraft patch: count `work` more decoding work, refusing past [`bitmap::MAX_WORK`] before what
/// asks for it is decoded.
fn charge_work(left: &mut u64, work: u64) -> Result<()> {
    *left = left.checked_sub(work).ok_or(OverflowError::DecodingWork)?;
    Ok(())
}

/// PdfCraft patch: the work of a region decoded pixel by pixel.
fn region_work(region: &decode::RegionSegmentInfo) -> u64 {
    bitmap::decode_work(region.width, region.height)
}

fn decode_segments(
    segments: &[segment::Segment<'_>],
    height_from_stripes: Option<u32>,
    decoder_ctx: &mut DecoderContext,
) -> Result<()> {
    // Find and parse page information segment first.
    if let Some(page_info) = segments
        .iter()
        .find(|s| s.header.segment_type == SegmentType::PageInformation)
    {
        let mut reader = Reader::new(page_info.data);
        init_page(
            &mut reader,
            height_from_stripes,
            &mut decoder_ctx.page_state,
            &mut decoder_ctx.page_bitmap,
        )?;
    } else {
        bail!(FormatError::MissingPageInfo);
    }

    let page_bitmap = &mut decoder_ctx.page_bitmap;
    let page_state = &mut decoder_ctx.page_state;
    let scratch_buffers = &mut decoder_ctx.scratch_buffers;
    let mut work_left = bitmap::MAX_WORK;

    // Process all segments.
    for seg in segments {
        let mut reader = Reader::new(seg.data);

        match seg.header.segment_type {
            SegmentType::PageInformation => {
                // Already processed above, skip.
            }
            SegmentType::ImmediateGenericRegion | SegmentType::ImmediateLosslessGenericRegion => {
                let had_unknown_length = seg.header.data_length.is_none();
                let header = generic::parse(&mut reader, had_unknown_length)?;
                charge_work(&mut work_left, region_work(&header.region_info))?;

                if page_state.can_decode_directly(page_bitmap, &header.region_info, false) {
                    generic::decode_into(&header, page_bitmap, scratch_buffers)?;
                } else {
                    let region = generic::decode(&header, scratch_buffers)?;
                    page_bitmap.combine(
                        &region.bitmap,
                        region.bitmap.x_location as i32,
                        region.bitmap.y_location as i32,
                        region.combination_operator,
                    );
                }
                page_state.page_pristine = false;
            }
            SegmentType::IntermediateGenericRegion => {
                // Intermediate segments cannot have unknown length.
                let header = generic::parse(&mut reader, false)?;
                charge_work(&mut work_left, region_work(&header.region_info))?;
                let region = generic::decode(&header, scratch_buffers)?;
                page_state.store_region(seg.header.segment_number, region.bitmap)?;
            }
            SegmentType::PatternDictionary => {
                let header = pattern::parse(&mut reader)?;
                charge_work(&mut work_left, header.work())?;
                let dictionary = pattern::decode(&header, scratch_buffers, page_state.retained_room())?;
                page_state.store_pattern_dictionary(seg.header.segment_number, dictionary)?;
            }
            SegmentType::SymbolDictionary => {
                // "1) Concatenate all the input symbol dictionaries to form SDINSYMS."
                // (6.5.5, step 1)
                // Collect references to avoid cloning; symbols are only cloned if re-exported.
                let input_symbols = page_state.referred_symbols(&seg.header.referred_to_segments)?;
                charge_work(&mut work_left, input_symbols.len() as u64)?;

                // Collect Huffman tables from referred table segments.
                let referred_tables: Vec<HuffmanTable> = seg
                    .header
                    .referred_to_segments
                    .iter()
                    .filter_map(|&num| page_state.get_huffman_table(num))
                    .cloned()
                    .collect();

                // Get retained contexts from the last referred symbol dictionary (7.4.2.2 step 3).
                let retained_contexts = seg
                    .header
                    .referred_to_segments
                    .last()
                    .and_then(|&num| page_state.get_symbol_dictionary(num))
                    .and_then(|dict| dict.retained_contexts.as_ref());

                let header = symbol::parse(&mut reader)?;
                let dictionary = symbol::decode(
                    &header,
                    &input_symbols,
                    &referred_tables,
                    &page_state.standard_tables,
                    retained_contexts,
                    page_state.retained_room(),
                    &mut work_left,
                )?;
                page_state.store_symbol_dictionary(seg.header.segment_number, dictionary)?;
            }
            SegmentType::ImmediateTextRegion | SegmentType::ImmediateLosslessTextRegion => {
                // Collect symbols from referred symbol dictionaries (SBSYMS).
                let symbols = page_state.referred_symbols(&seg.header.referred_to_segments)?;
                charge_work(&mut work_left, symbols.len() as u64)?;

                // Collect Huffman tables from referred table segments.
                // "These user-supplied Huffman decoding tables may be supplied either
                // as a Tables segment..." (7.4.3.1.6)
                let referred_tables: Vec<HuffmanTable> = seg
                    .header
                    .referred_to_segments
                    .iter()
                    .filter_map(|&num| page_state.get_huffman_table(num))
                    .cloned()
                    .collect();

                let header = text::parse(&mut reader, symbols.len() as u32)?;
                charge_work(&mut work_left, bitmap::copy_work(header.region_info.width, header.region_info.height))?;
                if header.flags.use_huffman {
                    // Its symbol ID table has a line for each symbol.
                    charge_work(&mut work_left, (symbols.len() as u64).saturating_mul(bitmap::HUFFMAN_LINE_WORK))?;
                }

                if page_state.can_decode_directly(
                    page_bitmap,
                    &header.region_info,
                    header.flags.default_pixel,
                ) {
                    text::decode_into(
                        &header,
                        &symbols,
                        &referred_tables,
                        &page_state.standard_tables,
                        page_bitmap,
                        scratch_buffers,
                        &mut work_left,
                    )?;
                } else {
                    let region = text::decode(
                        &header,
                        &symbols,
                        &referred_tables,
                        &page_state.standard_tables,
                        scratch_buffers,
                        &mut work_left,
                    )?;
                    page_bitmap.combine(
                        &region.bitmap,
                        region.bitmap.x_location as i32,
                        region.bitmap.y_location as i32,
                        region.combination_operator,
                    );
                }
                page_state.page_pristine = false;
            }
            SegmentType::IntermediateTextRegion => {
                // Collect symbols from referred symbol dictionaries (SBSYMS).
                let symbols = page_state.referred_symbols(&seg.header.referred_to_segments)?;
                charge_work(&mut work_left, symbols.len() as u64)?;

                // Collect Huffman tables from referred table segments.
                let referred_tables: Vec<HuffmanTable> = seg
                    .header
                    .referred_to_segments
                    .iter()
                    .filter_map(|&num| page_state.get_huffman_table(num))
                    .cloned()
                    .collect();

                let header = text::parse(&mut reader, symbols.len() as u32)?;
                charge_work(&mut work_left, bitmap::copy_work(header.region_info.width, header.region_info.height))?;
                if header.flags.use_huffman {
                    // Its symbol ID table has a line for each symbol.
                    charge_work(&mut work_left, (symbols.len() as u64).saturating_mul(bitmap::HUFFMAN_LINE_WORK))?;
                }
                let region = text::decode(
                    &header,
                    &symbols,
                    &referred_tables,
                    &page_state.standard_tables,
                    scratch_buffers,
                    &mut work_left,
                )?;
                page_state.store_region(seg.header.segment_number, region.bitmap)?;
            }
            SegmentType::ImmediateHalftoneRegion | SegmentType::ImmediateLosslessHalftoneRegion => {
                let pattern_dict = seg
                    .header
                    .referred_to_segments
                    .first()
                    .and_then(|&num| page_state.get_pattern_dictionary(num))
                    .ok_or(SegmentError::MissingPatternDictionary)?;

                let header = halftone::parse(&mut reader)?;
                charge_work(&mut work_left, halftone::work(&header, pattern_dict))?;

                if page_state.can_decode_directly(
                    page_bitmap,
                    &header.region_info,
                    header.flags.initial_pixel_color,
                ) {
                    halftone::decode_into(&header, pattern_dict, page_bitmap, scratch_buffers)?;
                } else {
                    let region = halftone::decode(&header, pattern_dict, scratch_buffers)?;
                    page_bitmap.combine(
                        &region.bitmap,
                        region.bitmap.x_location as i32,
                        region.bitmap.y_location as i32,
                        region.combination_operator,
                    );
                }
                page_state.page_pristine = false;
            }
            SegmentType::IntermediateHalftoneRegion => {
                let pattern_dict = seg
                    .header
                    .referred_to_segments
                    .first()
                    .and_then(|&num| page_state.get_pattern_dictionary(num))
                    .ok_or(SegmentError::MissingPatternDictionary)?;

                let header = halftone::parse(&mut reader)?;
                charge_work(&mut work_left, halftone::work(&header, pattern_dict))?;
                let region = halftone::decode(&header, pattern_dict, scratch_buffers)?;
                page_state.store_region(seg.header.segment_number, region.bitmap)?;
            }
            SegmentType::IntermediateGenericRefinementRegion => {
                // Same logic as immediate refinement, but store result instead of combining.
                let reference = seg
                    .header
                    .referred_to_segments
                    .first()
                    .and_then(|&num| page_state.get_referred_segment(num))
                    .unwrap_or(page_bitmap);

                let header = generic_refinement::parse(&mut reader)?;
                charge_work(&mut work_left, region_work(&header.region_info))?;
                let region = generic_refinement::decode(&header, reference, scratch_buffers)?;
                page_state.store_region(seg.header.segment_number, region.bitmap)?;
            }
            SegmentType::ImmediateGenericRefinementRegion
            | SegmentType::ImmediateLosslessGenericRefinementRegion => {
                // "3) Determine the buffer associated with the region segment that
                // this segment refers to." (7.4.7.5)
                //
                // "2) If there are no referred-to segments, then use the page
                // bitmap as the reference buffer." (7.4.7.5)
                let referred_segment = seg
                    .header
                    .referred_to_segments
                    .first()
                    .and_then(|&num| page_state.get_referred_segment(num));

                let header = generic_refinement::parse(&mut reader)?;
                charge_work(&mut work_left, region_work(&header.region_info))?;

                if let Some(referred_segment) = referred_segment
                    && page_state.can_decode_directly(page_bitmap, &header.region_info, false)
                {
                    generic_refinement::decode_into(
                        &header,
                        referred_segment,
                        page_bitmap,
                        scratch_buffers,
                    )?;
                } else {
                    let reference = referred_segment.unwrap_or(page_bitmap);
                    let region = generic_refinement::decode(&header, reference, scratch_buffers)?;
                    page_bitmap.combine(
                        &region.bitmap,
                        region.bitmap.x_location as i32,
                        region.bitmap.y_location as i32,
                        region.combination_operator,
                    );
                }
                page_state.page_pristine = false;
            }
            SegmentType::Tables => {
                // "Tables – see 7.4.13." (type 53)
                // "This segment contains data which defines one or more user-supplied
                // Huffman coding tables." (7.4.13)
                let table = HuffmanTable::read_custom(&mut reader)?;
                page_state.store_huffman_table(seg.header.segment_number, table)?;
            }
            SegmentType::EndOfPage | SegmentType::EndOfFile => {
                break;
            }
            // Other segment types not yet implemented.
            _ => {}
        }
    }

    Ok(())
}

/// PdfCraft patch: the words a stored item costs in its list, at least, apart from what it holds.
const ENTRY_WORDS: u64 = 32;

/// PdfCraft patch: the most words a page's segments may hold for later ones, 64 MiB: two square
/// bitmaps of 16,384 a side (`bitmap::MAX_PIXELS`). Each stored item is bounded on its own, but without a
/// bound on all of them a stream of a few dozen bytes a segment held 33 MB each (a 16,384-sample
/// square region of typical rows), and a few kilobytes could exhaust memory. A segment being
/// decoded takes more for a while, within its own bounds: the page bitmap isn't counted, nor is
/// what a region or a grid uses until it is stored or freed.
const MAX_RETAINED_WORDS: u64 =
    2 * (bitmap::MAX_PIXELS / bitmap::WORD_BITS as u64 + bitmap::BITMAP_WORDS as u64 + ENTRY_WORDS);

/// PdfCraft patch: the most symbols one segment may refer to, 2^20, far beyond the 65,535 a
/// dictionary may export. References may repeat, so each names the same symbols again: without
/// this, 65,535 references to one dictionary in a 78 KB stream collected 17 GB of pointers.
const MAX_REFERRED_SYMBOLS: u64 = 1 << 20;

/// Page-level decoding state for a JBIG2 page.
#[derive(Default)]
pub(crate) struct PageState {
    /// The parsed page information.
    pub(crate) page_info: PageInformation,
    /// Whether the page bitmap is still in its initial state (not yet painted to).
    pub(crate) page_pristine: bool,
    /// Decoded intermediate regions, stored as (`segment_number`, region) pairs.
    pub(crate) referred_segments: Vec<(u32, Bitmap)>,
    /// Decoded pattern dictionaries, stored as (`segment_number`, dictionary) pairs.
    pub(crate) pattern_dictionaries: Vec<(u32, PatternDictionary)>,
    /// Decoded symbol dictionaries, stored as (`segment_number`, dictionary) pairs.
    pub(crate) symbol_dictionaries: Vec<(u32, SymbolDictionary)>,
    /// Decoded Huffman tables from table segments, stored as (`segment_number`, table) pairs.
    /// "Tables – see 7.4.13." (type 53)
    pub(crate) huffman_tables: Vec<(u32, HuffmanTable)>,
    /// PdfCraft patch: the words the regions, dictionaries and tables above hold, at most
    /// [`MAX_RETAINED_WORDS`].
    pub(crate) retained_words: u64,
    /// Standard Huffman tables (`TABLE_A` through `TABLE_O`).
    pub(crate) standard_tables: StandardHuffmanTables,
}

impl PageState {
    fn reset(&mut self, page_info: PageInformation) {
        self.page_info = page_info;
        self.page_pristine = true;
        self.referred_segments.clear();
        self.pattern_dictionaries.clear();
        self.symbol_dictionaries.clear();
        self.huffman_tables.clear();
        self.retained_words = 0;
        // Standard tables are lazily built and reused across images.
    }

    /// Check if an immediate region can be decoded directly into the page bitmap.
    fn can_decode_directly(
        &self,
        page_bitmap: &Bitmap,
        region_info: &decode::RegionSegmentInfo,
        region_default_pixel: bool,
    ) -> bool {
        if !self.page_pristine {
            return false;
        }

        let covers_page = region_info.x_location == 0
            && region_info.y_location == 0
            && region_info.width == page_bitmap.width
            && region_info.height == page_bitmap.height;

        if !covers_page {
            return false;
        }

        let page_default_is_zero = self.page_info.flags.default_pixel == 0;

        if region_default_pixel == page_default_is_zero {
            return false;
        }

        let op = region_info.combination_operator;
        match op {
            CombinationOperator::Replace => true,
            CombinationOperator::Or | CombinationOperator::Xor => page_default_is_zero,
            CombinationOperator::And | CombinationOperator::Xnor => !page_default_is_zero,
        }
    }

    /// PdfCraft patch: the words the page may still hold for later segments.
    fn retained_room(&self) -> u64 {
        MAX_RETAINED_WORDS.saturating_sub(self.retained_words)
    }

    /// PdfCraft patch: count a stored item holding `words` for later segments, refusing past
    /// [`MAX_RETAINED_WORDS`].
    fn retain(&mut self, words: usize) -> Result<()> {
        self.retained_words = self.retained_words.saturating_add(words as u64).saturating_add(ENTRY_WORDS);
        if self.retained_words > MAX_RETAINED_WORDS {
            bail!(OverflowError::RetainedBitmaps);
        }
        Ok(())
    }

    /// Store a decoded region for later reference.
    fn store_region(&mut self, segment_number: u32, region: Bitmap) -> Result<()> {
        self.retain(region.words())?;
        self.referred_segments.push((segment_number, region));
        Ok(())
    }

    /// Look up a referred segment by number.
    fn get_referred_segment(&self, segment_number: u32) -> Option<&Bitmap> {
        self.referred_segments
            .binary_search_by_key(&segment_number, |(num, _)| *num)
            .ok()
            .map(|idx| &self.referred_segments[idx].1)
    }

    /// Store a decoded pattern dictionary for later reference.
    fn store_pattern_dictionary(&mut self, segment_number: u32, dictionary: PatternDictionary) -> Result<()> {
        self.retain(dictionary.words())?;
        self.pattern_dictionaries.push((segment_number, dictionary));
        Ok(())
    }

    /// Look up a pattern dictionary by segment number.
    fn get_pattern_dictionary(&self, segment_number: u32) -> Option<&PatternDictionary> {
        self.pattern_dictionaries
            .binary_search_by_key(&segment_number, |(num, _)| *num)
            .ok()
            .map(|idx| &self.pattern_dictionaries[idx].1)
    }

    /// Store a decoded symbol dictionary for later reference.
    fn store_symbol_dictionary(&mut self, segment_number: u32, dictionary: SymbolDictionary) -> Result<()> {
        self.retain(dictionary.words())?;
        self.symbol_dictionaries.push((segment_number, dictionary));
        Ok(())
    }

    /// PdfCraft patch: the symbols of the dictionaries in `segments`, in order, refusing past
    /// [`MAX_REFERRED_SYMBOLS`] before anything is collected.
    fn referred_symbols(&self, segments: &[u32]) -> Result<Vec<&Bitmap>> {
        let dictionaries = || segments.iter().filter_map(|&num| self.get_symbol_dictionary(num));
        let count = dictionaries()
            .map(|dictionary| dictionary.exported_symbols.len() as u64)
            .fold(0, u64::saturating_add);
        if count > MAX_REFERRED_SYMBOLS {
            bail!(OverflowError::SymbolCount);
        }
        Ok(dictionaries().flat_map(|dictionary| dictionary.exported_symbols.iter()).collect())
    }

    /// Look up a symbol dictionary by segment number.
    fn get_symbol_dictionary(&self, segment_number: u32) -> Option<&SymbolDictionary> {
        self.symbol_dictionaries
            .binary_search_by_key(&segment_number, |(num, _)| *num)
            .ok()
            .map(|idx| &self.symbol_dictionaries[idx].1)
    }

    /// Store a decoded Huffman table for later reference.
    fn store_huffman_table(&mut self, segment_number: u32, table: HuffmanTable) -> Result<()> {
        self.retain(table.words())?;
        self.huffman_tables.push((segment_number, table));
        Ok(())
    }

    /// Look up a Huffman table by segment number.
    fn get_huffman_table(&self, segment_number: u32) -> Option<&HuffmanTable> {
        self.huffman_tables
            .binary_search_by_key(&segment_number, |(num, _)| *num)
            .ok()
            .map(|idx| &self.huffman_tables[idx].1)
    }
}

/// Parse page information and initialize the page bitmap.
fn init_page(
    reader: &mut Reader<'_>,
    height_from_stripes: Option<u32>,
    page: &mut PageState,
    bitmap: &mut Bitmap,
) -> Result<()> {
    let page_info = parse_page_information(reader)?;

    // "A page's bitmap height may be declared in its page information segment
    // to be unknown (by specifying a height of 0xFFFFFFFF). In this case, the
    // page must be striped." (7.4.8.2)
    let height = if page_info.height == 0xFFFF_FFFF {
        height_from_stripes.ok_or(FormatError::UnknownPageHeight)?
    } else {
        page_info.height
    };

    // "Bit 2: Page default pixel value. This bit contains the initial value
    // for every pixel in the page, before any region segments are decoded
    // or drawn." (7.4.8.5)
    bitmap.reinitialize(page_info.width, height, page_info.flags.default_pixel != 0)?;

    page.reset(page_info);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A page past the bounds is refused when the stream is opened, since callers size their
    /// output from it before decoding starts. 16,384 squared is exactly the bound on pixels.
    #[test]
    fn a_page_past_the_bounds_is_refused_when_opened() {
        let stream = |width: u32, height: u32| {
            let mut stream = vec![0, 0, 0, 0, 48, 0, 1, 0, 0, 0, 19];
            stream.extend_from_slice(&width.to_be_bytes());
            stream.extend_from_slice(&height.to_be_bytes());
            stream.extend_from_slice(&[0; 11]); // resolution, flags, striping
            stream
        };
        assert!(Image::new_embedded(&stream(16384, 16384), None).is_ok());
        assert!(Image::new_embedded(&stream(16385, 16384), None).is_err());
        assert!(Image::new_embedded(&stream(65536, 1), None).is_err(), "too wide, though few pixels");
        assert!(Image::new_embedded(&stream(1 << 20, 1 << 20), None).is_err());
    }

    /// A stream and its globals are one stream of segments, so they have one count.
    #[test]
    fn the_segments_of_a_stream_and_its_globals_count_together() {
        let segment: &[u8] = &[0, 0, 0, 0, 62, 0, 1, 0, 0, 0, 0]; // an empty extension
        let half = segment.repeat(1 << 19);
        let more = segment.repeat((1 << 19) + 1);
        let refused = |data: &[u8]| {
            let error = Image::new_embedded(data, Some(&half)).err().map(|e| e.to_string());
            error.as_deref() == Some("too many segments or references in a stream")
        };
        assert!(!refused(&half), "2^20 segments are allowed");
        assert!(refused(&more), "one more isn't");
    }

    /// A dictionary being decoded has what the page may still hold, not all of it: refused only
    /// once it is stored, its copies would already have been made.
    #[test]
    fn the_room_left_shrinks_as_segments_are_stored() {
        let mut page = PageState::default();
        assert_eq!(page.retained_room(), MAX_RETAINED_WORDS);
        page.retain(1000).unwrap();
        assert_eq!(page.retained_room(), MAX_RETAINED_WORDS - 1000 - ENTRY_WORDS);
        page.retained_words = MAX_RETAINED_WORDS;
        assert_eq!(page.retained_room(), 0);
    }
}
