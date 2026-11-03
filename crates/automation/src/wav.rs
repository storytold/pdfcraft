//! Bounded, spec-compliant RIFF/WAVE parser for voice comments (AGENTS.md §4, ISO 32000-2 §13.3).
//!
//! Enforces memory limits, checked arithmetic, strictly validates RIFF chunk
//! structure, handles arbitrary chunk ordering, validates padding bytes on
//! odd-length chunks, and rejects malformed, oversized, or truncated audio files.

#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

/// Maximum allowed WAV file size: 64 MiB (protects against memory exhaustion).
pub const MAX_WAV_FILE_SIZE: usize = 64 * 1024 * 1024;

/// Maximum audio duration allowed for a voice comment: 1 hour (3600 seconds).
pub const MAX_AUDIO_DURATION_SECS: u64 = 3600;

/// Minimum supported sample rate (4 kHz).
pub const MIN_SAMPLE_RATE: u32 = 4_000;

/// Maximum supported sample rate (96 kHz, per ISO 32000-2 sound dict range).
pub const MAX_SAMPLE_RATE: u32 = 96_000;

/// Parsed sound data formatted for PDF ISO 32000-2 `/Sound` stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WavAudio {
    /// Audio sampling rate in Hertz.
    pub rate: u32,
    /// Number of audio channels (1 for mono, 2 for stereo).
    pub channels: u16,
    /// Number of bits per sample (16).
    pub bits_per_sample: u16,
    /// Audio samples encoded as big-endian signed 16-bit PCM for PDF `/E /Signed /B 16`.
    pub samples: Vec<u8>,
}

/// Errors occurring during RIFF/WAVE parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WavError {
    EmptyInput,
    FileTooLarge(usize),
    NotRiff,
    NotWave,
    TruncatedHeader,
    TruncatedChunkHeader,
    TruncatedChunkData { tag: [u8; 4], expected: usize, available: usize },
    DuplicateFmtChunk,
    DuplicateDataChunk,
    MissingFmtChunk,
    MissingDataChunk,
    DataBeforeFmt,
    FmtChunkTooSmall(usize),
    UnsupportedFormat(u16),
    InvalidChannels(u16),
    InvalidSampleRate(u32),
    UnsupportedBitDepth(u16),
    BlockAlignMismatch { expected: u16, actual: u16 },
    ByteRateMismatch { expected: u32, actual: u32 },
    EmptyAudio,
    IncompleteFrame { data_len: usize, block_align: usize },
    DurationExceeded { seconds: u64, max: u64 },
    IntegerOverflow,
    Io(String),
}

