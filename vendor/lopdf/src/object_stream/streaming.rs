//! PdfCraft patch: an object stream decoded as it is read.
//!
//! `ObjectStream::new_members_within` used to decode a whole stream into one buffer before
//! reading any member, so every load of an object stream held all of its decoded bytes at once
//! (up to the 256 MiB decompression limit for one stream), and a lookup that wants one member
//! decoded and kept the members around it too. Here a stream whose only filter is a plain
//! `/FlateDecode` is inflated in one pass: the index is read, then the members `keep` accepts are
//! collected into batches of about [`BATCH_BYTES`] and parsed, and the bytes of the others are
//! decoded and dropped. What a load holds is the index (then the list of members to read), one
//! batch and the largest member read, instead of the whole stream. A member's bytes run to the
//! next offset (the last member's to the end of the stream), as the parser has always seen them,
//! so padding after a member that is read is held with it.
//!
//! Given the same decoded bytes, the result is the one the whole-stream path gives, error for
//! error:
//! - every decoded byte, kept or not, counts towards the decompression limit, and a stream over
//!   it is refused whatever the members before the excess;
//! - members are bounded by the next distinct offset, the first entry of an offset wins, and the
//!   shared node budget builds and skips the same members (see `parse_members`); the nodes of a
//!   stream that is refused part-way are given back, as if none had been parsed;
//! - with a limit, an error found in the dictionary or the index is reported only once the rest
//!   of the stream is known to be within the limit, which the whole-stream path checks first.
//!
//! The decoded bytes are the same for intact data. For corrupt data they are everything decoded
//! before the corruption, a prefix that depends only on the stream (see [`Inflate`]); the
//! whole-stream path keeps a shorter prefix that depends on how flate2's reads fall, and loses the
//! last of the data when only the Adler-32 checksum is wrong.
//!
//! Streams with other filters or a predictor keep the whole-stream path. When the index lists
//! members out of offset order (which no writer does, but a damaged file can), the rest of the
//! stream is decoded after the index, and the members are read from the whole of it.

use super::{Member, NodeBudget, ObjectStream, check_member_count, first_offset, parse_members};
use crate::{DecompressError, Error, Object, ObjectId, Result, Stream};
use flate2::{Decompress, FlushDecompress, Status};
use log::warn;
use std::collections::BTreeMap;
use std::str::FromStr;

/// Decoded member bytes collected before they are parsed together (in parallel when the budget
/// allows). One member longer than this is parsed on its own.
const BATCH_BYTES: usize = 1 << 20;

/// Deflate's largest expansion: a 258-byte match coded in two one-bit codes.
const MAX_DEFLATE_RATIO: usize = 1032;

/// Output the decoder may hold back from what its consumed input implies (its 32 KiB window and
/// bit buffer).
const DECODER_SLACK: usize = 64 << 10;

/// Whether `stream` is decoded by [`members_within`]: its one filter is `/FlateDecode` and
/// it has no predictor (`Stream::decompress_predictor` would transform nothing).
pub(super) fn is_streamable(stream: &Stream) -> bool {
    let Ok(filters) = stream.filters() else {
        return false;
    };
    if filters != [b"FlateDecode".as_slice()] {
        return false;
    }
    let predictor = stream
        .dict
        .get(b"DecodeParms")
        .and_then(Object::as_dict)
        .ok()
        .and_then(|params| params.get(b"Predictor").and_then(Object::as_i64).ok())
        .unwrap_or(1);
    predictor != 2 && !(10..=15).contains(&predictor)
}

/// The members of a [streamable](is_streamable) stream that `keep` accepts, built from `budget`.
pub(super) fn members_within(
    stream: &Stream, max_decompressed_size: Option<usize>, budget: &NodeBudget, keep: &impl Fn(u32) -> bool,
) -> Result<BTreeMap<ObjectId, Object>> {
    let mut objects = BTreeMap::new();
    let mut inflate = Inflate::new(&stream.content, max_decompressed_size);
    let mut index = Vec::new();
    // Decoding nothing gives no members, whatever the dictionary says.
    if inflate.append(Some(1), &mut index)? == 0 {
        return Ok(objects);
    }
    let plan = match plan(stream, &mut inflate, &mut index, keep) {
        Ok(Some(plan)) => plan,
        Ok(None) => {
            // Offsets out of order: the members are parsed from the whole stream, decoded on from
            // the index already read.
            inflate.append(None, &mut index)?;
            return ObjectStream::members_of(stream, &index, budget, keep);
        }
        Err(err) => {
            inflate.finish()?;
            return Err(err);
        }
    };

    drop(index);
    let mut used = 0;
    let read = read_members(&mut inflate, &plan, budget, &mut objects, &mut used).and_then(|skipped| {
        inflate.finish()?;
        Ok(skipped)
    });
    match read {
        Ok(skipped) => {
            if skipped {
                warn!("object stream: the object budget is spent; remaining members were skipped");
            }
            Ok(objects)
        }
        Err(err) => {
            // The stream is refused, so its members are dropped: give back what they built, as
            // the whole-stream path, which decodes before it parses, would never have spent it.
            budget.refund(used);
            Err(err)
        }
    }
}

