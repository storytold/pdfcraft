//! OpenType / TrueType Table Directory Parser per ISO/IEC 14496-22.

use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct TableRecord {
    pub tag: [u8; 4],
    pub checksum: u32,
    pub offset: usize,
    pub length: usize,
}

pub struct FontTableDirectory {
    pub sfnt_version: u32,
    pub tables: HashMap<[u8; 4], TableRecord>,
}

impl FontTableDirectory {
    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < 12 {
            return None;
        }

        let sfnt_version = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
        let num_tables = u16::from_be_bytes([data[4], data[5]]) as usize;

        let mut tables = HashMap::new();
        let mut cursor = 12usize;

        for _ in 0..num_tables {
            if cursor + 16 > data.len() {
                break;
            }
            let tag = [data[cursor], data[cursor + 1], data[cursor + 2], data[cursor + 3]];
            let checksum = u32::from_be_bytes([data[cursor + 4], data[cursor + 5], data[cursor + 6], data[cursor + 7]]);
            let offset = u32::from_be_bytes([data[cursor + 8], data[cursor + 9], data[cursor + 10], data[cursor + 11]]) as usize;
            let length = u32::from_be_bytes([data[cursor + 12], data[cursor + 13], data[cursor + 14], data[cursor + 15]]) as usize;

            tables.insert(tag, TableRecord {
                tag,
                checksum,
                offset,
                length,
            });
            cursor += 16;
        }

        Some(Self {
            sfnt_version,
            tables,
        })
    }

    pub fn has_table(&self, tag: &[u8; 4]) -> bool {
        self.tables.contains_key(tag)
    }
}
