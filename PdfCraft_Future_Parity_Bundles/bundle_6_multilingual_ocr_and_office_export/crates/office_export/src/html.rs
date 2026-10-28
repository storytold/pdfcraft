//! Semantic HTML5 Structural Exporter.

pub struct HtmlDocumentBuilder {
    title: String,
    content: String,
}

impl HtmlDocumentBuilder {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            content: String::new(),
        }
    }

    pub fn add_heading(&mut self, level: u8, text: &str) {
        let tag = format!("h{}", level.clamp(1, 6));
        self.content.push_str(&format!("<{}>{}</{}>\n", tag, Self::escape_html(text), tag));
    }

    pub fn add_paragraph(&mut self, text: &str) {
        self.content.push_str(&format!("<p>{}</p>\n", Self::escape_html(text)));
    }

    pub fn add_table(&mut self, rows: &[Vec<String>]) {
        self.content.push_str("<table border=\"1\" cellpadding=\"4\" style=\"border-collapse:collapse;\">\n");
        for (i, row) in rows.iter().enumerate() {
            self.content.push_str("  <tr>\n");
            for cell in row {
                let tag = if i == 0 { "th" } else { "td" };
                self.content.push_str(&format!("    <{}>{}</{}>\n", tag, Self::escape_html(cell), tag));
            }
            self.content.push_str("  </tr>\n");
        }
        self.content.push_str("</table>\n");
    }

    pub fn build_html(&self) -> String {
        format!(
            "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n  <meta charset=\"UTF-8\">\n  <title>{}</title>\n</head>\n<body>\n{}</body>\n</html>",
            Self::escape_html(&self.title),
            self.content
        )
    }

    fn escape_html(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
            .replace('\'', "&#39;")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_html_export_structure() {
        let mut builder = HtmlDocumentBuilder::new("Test Document");
        builder.add_heading(1, "Overview");
        builder.add_paragraph("Sample paragraph content.");
        let html = builder.build_html();
        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("<h1>Overview</h1>"));
        assert!(html.contains("<p>Sample paragraph content.</p>"));
    }
}
