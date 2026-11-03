//! Extracts voice comment audio streams as WAV PCM.
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use lopdf::{Document, Object};
use std::panic::{AssertUnwindSafe, catch_unwind};

const MAX_SOUND_BYTES: usize = 48_000 * 2 * 2 * 3600;

pub fn sound_wav(bytes: &[u8], password: Option<&str>, page: usize, index: usize) -> Result<Vec<u8>, String> {
    let run = || -> Result<Vec<u8>, String> {
        let options = lopdf::LoadOptions { password: password.map(str::to_owned), ..lopdf::LoadOptions::default() };
        let doc = Document::load_mem_with_options(bytes, options).map_err(|e| e.to_string())?;
        let pages = doc.get_pages();
        let page_key = (page.saturating_add(1)) as u32;
        let &page_id = pages.get(&page_key).ok_or_else(|| "page not found".to_string())?;
        let page_dict = doc.get_dictionary(page_id).map_err(|e| e.to_string())?;
        let annots_obj = page_dict.get(b"Annots").map_err(|e| e.to_string())?;
        let annots_arr = match annots_obj {
            Object::Array(arr) => arr,
            Object::Reference(r) => doc.get_object(*r).map_err(|e| e.to_string())?.as_array().map_err(|e| e.to_string())?,
            _ => return Err("annots is not an array".to_string()),
        };
        let annot_ref = annots_arr.get(index).ok_or_else(|| "comment not found".to_string())?;
        let annot_dict = match annot_ref {
            Object::Dictionary(d) => d,
            Object::Reference(r) => doc.get_object(*r).map_err(|e| e.to_string())?.as_dict().map_err(|e| e.to_string())?,
            _ => return Err("annot is not a dictionary".to_string()),
        };
        let sound_obj = annot_dict.get(b"Sound").map_err(|_| "the comment has no recording".to_string())?;
        let stream = match sound_obj {
            Object::Stream(s) => s,
            Object::Reference(r) => doc.get_object(*r).map_err(|e| e.to_string())?.as_stream().map_err(|_| "not a stream".to_string())?,
            _ => return Err("the recording is not a sound stream".to_string()),
        };
        let num = |k: &[u8]| -> Option<f32> {
            stream.dict.get(k).ok().and_then(|o| match o {
                Object::Real(f) => Some(*f),
                Object::Integer(i) => Some(*i as f32),
                Object::Reference(r) => doc.get_object(*r).ok().and_then(|o2| match o2 {
                    Object::Real(f) => Some(*f),
                    Object::Integer(i) => Some(*i as f32),
                    _ => None,
                }),
                _ => None,
            })
        };
        let rate = num(b"R")
            .filter(|r| r.is_finite() && *r >= 1.0 && *r <= 384_000.0)
            .ok_or_else(|| "the recording has no sampling rate".to_string())? as u32;
        let channels = num(b"C").map_or(1, |c| if c.is_finite() { c.clamp(1.0, 8.0) as u16 } else { 1 });
        let bits = num(b"B").map_or(8, |b| if b.is_finite() { b as u16 } else { 8 });
        let encoding = stream
            .dict
            .get(b"E")
            .ok()
            .and_then(|o| match o {
                Object::Name(n) => Some(n.as_slice()),
                Object::Reference(r) => doc.get_object(*r).ok().and_then(|o2| o2.as_name().ok()),
                _ => None,
            })
            .unwrap_or(b"Raw")
            .to_vec();

        let data = if stream.dict.get(b"Filter").is_ok() {
            stream.decompressed_content_with_limit(MAX_SOUND_BYTES).map_err(|e| format!("failed to decompress sound stream: {e}"))?
        } else if stream.content.len() > MAX_SOUND_BYTES {
            return Err(format!("sound stream exceeds limit of {MAX_SOUND_BYTES} bytes"));
        } else {
            stream.content.clone()
        };

        let (pcm, bits) = match (encoding.as_slice(), bits) {
            (b"muLaw", _) => (data.iter().flat_map(|b| mu_law(*b).to_le_bytes()).collect::<Vec<u8>>(), 16),
            (b"ALaw", _) => (data.iter().flat_map(|b| a_law(*b).to_le_bytes()).collect::<Vec<u8>>(), 16),
            (b"Signed", 8) => (data.iter().map(|b| b ^ 0x80).collect(), 8),
            (_, 8) => (data, 8),
            (b"Signed", 16) => (data.as_chunks::<2>().0.iter().flat_map(|s| [s[1], s[0]]).collect(), 16),
            (_, 16) => (data.as_chunks::<2>().0.iter().flat_map(|s| [s[1], s[0] ^ 0x80]).collect(), 16),
            (_, b) => return Err(format!("{b}-bit recordings can't be played")),
        };
        Ok(wav(&pcm, rate, channels, bits))
    };
    catch_unwind(AssertUnwindSafe(run)).unwrap_or_else(|_| Err("reading the recording crashed".to_string()))
}

fn wav(pcm: &[u8], rate: u32, channels: u16, bits: u16) -> Vec<u8> {
    let len = u32::try_from(pcm.len()).unwrap_or(u32::MAX - 36).min(u32::MAX - 36);
    let block = channels.saturating_mul(bits / 8).max(1);
    let mut w = Vec::with_capacity(pcm.len().saturating_add(44));
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(len + 36).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&channels.to_le_bytes());
    w.extend_from_slice(&rate.to_le_bytes());
    w.extend_from_slice(&rate.saturating_mul(u32::from(block)).to_le_bytes());
    w.extend_from_slice(&block.to_le_bytes());
    w.extend_from_slice(&bits.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&len.to_le_bytes());
    w.extend_from_slice(pcm.get(..len as usize).unwrap_or(pcm));
    w
}

fn mu_law(b: u8) -> i16 {
    let u = !b;
    let t = ((i32::from(u & 0x0F) << 3) + 0x84) << ((u & 0x70) >> 4);
    (if u & 0x80 != 0 { 0x84 - t } else { t - 0x84 }) as i16
}

fn a_law(b: u8) -> i16 {
    let a = b ^ 0x55;
    let seg = (a & 0x70) >> 4;
    let mut t = i32::from(a & 0x0F) << 4;
    t = match seg {
        0 => t + 8,
        1 => t + 0x108,
        s => (t + 0x108) << (s - 1),
    };
    (if a & 0x80 != 0 { t } else { -t }) as i16
}