impl std::fmt::Display for WavError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyInput => write!(f, "empty audio file"),
            Self::FileTooLarge(size) => write!(f, "audio file is too large ({size} bytes, maximum is {MAX_WAV_FILE_SIZE} bytes)"),
            Self::NotRiff => write!(f, "not a RIFF file (missing 'RIFF' signature)"),
            Self::NotWave => write!(f, "not a WAVE file (missing 'WAVE' format tag)"),
            Self::TruncatedHeader => write!(f, "truncated RIFF/WAVE header"),
            Self::TruncatedChunkHeader => write!(f, "truncated chunk header in WAV file"),
            Self::TruncatedChunkData { tag, expected, available } => {
                let tag_str = String::from_utf8_lossy(tag);
                write!(f, "truncated '{tag_str}' chunk: expected {expected} bytes, found {available} bytes")
            }
            Self::DuplicateFmtChunk => write!(f, "malformed WAV: multiple 'fmt ' chunks"),
            Self::DuplicateDataChunk => write!(f, "malformed WAV: multiple 'data' chunks"),
            Self::MissingFmtChunk => write!(f, "malformed WAV: missing 'fmt ' chunk"),
            Self::MissingDataChunk => write!(f, "malformed WAV: missing 'data' chunk"),
            Self::DataBeforeFmt => write!(f, "malformed WAV: 'data' chunk appears before 'fmt ' chunk"),
            Self::FmtChunkTooSmall(sz) => write!(f, "malformed WAV: 'fmt ' chunk size {sz} is smaller than standard 16 bytes"),
            Self::UnsupportedFormat(code) => write!(f, "unsupported WAV format {code}: only uncompressed linear PCM is supported"),
            Self::InvalidChannels(ch) => write!(f, "unsupported channel count {ch}: only mono (1) or stereo (2) is supported"),
            Self::InvalidSampleRate(r) => write!(f, "unsupported sample rate {r} Hz: must be between {MIN_SAMPLE_RATE} and {MAX_SAMPLE_RATE} Hz"),
            Self::UnsupportedBitDepth(b) => write!(f, "unsupported bit depth {b}: only 16-bit or 8-bit linear PCM is supported"),
            Self::BlockAlignMismatch { expected, actual } => {
                write!(f, "corrupted WAV header: block align mismatch (declared {actual}, expected {expected})")
            }
            Self::ByteRateMismatch { expected, actual } => {
                write!(f, "corrupted WAV header: byte rate mismatch (declared {actual}, expected {expected})")
            }
            Self::EmptyAudio => write!(f, "audio recording has no samples"),
            Self::IncompleteFrame { data_len, block_align } => {
                write!(f, "corrupted WAV data: data size {data_len} is not a multiple of frame size {block_align}")
            }
            Self::DurationExceeded { seconds, max } => {
                write!(f, "audio duration ({seconds}s) exceeds maximum permitted length ({max}s)")
            }
            Self::IntegerOverflow => write!(f, "integer overflow while parsing WAV chunk offsets"),
            Self::Io(err) => write!(f, "I/O error reading audio file: {err}"),
        }
    }
}

impl std::error::Error for WavError {}

