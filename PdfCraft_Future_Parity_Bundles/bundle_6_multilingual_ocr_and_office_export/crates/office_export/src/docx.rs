//! ISO/IEC 29500 WordprocessingML (.docx) Document Generator.

pub struct DocxDocumentBuilder {
    body_xml: String,
}

impl Default for DocxDocumentBuilder {
    fn default() -> Self {
        Self {
            body_xml: String::new(),
        }
    }
}

impl DocxDocumentBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends a paragraph with optional bold/italic styling.
    pub fn add_paragraph(&mut self, text: &str, is_bold: bool, is_italic: bool) {
        self.body_xml.push_str("<w:p><w:r>");
        if is_bold || is_italic {
            self.body_xml.push_str("<w:rPr>");
            if is_bold {
                self.body_xml.push_str("<w:b/>");
            }
            if is_italic {
                self.body_xml.push_str("<w:i/>");
            }
            self.body_xml.push_str("</w:rPr>");
        }
        self.body_xml.push_str(&format!("<w:t>{}</w:t>", Self::escape_xml(text)));
        self.body_xml.push_str("</w:r></w:p>");
    }

    /// Appends a heading paragraph (Level 1..6).
    pub fn add_heading(&mut self, level: u8, text: &str) {
        let style = format!("Heading{}", level.clamp(1, 6));
        self.body_xml.push_str(&format!("<w:p><w:pPr><w:pStyle w:val=\"{}\"/></w:pPr>", style));
        self.body_xml.push_str(&format!("<w:r><w:t>{}</w:t></w:r></w:p>", Self::escape_xml(text)));
    }

    /// Appends a table grid.
    pub fn add_table(&mut self, rows: &[Vec<String>]) {
        self.body_xml.push_str("<w:tbl><w:tblPr><w:tblBorders><w:top w:val=\"single\" w:sz=\"4\"/><w:bottom w:val=\"single\" w:sz=\"4\"/></w:tblBorders></w:tblPr>");
        for row in rows {
            self.body_xml.push_str("<w:tr>");
            for cell in row {
                self.body_xml.push_str(&format!("<w:tc><w:p><w:r><w:t>{}</w:t></w:r></w:p></w:tc>", Self::escape_xml(cell)));
            }
            self.body_xml.push_str("</w:tr>");
        }
        self.body_xml.push_str("</w:tbl>");
    }

    /// Generates complete `word/document.xml` XML payload.
    pub fn build_document_xml(&self) -> String {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\
<w:body>{}<w:sectPr/></w:body></w:document>",
            self.body_xml
        )
    }

    fn escape_xml(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
            .replace('\'', "&apos;")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_docx_builder_structure() {
        let mut doc = DocxDocumentBuilder::new();
        doc.add_heading(1, "Annual Financial Report");
        doc.add_paragraph("Total revenue exceeded projections.", true, false);
        doc.add_table(&[
            vec!["Metric".into(), "Value".into()],
            vec!["Revenue".into(), "$1.2B".into()],
        ]);

        let xml = doc.build_document_xml();
        assert!(xml.contains("Heading1"));
        assert!(xml.contains("<w:b/>"));
        assert!(xml.contains("<w:tbl>"));
        assert!(xml.contains("$1.2B"));
    }
}
