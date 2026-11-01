//! The palette box (pclr), defined in I.5.3.4.

use alloc::vec::Vec;

use crate::error::{FormatError, Result, bail};
use crate::jp2::ImageBoxes;
use crate::reader::BitReader;

// PdfCraft patch (3).
/// The deepest palette column decoded, as deep as a codestream component.
const MAX_BIT_DEPTH: u8 = 31;

pub(crate) fn parse(boxes: &mut ImageBoxes, data: &[u8]) -> Result<()> {
    let mut reader = BitReader::new(data);
    let num_entries = reader.read_u16().ok_or(FormatError::InvalidBox)? as usize;
    let num_components = reader.read_byte().ok_or(FormatError::InvalidBox)? as usize;

    if num_entries == 0 || num_components == 0 {
        bail!(FormatError::InvalidBox);
    }

    let mut columns = Vec::with_capacity(num_components);
    for _ in 0..num_components {
        let descriptor = reader.read_byte().ok_or(FormatError::InvalidBox)?;
        let bit_depth = (descriptor & 0x7F)
            .checked_add(1)
            .ok_or(FormatError::InvalidBox)?;
        let is_signed = (descriptor & 0x80) != 0;

        if is_signed {
            bail!(FormatError::InvalidBox);
        }

        // PdfCraft patch (3): ISO 15444-1 I.5.3.4 allows columns of 1 to 38 bits, and the
        // descriptor can say 128. Like the codestream's components (`BITPLANE_BIT_SIZE`), this
        // decoder handles at most 31: a deeper column overflowed the shifts that rescale its
        // colours (`1_u32 << bit_depth`), which panicked in debug builds. The box is invalid
        // then, and a lenient decode sets it aside as it does any invalid palette box.
        if bit_depth > MAX_BIT_DEPTH {
            bail!(FormatError::InvalidBox);
        }

        columns.push(PaletteColumn { bit_depth });
    }

    let mut entries = Vec::with_capacity(num_entries);

    for _ in 0..num_entries {
        let mut row = Vec::with_capacity(num_components);

        for column in &columns {
            let num_bytes = (column.bit_depth as usize).div_ceil(8).max(1);
            let raw_bytes = reader
                .read_bytes(num_bytes)
                .ok_or(FormatError::InvalidBox)?;
            let mut raw_value = 0_u64;
            for &byte in raw_bytes {
                raw_value = (raw_value << 8) | byte as u64;
            }

            row.push(raw_value);
        }

        entries.push(row);
    }

    boxes.palette = Some(PaletteBox { entries, columns });

    Ok(())
}

#[derive(Debug, Clone)]
pub(crate) struct PaletteBox {
    pub(crate) entries: Vec<Vec<u64>>,
    pub(crate) columns: Vec<PaletteColumn>,
}

impl PaletteBox {
    #[inline(always)]
    pub(crate) fn map(&self, entry: usize, column: usize) -> Option<u64> {
        self.entries
            .get(entry)
            .and_then(|row| row.get(column))
            .copied()
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct PaletteColumn {
    pub(crate) bit_depth: u8,
}