/// Parse raw WAV bytes into validated audio metadata and big-endian 16-bit PCM samples
/// formatted for PDF ISO 32000-2 `/Sound` objects.
pub fn parse_wav_samples(data: &[u8]) -> Result<WavAudio, WavError> {
    if data.is_empty() {
        return Err(WavError::EmptyInput);
    }
    if data.len() > MAX_WAV_FILE_SIZE {
        return Err(WavError::FileTooLarge(data.len()));
    }
    if data.len() < 12 {
        return Err(WavError::TruncatedHeader);
    }
    if data.get(0..4) != Some(b"RIFF") {
        return Err(WavError::NotRiff);
    }
    let riff_size = u32::from_le_bytes([
        *data.get(4).ok_or(WavError::TruncatedHeader)?,
        *data.get(5).ok_or(WavError::TruncatedHeader)?,
        *data.get(6).ok_or(WavError::TruncatedHeader)?,
        *data.get(7).ok_or(WavError::TruncatedHeader)?,
    ]) as usize;

    if data.get(8..12) != Some(b"WAVE") {
        return Err(WavError::NotWave);
    }
    if riff_size < 4 {
        return Err(WavError::TruncatedHeader);
    }

    let mut pos = 12usize;
    let mut fmt_info: Option<FmtHeader> = None;
    let mut audio_data: Option<RawAudioData> = None;

    while pos < data.len() {
        if pos.checked_add(8).ok_or(WavError::IntegerOverflow)? > data.len() {
            // Trailing junk of less than 8 bytes: if we already have fmt and data, accept it;
            // otherwise, the chunk header is truncated.
            if fmt_info.is_some() && audio_data.is_some() {
                break;
            }
            return Err(WavError::TruncatedChunkHeader);
        }

        let id = [
            *data.get(pos).ok_or(WavError::TruncatedChunkHeader)?,
            *data.get(pos + 1).ok_or(WavError::TruncatedChunkHeader)?,
            *data.get(pos + 2).ok_or(WavError::TruncatedChunkHeader)?,
            *data.get(pos + 3).ok_or(WavError::TruncatedChunkHeader)?,
        ];
        let chunk_len = u32::from_le_bytes([
            *data.get(pos + 4).ok_or(WavError::TruncatedChunkHeader)?,
            *data.get(pos + 5).ok_or(WavError::TruncatedChunkHeader)?,
            *data.get(pos + 6).ok_or(WavError::TruncatedChunkHeader)?,
            *data.get(pos + 7).ok_or(WavError::TruncatedChunkHeader)?,
        ]) as usize;

        let payload_start = pos.checked_add(8).ok_or(WavError::IntegerOverflow)?;
        let payload_end = payload_start.checked_add(chunk_len).ok_or(WavError::IntegerOverflow)?;

        if payload_end > data.len() {
            return Err(WavError::TruncatedChunkData { tag: id, expected: chunk_len, available: data.len().saturating_sub(payload_start) });
        }

        let chunk_slice = data.get(payload_start..payload_end).ok_or(WavError::TruncatedChunkData {
            tag: id,
            expected: chunk_len,
            available: data.len().saturating_sub(payload_start),
        })?;

        match &id {
            b"fmt " => {
                if fmt_info.is_some() {
                    return Err(WavError::DuplicateFmtChunk);
                }
                fmt_info = Some(parse_fmt(chunk_slice)?);
            }
            b"data" => {
                if fmt_info.is_none() {
                    return Err(WavError::DataBeforeFmt);
                }
                if audio_data.is_some() {
                    return Err(WavError::DuplicateDataChunk);
                }
                audio_data = Some(RawAudioData { payload_start, chunk_len });
            }
            _ => {
                // Ignore recognized metadata chunks (LIST, INFO, bext, JUNK, etc.) safely
            }
        }

        // In RIFF, every chunk is padded to a 2-byte boundary (1 pad byte if chunk_len is odd).
        let pad = chunk_len % 2;
        pos = payload_end.checked_add(pad).ok_or(WavError::IntegerOverflow)?;
    }

    let fmt = fmt_info.ok_or(WavError::MissingFmtChunk)?;
    let raw = audio_data.ok_or(WavError::MissingDataChunk)?;

    if raw.chunk_len == 0 {
        return Err(WavError::EmptyAudio);
    }
    let block_align = usize::from(fmt.block_align);
    if block_align == 0 || raw.chunk_len % block_align != 0 {
        return Err(WavError::IncompleteFrame { data_len: raw.chunk_len, block_align });
    }

    let total_frames = (raw.chunk_len / block_align) as u64;
    if total_frames == 0 {
        return Err(WavError::EmptyAudio);
    }
    let duration_secs = total_frames / u64::from(fmt.sample_rate);
    if duration_secs > MAX_AUDIO_DURATION_SECS {
        return Err(WavError::DurationExceeded { seconds: duration_secs, max: MAX_AUDIO_DURATION_SECS });
    }

    let payload = data.get(raw.payload_start..raw.payload_start + raw.chunk_len).ok_or(WavError::TruncatedChunkData {
        tag: *b"data",
        expected: raw.chunk_len,
        available: 0,
    })?;

    // Convert samples to big-endian signed 16-bit PCM for PDF ISO 32000-2 §13.3 (/E /Signed /B 16)
    let samples = match fmt.bits_per_sample {
        16 => {
            // Little-endian signed 16-bit to big-endian signed 16-bit
            let mut out = Vec::with_capacity(payload.len());
            for &[lo, hi] in payload.as_chunks::<2>().0 {
                out.push(hi);
                out.push(lo);
            }
            out
        }
        8 => {
            // Unsigned 8-bit PCM (0..255, 128 is zero) to signed 16-bit big-endian
            let mut out = Vec::with_capacity(payload.len().saturating_mul(2));
            for &byte in payload {
                let sample_16 = ((i32::from(byte) - 128) << 8) as i16;
                let be_bytes = sample_16.to_be_bytes();
                out.push(be_bytes[0]);
                out.push(be_bytes[1]);
            }
            out
        }
        _ => return Err(WavError::UnsupportedBitDepth(fmt.bits_per_sample)),
    };

    Ok(WavAudio { rate: fmt.sample_rate, channels: fmt.channels, bits_per_sample: 16, samples })
}

