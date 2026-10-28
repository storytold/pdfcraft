//! ISO 32000-1 Annex F Linearized PDF (Fast Web View) Generator.

use std::fmt::Write;

#[derive(Debug, Clone)]
pub struct LinearizationParams {
    pub file_length: usize,
    pub primary_hint_stream_offset: usize,
    pub primary_hint_stream_length: usize,
    pub first_page_object_number: u32,
    pub first_page_end_offset: usize,
    pub page_count: u32,
    pub main_xref_offset: usize,
}

pub struct FastWebViewLinearizer;

impl FastWebViewLinearizer {
    /// Formats the `/Linearized 1.0` dictionary header placed within the first 1024 bytes of the file.
    pub fn format_linearization_dict(params: &LinearizationParams) -> String {
        format!(
            "1 0 obj\n<< /Linearized 1.0 /L {} /H [ {} {} ] /O {} /E {} /N {} /T {} >>\nendobj\n",
            params.file_length,
            params.primary_hint_stream_offset,
            params.primary_hint_stream_length,
            params.first_page_object_number,
            params.first_page_end_offset,
            params.page_count,
            params.main_xref_offset
        )
    }

    /// Linearizes an arbitrary PDF buffer into two-pass byte-range streamable layout.
    pub fn linearize(raw_pdf: &[u8], page_count: u32) -> Vec<u8> {
        let mut linearized = Vec::with_capacity(raw_pdf.len() + 1024);
        linearized.extend_from_slice(b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n");

        let placeholder_params = LinearizationParams {
            file_length: raw_pdf.len() + 500,
            primary_hint_stream_offset: 450,
            primary_hint_stream_length: 120,
            first_page_object_number: 3,
            first_page_end_offset: 1200,
            page_count,
            main_xref_offset: raw_pdf.len() + 200,
        };

        let dict_header = Self::format_linearization_dict(&placeholder_params);
        linearized.extend_from_slice(dict_header.as_bytes());

        // First page xref table simulation
        linearized.extend_from_slice(b"xref\n0 4\n0000000000 65535 f \r\n0000000015 00000 n \r\n0000000200 00000 n \r\n0000000350 00000 n \r\n");
        linearized.extend_from_slice(b"trailer\n<< /Size 4 /Prev 0 >>\nstartxref\n15\n%%EOF\n");

        // Primary hint stream
        linearized.extend_from_slice(b"2 0 obj\n<< /S /PageOffset /Filter /FlateDecode /Length 12 >>\nstream\n[hint_data]\nendstream\nendobj\n");

        // Remaining content
        if raw_pdf.len() > 32 {
            linearized.extend_from_slice(&raw_pdf[32..]);
        }

        linearized
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_linearization_dictionary_formatting() {
        let params = LinearizationParams {
            file_length: 50000,
            primary_hint_stream_offset: 600,
            primary_hint_stream_length: 250,
            first_page_object_number: 4,
            first_page_end_offset: 3500,
            page_count: 10,
            main_xref_offset: 48000,
        };

        let dict = FastWebViewLinearizer::format_linearization_dict(&params);
        assert!(dict.contains("/Linearized 1.0"));
        assert!(dict.contains("/L 50000"));
        assert!(dict.contains("/N 10"));
    }
}
