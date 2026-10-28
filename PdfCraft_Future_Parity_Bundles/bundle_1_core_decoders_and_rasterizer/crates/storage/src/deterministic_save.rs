//! Deterministic Full-Save Serializer for PDF Documents.
//! Guarantees byte-for-byte reproducible saves across platforms and repeated runs.

use std::collections::BTreeMap;
use std::fmt::Write;

pub struct DeterministicPdfWriter {
    objects: BTreeMap<u32, Vec<u8>>,
    root_id: u32,
    info_id: Option<u32>,
}

impl DeterministicPdfWriter {
    pub fn new(root_id: u32) -> Self {
        Self {
            objects: BTreeMap::new(),
            root_id,
            info_id: None,
        }
    }

    pub fn add_object(&mut self, id: u32, payload: Vec<u8>) {
        self.objects.insert(id, payload);
    }

    pub fn set_info_id(&mut self, info_id: u32) {
        self.info_id = Some(info_id);
    }

    /// Serializes entire document into byte-for-byte reproducible output.
    pub fn serialize(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n");

        let mut offsets: BTreeMap<u32, usize> = BTreeMap::new();

        // Write objects in strictly ascending numerical order
        for (&id, payload) in &self.objects {
            let offset = out.len();
            offsets.insert(id, offset);

            let header = format!("{} 0 obj\n", id);
            out.extend_from_slice(header.as_bytes());
            out.extend_from_slice(payload);
            if !payload.ends_with(b"\n") {
                out.push(b'\n');
            }
            out.extend_from_slice(b"endobj\n");
        }

        let startxref_offset = out.len();

        // Write standard 20-byte fixed-width cross-reference table
        let max_id = self.objects.keys().copied().max().unwrap_or(0);
        let total_count = max_id + 1;

        let mut xref_str = String::new();
        writeln!(&mut xref_str, "xref").unwrap();
        writeln!(&mut xref_str, "0 {}", total_count).unwrap();
        // Object 0 is always free linked list head
        writeln!(&mut xref_str, "0000000000 65535 f \r").unwrap();

        for id in 1..total_count {
            if let Some(&off) = offsets.get(&id) {
                writeln!(&mut xref_str, "{:010} 00000 n \r", off).unwrap();
            } else {
                writeln!(&mut xref_str, "0000000000 00000 f \r").unwrap();
            }
        }

        out.extend_from_slice(xref_str.as_bytes());

        // Deterministic document digest ID: hash of object count and root_id
        let deterministic_id = format!("{:016X}{:016X}", self.objects.len(), self.root_id);

        let mut trailer_str = String::new();
        writeln!(&mut trailer_str, "trailer").unwrap();
        write!(&mut trailer_str, "<< /Size {} /Root {} 0 R /ID [<{2}> <{2}>]", total_count, self.root_id, deterministic_id).unwrap();
        if let Some(info) = self.info_id {
            write!(&mut trailer_str, " /Info {} 0 R", info).unwrap();
        }
        writeln!(&mut trailer_str, " >>").unwrap();
        writeln!(&mut trailer_str, "startxref").unwrap();
        writeln!(&mut trailer_str, "{}", startxref_offset).unwrap();
        write!(&mut trailer_str, "%%EOF\n").unwrap();

        out.extend_from_slice(trailer_str.as_bytes());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deterministic_save_is_reproducible() {
        let mut writer1 = DeterministicPdfWriter::new(1);
        writer1.add_object(1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec());
        writer1.add_object(2, b"<< /Type /Pages /Count 0 >>".to_vec());
        let out1 = writer1.serialize();

        let mut writer2 = DeterministicPdfWriter::new(1);
        // Add in reverse order; output must remain identical
        writer2.add_object(2, b"<< /Type /Pages /Count 0 >>".to_vec());
        writer2.add_object(1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec());
        let out2 = writer2.serialize();

        assert_eq!(out1, out2);
        assert!(out1.starts_with(b"%PDF-1.7\n"));
        assert!(out1.ends_with(b"%%EOF\n"));
    }
}