#[derive(Debug, Clone, Copy)]
struct FmtHeader {
    sample_rate: u32,
    channels: u16,
    bits_per_sample: u16,
    block_align: u16,
}

fn parse_fmt(chunk: &[u8]) -> Result<FmtHeader, WavError> {
    if chunk.len() < 16 {
        return Err(WavError::FmtChunkTooSmall(chunk.len()));
    }
    let format = u16::from_le_bytes([
        *chunk.first().ok_or(WavError::FmtChunkTooSmall(chunk.len()))?,
        *chunk.get(1).ok_or(WavError::FmtChunkTooSmall(chunk.len()))?,
    ]);
    if format != 1 {
        return Err(WavError::UnsupportedFormat(format));
    }

    let channels = u16::from_le_bytes([
        *chunk.get(2).ok_or(WavError::FmtChunkTooSmall(chunk.len()))?,
        *chunk.get(3).ok_or(WavError::FmtChunkTooSmall(chunk.len()))?,
    ]);
    if !(1..=2).contains(&channels) {
        return Err(WavError::InvalidChannels(channels));
    }

    let sample_rate = u32::from_le_bytes([
        *chunk.get(4).ok_or(WavError::FmtChunkTooSmall(chunk.len()))?,
        *chunk.get(5).ok_or(WavError::FmtChunkTooSmall(chunk.len()))?,
        *chunk.get(6).ok_or(WavError::FmtChunkTooSmall(chunk.len()))?,
        *chunk.get(7).ok_or(WavError::FmtChunkTooSmall(chunk.len()))?,
    ]);
    if !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&sample_rate) {
        return Err(WavError::InvalidSampleRate(sample_rate));
    }

    let byte_rate = u32::from_le_bytes([
        *chunk.get(8).ok_or(WavError::FmtChunkTooSmall(chunk.len()))?,
        *chunk.get(9).ok_or(WavError::FmtChunkTooSmall(chunk.len()))?,
        *chunk.get(10).ok_or(WavError::FmtChunkTooSmall(chunk.len()))?,
        *chunk.get(11).ok_or(WavError::FmtChunkTooSmall(chunk.len()))?,
    ]);

    let block_align = u16::from_le_bytes([
        *chunk.get(12).ok_or(WavError::FmtChunkTooSmall(chunk.len()))?,
        *chunk.get(13).ok_or(WavError::FmtChunkTooSmall(chunk.len()))?,
    ]);

    let bits_per_sample = u16::from_le_bytes([
        *chunk.get(14).ok_or(WavError::FmtChunkTooSmall(chunk.len()))?,
        *chunk.get(15).ok_or(WavError::FmtChunkTooSmall(chunk.len()))?,
    ]);
    if bits_per_sample != 16 && bits_per_sample != 8 {
        return Err(WavError::UnsupportedBitDepth(bits_per_sample));
    }

    let expected_align = channels.saturating_mul(bits_per_sample / 8);
    if block_align != expected_align {
        return Err(WavError::BlockAlignMismatch { expected: expected_align, actual: block_align });
    }

    let expected_byte_rate = sample_rate.saturating_mul(u32::from(block_align));
    if byte_rate != expected_byte_rate {
        return Err(WavError::ByteRateMismatch { expected: expected_byte_rate, actual: byte_rate });
    }

    Ok(FmtHeader { sample_rate, channels, bits_per_sample, block_align })
}

struct RawAudioData {
    payload_start: usize,
    chunk_len: usize,
}

/// Helper for automation/comments callers returning `(rate, channels, big_endian_samples)`
/// with a string error message for tool response formatting.
pub fn wav_samples(data: &[u8]) -> std::result::Result<(u32, u8, Vec<u8>), String> {
    parse_wav_samples(data).map(|w| (w.rate, w.channels as u8, w.samples)).map_err(|e| e.to_string())
}

