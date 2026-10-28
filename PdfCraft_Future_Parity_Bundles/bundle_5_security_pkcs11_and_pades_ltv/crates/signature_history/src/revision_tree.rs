//! Per-Revision Document History Scanner and Rollback Inspector.

#[derive(Debug, Clone)]
pub struct DocumentRevision {
    pub revision_number: usize,
    pub eof_offset: usize,
    pub is_signed: bool,
    pub signer_name: Option<String>,
    pub signing_time: Option<String>,
    pub byte_range: Option<[usize; 4]>,
}

pub struct RevisionHistoryScanner;

impl RevisionHistoryScanner {
    /// Scans a PDF file buffer for incremental revisions delimited by `%%EOF`.
    pub fn scan_revisions(pdf_data: &[u8]) -> Vec<DocumentRevision> {
        let mut revisions = Vec::new();
        let text = String::from_utf8_lossy(pdf_data);

        let mut search_start = 0;
        let mut rev_count = 0;

        while let Some(eof_pos) = text[search_start..].find("%%EOF") {
            let abs_eof = search_start + eof_pos + 5;
            rev_count += 1;

            let section = &text[search_start..abs_eof];
            let is_signed = section.contains("/Type /Sig") || section.contains("/Type/Sig");

            let signer_name = if is_signed {
                Self::extract_pdf_string(section, "/Name")
            } else {
                None
            };

            let signing_time = if is_signed {
                Self::extract_pdf_string(section, "/M")
            } else {
                None
            };

            revisions.push(DocumentRevision {
                revision_number: rev_count,
                eof_offset: abs_eof,
                is_signed,
                signer_name,
                signing_time,
                byte_range: None,
            });

            search_start = abs_eof;
        }

        if revisions.is_empty() {
            revisions.push(DocumentRevision {
                revision_number: 1,
                eof_offset: pdf_data.len(),
                is_signed: false,
                signer_name: None,
                signing_time: None,
                byte_range: None,
            });
        }

        revisions
    }

    /// Extracts historical document byte slice as it existed at the specified revision.
    pub fn extract_historical_snapshot<'a>(pdf_data: &'a [u8], revision: &DocumentRevision) -> &'a [u8] {
        let slice_end = revision.eof_offset.min(pdf_data.len());
        &pdf_data[..slice_end]
    }

    fn extract_pdf_string(slice: &str, key: &str) -> Option<String> {
        if let Some(pos) = slice.find(key) {
            let after = &slice[pos + key.len()..];
            if let Some(start_paren) = after.find('(') {
                if let Some(end_paren) = after[start_paren + 1..].find(')') {
                    return Some(after[start_paren + 1..start_paren + 1 + end_paren].to_string());
                }
            }
        }
        None
    }
}