/// A member to read: its object number and decoded offset, and the offset of the next distinct
/// member, which ends it (`None`: it runs to the end of the stream).
struct Span {
    id: u32,
    start: usize,
    end: Option<usize>,
}

/// Read the index into `index` (which holds the stream's first byte) and list the members to
/// read, in offset order. `None` when the index lists them out of order.
fn plan(
    stream: &Stream, inflate: &mut Inflate<'_>, index: &mut Vec<u8>, keep: &impl Fn(u32) -> bool,
) -> Result<Option<Vec<Span>>> {
    let first = first_offset(stream)?;
    // With `/First 0` the byte already read is a member's, but there is no index to list it.
    let missing = first.saturating_sub(index.len());
    if missing > 0 && inflate.append(Some(missing), index)? < missing {
        return Err(Error::InvalidOffset(first));
    }
    let index = index.get(..first).ok_or(Error::InvalidOffset(first))?;
    let index = std::str::from_utf8(index).map_err(|e| Error::InvalidObjectStream(e.to_string()))?;

    // As in the whole-stream path: at most `MAX_OBJECT_STREAM_MEMBERS` entries, a pair with a
    // word that isn't a number is ignored, and an entry repeating the previous offset is skipped.
    // Only the members `keep` accepts are listed, each ending where the next offset begins.
    let mut words = index.split_whitespace().take(2 * super::MAX_OBJECT_STREAM_MEMBERS);
    let mut count = 0;
    let mut plan = Vec::new();
    let mut previous = None;
    let mut open: Option<(u32, usize)> = None;
    while let Some(id) = words.next() {
        count += 1;
        let Some(offset) = words.next() else { break };
        count += 1;
        let (Ok(id), Ok(offset)) = (u32::from_str(id), u32::from_str(offset)) else {
            continue;
        };
        let offset = first.saturating_add(offset as usize);
        match previous {
            Some(previous) if offset < previous => return Ok(None),
            Some(previous) if offset == previous => {
                warn!("duplicate offset in object stream");
                continue;
            }
            _ => previous = Some(offset),
        }
        if let Some((id, start)) = open.take() {
            plan.push(Span {
                id,
                start,
                end: Some(offset),
            });
        }
        if keep(id) {
            open = Some((id, offset));
        }
    }
    if let Some((id, start)) = open {
        plan.push(Span { id, start, end: None });
    }
    // Counted over the whole index, as the whole-stream path counts it.
    count += words.count();
    check_member_count(stream, count)?;
    Ok(Some(plan))
}

/// Read and parse the members `plan` lists, adding the nodes they keep to `used`. Returns whether
/// a member was skipped because the budget was spent.
fn read_members(
    inflate: &mut Inflate<'_>, plan: &[Span], budget: &NodeBudget, objects: &mut BTreeMap<ObjectId, Object>,
    used: &mut usize,
) -> Result<bool> {
    let mut skipped = false;
    let mut bytes = Vec::new();
    let mut batch: Vec<Member> = Vec::new();
    let mut parse = |bytes: &mut Vec<u8>, batch: &mut Vec<Member>, skipped: &mut bool| {
        if batch.is_empty() {
            return;
        }
        let parsed = parse_members(bytes, batch, budget, objects);
        *used = used.saturating_add(parsed.used);
        *skipped |= parsed.skipped;
        bytes.clear();
        batch.clear();
    };
    for span in plan {
        inflate.skip(Some(span.start.saturating_sub(inflate.position())))?;
        let len = span.end.map(|end| end.saturating_sub(span.start));
        // Once the budget is spent nothing this stream holds is given back to it before the
        // next batch reserves, so the members left would be skipped: don't keep their bytes.
        if budget.spent() {
            skipped = true;
            inflate.skip(len)?;
            continue;
        }
        let from = bytes.len();
        inflate.append(len, &mut bytes)?;
        let read = bytes.get(from..).unwrap_or_default();
        let start = from + read.iter().take_while(|byte| byte.is_ascii_whitespace()).count();
        if read.is_empty() {
            // The stream ends before this offset, so it ends before the later ones too.
            warn!("out-of-bounds offset in object stream");
            break;
        }
        if start >= bytes.len() {
            warn!("only whitespace after offset in object stream");
            bytes.truncate(from);
            continue;
        }
        batch.push((span.id, start, bytes.len()));
        if bytes.len() >= BATCH_BYTES {
            parse(&mut bytes, &mut batch, &mut skipped);
        }
    }
    parse(&mut bytes, &mut batch, &mut skipped);
    Ok(skipped)
}

/// Decoded bytes [`Inflate`] holds between reads: the unit it decodes in, so the bytes it
/// decodes depend only on the stream, not on how they are read.
const WINDOW: usize = 64 << 10;