/// Reads bounded WAV audio data from any `std::io::Read` stream.
///
/// Takes at most `MAX_WAV_FILE_SIZE + 1` bytes to prevent unbounded memory allocation
/// or hangs on infinite streams, and returns [`WavError::FileTooLarge`] if the input exceeds
/// [`MAX_WAV_FILE_SIZE`].
pub fn read_wav_bounded<R: std::io::Read>(mut reader: R) -> Result<Vec<u8>, WavError> {
    use std::io::Read as _;
    let mut data = Vec::new();
    reader.by_ref().take(MAX_WAV_FILE_SIZE as u64 + 1).read_to_end(&mut data).map_err(|e| WavError::Io(e.to_string()))?;
    if data.len() > MAX_WAV_FILE_SIZE {
        return Err(WavError::FileTooLarge(data.len()));
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Constructs a synthetic valid RIFF/WAVE file for testing.
    fn make_wav(channels: u16, rate: u32, bits: u16, pcm: &[u8], extra_chunks: &[(&[u8; 4], &[u8])]) -> Vec<u8> {
        let block_align = channels * (bits / 8);
        let byte_rate = rate * u32::from(block_align);
        let mut fmt_payload = Vec::new();
        fmt_payload.extend_from_slice(&1u16.to_le_bytes()); // PCM
        fmt_payload.extend_from_slice(&channels.to_le_bytes());
        fmt_payload.extend_from_slice(&rate.to_le_bytes());
        fmt_payload.extend_from_slice(&byte_rate.to_le_bytes());
        fmt_payload.extend_from_slice(&block_align.to_le_bytes());
        fmt_payload.extend_from_slice(&bits.to_le_bytes());

        let mut body = Vec::new();
        for (tag, data) in extra_chunks {
            body.extend_from_slice(*tag);
            body.extend_from_slice(&(data.len() as u32).to_le_bytes());
            body.extend_from_slice(data);
            if data.len() % 2 != 0 {
                body.push(0); // Pad
            }
        }
        body.extend_from_slice(b"fmt ");
        body.extend_from_slice(&(fmt_payload.len() as u32).to_le_bytes());
        body.extend_from_slice(&fmt_payload);

        body.extend_from_slice(b"data");
        body.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
        body.extend_from_slice(pcm);
        if !pcm.len().is_multiple_of(2) {
            body.push(0);
        }

        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(4u32 + body.len() as u32).to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(&body);
        out
    }

    #[test]
    fn parses_standard_16bit_mono_wav() {
        let pcm = [0x12, 0x34, 0x56, 0x78]; // Two 16-bit samples: 0x3412, 0x7856
        let wav = make_wav(1, 44100, 16, &pcm, &[]);
        let audio = parse_wav_samples(&wav).expect("valid wav");
        assert_eq!(audio.rate, 44100);
        assert_eq!(audio.channels, 1);
        assert_eq!(audio.bits_per_sample, 16);
        // Little-endian in WAV becomes big-endian in PDF (/E /Signed)
        assert_eq!(audio.samples, vec![0x34, 0x12, 0x78, 0x56]);
    }

    #[test]
    fn parses_standard_16bit_stereo_wav() {
        let pcm = [0x01, 0x02, 0x03, 0x04];
        let wav = make_wav(2, 48000, 16, &pcm, &[]);
        let audio = parse_wav_samples(&wav).expect("valid stereo wav");
        assert_eq!(audio.rate, 48000);
        assert_eq!(audio.channels, 2);
        assert_eq!(audio.samples, vec![0x02, 0x01, 0x04, 0x03]);
    }

    #[test]
    fn parses_8bit_mono_wav_and_converts_to_16bit_signed() {
        // 128 is center (0 in signed 16-bit), 255 is max positive, 0 is max negative
        let pcm = [128, 255, 0];
        let wav = make_wav(1, 16000, 8, &pcm, &[]);
        let audio = parse_wav_samples(&wav).expect("valid 8-bit wav");
        assert_eq!(audio.rate, 16000);
        assert_eq!(audio.channels, 1);
        assert_eq!(audio.bits_per_sample, 16);
        // Sample 128 -> (128 - 128) << 8 = 0 -> [0x00, 0x00]
        assert_eq!(&audio.samples[0..2], &[0x00, 0x00]);
        // Sample 255 -> (255 - 128) << 8 = 127 << 8 = 32512 -> [0x7F, 0x00]
        assert_eq!(&audio.samples[2..4], &[0x7F, 0x00]);
    }

    #[test]
    fn parses_wav_with_metadata_chunk_before_fmt() {
        // Real-world WAVs from DAWs like ProTools or Audacity contain 'bext' or 'JUNK' before 'fmt '
        let junk_data = b"Some metadata before fmt chunk";
        let pcm = [0xAA, 0xBB];
        let wav = make_wav(1, 22050, 16, &pcm, &[(b"JUNK", junk_data)]);
        let audio = parse_wav_samples(&wav).expect("should handle chunk before fmt");
        assert_eq!(audio.rate, 22050);
        assert_eq!(audio.samples, vec![0xBB, 0xAA]);
    }

    #[test]
    fn parses_wav_with_odd_sized_metadata_chunk_and_padding() {
        // Odd length chunk (15 bytes) must be padded with 1 byte to 2-byte boundary
        let odd_junk = b"123456789012345";
        assert_eq!(odd_junk.len(), 15);
        let pcm = [0x11, 0x22];
        let wav = make_wav(1, 32000, 16, &pcm, &[(b"bext", odd_junk)]);
        let audio = parse_wav_samples(&wav).expect("should align odd chunk properly");
        assert_eq!(audio.rate, 32000);
        assert_eq!(audio.samples, vec![0x22, 0x11]);
    }

    #[test]
    fn parses_wav_with_metadata_chunk_between_fmt_and_data() {
        let block_align = 2u16;
        let byte_rate = 44100u32 * 2;
        let mut fmt_payload = Vec::new();
        fmt_payload.extend_from_slice(&1u16.to_le_bytes());
        fmt_payload.extend_from_slice(&1u16.to_le_bytes());
        fmt_payload.extend_from_slice(&44100u32.to_le_bytes());
        fmt_payload.extend_from_slice(&byte_rate.to_le_bytes());
        fmt_payload.extend_from_slice(&block_align.to_le_bytes());
        fmt_payload.extend_from_slice(&16u16.to_le_bytes());

        let mut body = Vec::new();
        body.extend_from_slice(b"fmt ");
        body.extend_from_slice(&(fmt_payload.len() as u32).to_le_bytes());
        body.extend_from_slice(&fmt_payload);

        let mid_chunk = b"Intermediary Chunk";
        body.extend_from_slice(b"INFO");
        body.extend_from_slice(&(mid_chunk.len() as u32).to_le_bytes());
        body.extend_from_slice(mid_chunk);
        if !mid_chunk.len().is_multiple_of(2) {
            body.push(0);
        }

        let pcm = [0x55, 0x66];
        body.extend_from_slice(b"data");
        body.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
        body.extend_from_slice(&pcm);

        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(4u32 + body.len() as u32).to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(&body);

        let audio = parse_wav_samples(&out).expect("chunk between fmt and data");
        assert_eq!(audio.rate, 44100);
        assert_eq!(audio.samples, vec![0x66, 0x55]);
    }

    #[test]
    fn rejects_missing_fmt_chunk() {
        let mut body = Vec::new();
        let pcm = [0x01, 0x02];
        body.extend_from_slice(b"data");
        body.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
        body.extend_from_slice(&pcm);

        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(4u32 + body.len() as u32).to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(&body);

        assert_eq!(parse_wav_samples(&out), Err(WavError::DataBeforeFmt));
    }

    #[test]
    fn rejects_missing_data_chunk() {
        let block_align = 2u16;
        let byte_rate = 44100u32 * 2;
        let mut fmt_payload = Vec::new();
        fmt_payload.extend_from_slice(&1u16.to_le_bytes());
        fmt_payload.extend_from_slice(&1u16.to_le_bytes());
        fmt_payload.extend_from_slice(&44100u32.to_le_bytes());
        fmt_payload.extend_from_slice(&byte_rate.to_le_bytes());
        fmt_payload.extend_from_slice(&block_align.to_le_bytes());
        fmt_payload.extend_from_slice(&16u16.to_le_bytes());

        let mut body = Vec::new();
        body.extend_from_slice(b"fmt ");
        body.extend_from_slice(&(fmt_payload.len() as u32).to_le_bytes());
        body.extend_from_slice(&fmt_payload);

        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(4u32 + body.len() as u32).to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(&body);

        assert_eq!(parse_wav_samples(&out), Err(WavError::MissingDataChunk));
    }

    #[test]
    fn rejects_truncated_chunk_headers() {
        let mut wav = make_wav(1, 44100, 16, &[0x11, 0x22], &[]);
        // Truncate in the middle of a chunk header
        wav.truncate(14);
        assert_eq!(parse_wav_samples(&wav), Err(WavError::TruncatedChunkHeader));
    }

    #[test]
    fn rejects_truncated_chunk_payload() {
        let mut wav = make_wav(1, 44100, 16, &[0x11, 0x22, 0x33, 0x44], &[]);
        wav.truncate(wav.len() - 2); // Cut off 2 data bytes
        assert!(matches!(parse_wav_samples(&wav), Err(WavError::TruncatedChunkData { .. })));
    }

    #[test]
    fn rejects_overflow_attempt_chunk_length() {
        let mut wav = make_wav(1, 44100, 16, &[0x11, 0x22], &[]);
        // Find 'data' chunk and overwrite length with 0xFFFFFFFF
        for i in 0..wav.len() - 8 {
            if &wav[i..i + 4] == b"data" {
                wav[i + 4..i + 8].copy_from_slice(&u32::MAX.to_le_bytes());
                break;
            }
        }
        assert!(matches!(parse_wav_samples(&wav), Err(WavError::TruncatedChunkData { .. } | WavError::IntegerOverflow)));
    }

    #[test]
    fn rejects_invalid_channels() {
        let pcm = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66];
        let wav = make_wav(3, 44100, 16, &pcm, &[]);
        assert_eq!(parse_wav_samples(&wav), Err(WavError::InvalidChannels(3)));

        let wav0 = make_wav(0, 44100, 16, &pcm, &[]);
        assert_eq!(parse_wav_samples(&wav0), Err(WavError::InvalidChannels(0)));
    }

    #[test]
    fn rejects_invalid_sample_rates() {
        let pcm = [0x11, 0x22];
        let wav_low = make_wav(1, 2000, 16, &pcm, &[]);
        assert_eq!(parse_wav_samples(&wav_low), Err(WavError::InvalidSampleRate(2000)));

        let wav_high = make_wav(1, 192_000, 16, &pcm, &[]);
        assert_eq!(parse_wav_samples(&wav_high), Err(WavError::InvalidSampleRate(192_000)));
    }

    #[test]
    fn rejects_unsupported_bit_depth() {
        let pcm = [0x11, 0x22, 0x33];
        let wav24 = make_wav(1, 44100, 24, &pcm, &[]);
        assert_eq!(parse_wav_samples(&wav24), Err(WavError::UnsupportedBitDepth(24)));
    }

    #[test]
    fn rejects_unsupported_compression_format() {
        // Format 3 = IEEE float
        let mut wav = make_wav(1, 44100, 16, &[0x11, 0x22], &[]);
        for i in 0..wav.len() - 8 {
            if &wav[i..i + 4] == b"fmt " {
                wav[i + 8] = 3; // Change format tag from 1 (PCM) to 3 (Float)
                wav[i + 9] = 0;
                break;
            }
        }
        assert_eq!(parse_wav_samples(&wav), Err(WavError::UnsupportedFormat(3)));
    }

    #[test]
    fn rejects_oversized_file() {
        // Construct header claiming gigantic size
        let huge = vec![0u8; MAX_WAV_FILE_SIZE + 1];
        assert_eq!(parse_wav_samples(&huge), Err(WavError::FileTooLarge(huge.len())));
    }

    #[test]
    fn rejects_malformed_riff_size() {
        let mut wav = make_wav(1, 44100, 16, &[0x11, 0x22], &[]);
        wav[4..8].copy_from_slice(&2u32.to_le_bytes()); // Less than 4 bytes
        assert_eq!(parse_wav_samples(&wav), Err(WavError::TruncatedHeader));
    }

    #[test]
    fn rejects_duplicate_fmt_chunk() {
        let block_align = 2u16;
        let byte_rate = 44100u32 * 2;
        let mut fmt_payload = Vec::new();
        fmt_payload.extend_from_slice(&1u16.to_le_bytes());
        fmt_payload.extend_from_slice(&1u16.to_le_bytes());
        fmt_payload.extend_from_slice(&44100u32.to_le_bytes());
        fmt_payload.extend_from_slice(&byte_rate.to_le_bytes());
        fmt_payload.extend_from_slice(&block_align.to_le_bytes());
        fmt_payload.extend_from_slice(&16u16.to_le_bytes());

        let mut body = Vec::new();
        body.extend_from_slice(b"fmt ");
        body.extend_from_slice(&(fmt_payload.len() as u32).to_le_bytes());
        body.extend_from_slice(&fmt_payload);
        body.extend_from_slice(b"fmt ");
        body.extend_from_slice(&(fmt_payload.len() as u32).to_le_bytes());
        body.extend_from_slice(&fmt_payload);

        let pcm = [0x11, 0x22];
        body.extend_from_slice(b"data");
        body.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
        body.extend_from_slice(&pcm);

        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(4u32 + body.len() as u32).to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(&body);

        assert_eq!(parse_wav_samples(&out), Err(WavError::DuplicateFmtChunk));
    }

    #[test]
    fn handles_fuzzed_corrupted_inputs_without_panicking() {
        let bad_inputs: &[&[u8]] = &[
            b"",
            b"R",
            b"RIFF",
            b"RIFF\0\0\0\0WAVE",
            b"RIFF\x20\0\0\0WAVEfmt \x10\0\0\0\x01\0\x01\0D\xac\0\0\x88X\x01\0\x02\0\x10\0data\xff\xff\xff\xff",
            &[0xFF; 64],
            &[0x00; 128],
        ];

        for input in bad_inputs {
            let res = parse_wav_samples(input);
            assert!(res.is_err(), "input should fail gracefully: {input:?}");
        }
    }

    #[test]
    fn read_wav_bounded_caps_read_at_max_size_plus_one() {
        struct CountingReader {
            total_available: usize,
            bytes_yielded: usize,
        }

        impl std::io::Read for CountingReader {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                let remaining = self.total_available.saturating_sub(self.bytes_yielded);
                if remaining == 0 {
                    return Ok(0);
                }
                let to_read = buf.len().min(remaining);
                buf[..to_read].fill(0x55);
                self.bytes_yielded += to_read;
                Ok(to_read)
            }
        }

        // Reader with MAX_WAV_FILE_SIZE + 2048 available bytes
        let mut reader = CountingReader { total_available: MAX_WAV_FILE_SIZE + 2048, bytes_yielded: 0 };

        let result = read_wav_bounded(&mut reader);
        assert_eq!(result, Err(WavError::FileTooLarge(MAX_WAV_FILE_SIZE + 1)));
        assert_eq!(reader.bytes_yielded, MAX_WAV_FILE_SIZE + 1);

        // Reader with valid size
        let mut valid_reader = CountingReader { total_available: 1024, bytes_yielded: 0 };
        let valid_result = read_wav_bounded(&mut valid_reader).expect("reads valid size");
        assert_eq!(valid_result.len(), 1024);
        assert_eq!(valid_reader.bytes_yielded, 1024);
    }
}