/// A Flate decoder that counts what it decodes against the decompression limit.
///
/// It decodes the deflate data after the two-byte zlib header, without the Adler-32 checksum.
/// `Stream::decompress_zlib` decodes the same data: it falls back to exactly this when the header
/// is bad, and like it a decoding error ends the stream with the bytes decoded before it. Unlike
/// it, those are all the bytes decoded before the error: flate2's `Read` drops the output of the
/// read that finds an error, so how much of a corrupt stream (or of one whose checksum alone is
/// wrong) the whole-stream path keeps depends on how its reads fall.
struct Inflate<'a> {
    /// Deflate data not yet decoded.
    input: &'a [u8],
    decompress: Decompress,
    window: Vec<u8>,
    /// The bytes of `window` decoded and not yet read.
    start: usize,
    end: usize,
    limit: Option<usize>,
    /// Bytes decoded so far, read or not.
    decoded: usize,
    /// The decoder has reached the end of the data, or an error.
    ended: bool,
    over_limit: bool,
}

impl<'a> Inflate<'a> {
    fn new(content: &'a [u8], limit: Option<usize>) -> Self {
        Self {
            input: content.get(2..).unwrap_or_default(),
            decompress: Decompress::new(false),
            window: Vec::new(),
            start: 0,
            end: 0,
            limit,
            decoded: 0,
            ended: false,
            over_limit: false,
        }
    }

    /// The offset of the next byte to read in the decoded stream.
    fn position(&self) -> usize {
        self.decoded.saturating_sub(self.end.saturating_sub(self.start))
    }

    fn over_limit(&self) -> Error {
        DecompressError::MemoryLimitExceeded {
            limit: self.limit.unwrap_or_default(),
        }
        .into()
    }

    /// The decoded bytes not yet read, decoding more when there are none: empty at the end.
    fn fill(&mut self) -> Result<&[u8]> {
        while self.start == self.end && !self.ended {
            // At most one byte past the limit is decoded, as `Stream::read_capped` reads, so a
            // stream over it is refused as soon as the excess is decoded.
            let room = match self.limit {
                Some(limit) => WINDOW.min(limit.saturating_add(1).saturating_sub(self.decoded)),
                None => WINDOW,
            };
            if self.window.len() < room {
                self.window.resize(room, 0);
            }
            let (input, output) = (self.decompress.total_in(), self.decompress.total_out());
            let status = self.decompress.decompress(
                self.input,
                self.window.get_mut(..room).unwrap_or_default(),
                FlushDecompress::None,
            );
            // Both are at most the lengths of the slices passed.
            let consumed = usize::try_from(self.decompress.total_in().saturating_sub(input)).unwrap_or(usize::MAX);
            let written = usize::try_from(self.decompress.total_out().saturating_sub(output)).unwrap_or(usize::MAX);
            self.input = self.input.get(consumed..).unwrap_or_default();
            self.decoded = self.decoded.saturating_add(written);
            (self.start, self.end) = (0, written.min(room));
            match status {
                Ok(Status::StreamEnd) => self.ended = true,
                // No progress: the data ends before its last block.
                Ok(_) if written == 0 && consumed == 0 => self.ended = true,
                Ok(_) => {}
                Err(err) => {
                    warn!(
                        "object stream: {err}; keeping the {} bytes decoded before it",
                        self.decoded
                    );
                    self.ended = true;
                }
            }
            if self.limit.is_some_and(|limit| self.decoded > limit) {
                (self.start, self.end, self.ended, self.over_limit) = (0, 0, true, true);
            }
        }
        if self.over_limit {
            return Err(self.over_limit());
        }
        Ok(self.window.get(self.start..self.end).unwrap_or_default())
    }

    /// Read up to `len` more bytes (`None`: all of them), appending them to `out` if given.
    /// Returns how many were read: fewer than `len` only at the end of the stream.
    fn read(&mut self, len: Option<usize>, mut out: Option<&mut Vec<u8>>) -> Result<usize> {
        let mut left = len.unwrap_or(usize::MAX);
        let mut read = 0;
        while left > 0 {
            let bytes = self.fill()?;
            if bytes.is_empty() {
                break;
            }
            let take = bytes.len().min(left);
            if let Some(out) = out.as_deref_mut() {
                out.extend_from_slice(bytes.get(..take).unwrap_or_default());
            }
            self.start += take;
            left -= take;
            read += take;
        }
        Ok(read)
    }

    /// Append up to `len` more decoded bytes (`None`: all of them) to `out`; returns how many.
    fn append(&mut self, len: Option<usize>, out: &mut Vec<u8>) -> Result<usize> {
        self.read(len, Some(out))
    }

    /// Decode and drop up to `len` bytes (`None`: all of them).
    fn skip(&mut self, len: Option<usize>) -> Result<()> {
        self.read(len, None).map(drop)
    }

    /// Check that the rest of the stream is within the limit, decoding it only when it could
    /// exceed it. Without a limit nothing is left to check, so the rest is never decoded.
    fn finish(&mut self) -> Result<()> {
        if self.over_limit {
            return Err(self.over_limit());
        }
        let Some(limit) = self.limit else {
            return Ok(());
        };
        // What the decoder still holds plus what its unread input can expand to.
        let most = self
            .input
            .len()
            .saturating_mul(MAX_DEFLATE_RATIO)
            .saturating_add(DECODER_SLACK)
            .saturating_add(self.decoded);
        if self.ended || most <= limit {
            return Ok(());
        }
        self.skip(None)
    }
}
