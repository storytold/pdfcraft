//! `ComicInfo.xml`, the metadata file comic book archives (`.cbz`) carry beside their pages:
//! the ComicRack schema, version 2.0, as comic readers and servers (Komga, Kavita, YACReader,
//! KOReader…) read it.

use crate::esc;

/// What a comic archive says about itself. Empty values are left out.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ComicInfo {
    pub title: Option<String>,
    /// The document's subject.
    pub summary: Option<String>,
    /// The document's author.
    pub writer: Option<String>,
    /// The document's language (`/Lang`), a BCP 47 tag such as `en` or `ja-JP`.
    pub language: Option<String>,
    /// Read right to left (the PDF's `/Direction /R2L`): `Manga` is `YesAndRightToLeft`.
    pub right_to_left: bool,
    pub pages: Vec<ComicPage>,
}

/// One page image, in archive order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ComicPage {
    pub width: u32,
    pub height: u32,
    /// The image file's size in bytes.
    pub size: u64,
    /// A bookmark to this page (a chapter title).
    pub bookmark: Option<String>,
}

fn element(out: &mut String, name: &str, value: Option<&str>) {
    if let Some(v) = value.map(str::trim).filter(|v| !v.is_empty()) {
        out.push_str(&format!("  <{name}>{}</{name}>\n", esc(v)));
    }
}

/// The `ComicInfo.xml` for `info`. Elements follow the schema's order (it is an `xs:sequence`).
/// The first page is the front cover; a page wider than it is tall is a double page (a spread).
pub fn comic_info(info: &ComicInfo) -> String {
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<ComicInfo xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" xmlns:xsd=\"http://www.w3.org/2001/XMLSchema\">\n",
    );
    element(&mut out, "Title", info.title.as_deref());
    element(&mut out, "Summary", info.summary.as_deref());
    element(&mut out, "Writer", info.writer.as_deref());
    out.push_str(&format!("  <PageCount>{}</PageCount>\n", info.pages.len()));
    element(&mut out, "LanguageISO", info.language.as_deref());
    if info.right_to_left {
        out.push_str("  <Manga>YesAndRightToLeft</Manga>\n");
    }
    if !info.pages.is_empty() {
        out.push_str("  <Pages>\n");
        for (i, p) in info.pages.iter().enumerate() {
            out.push_str(&format!("    <Page Image=\"{i}\""));
            if i == 0 {
                out.push_str(" Type=\"FrontCover\"");
            }
            if p.width > p.height {
                out.push_str(" DoublePage=\"true\"");
            }
            out.push_str(&format!(" ImageSize=\"{}\"", p.size));
            if let Some(b) = p.bookmark.as_deref().map(str::trim).filter(|b| !b.is_empty()) {
                out.push_str(&format!(" Bookmark=\"{}\"", esc(b)));
            }
            out.push_str(&format!(" ImageWidth=\"{}\" ImageHeight=\"{}\" />\n", p.width, p.height));
        }
        out.push_str("  </Pages>\n");
    }
    out.push_str("</ComicInfo>\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comic_info_lists_metadata_and_pages_in_schema_order() {
        let info = ComicInfo {
            title: Some("Tom & Jerry <1>".into()),
            summary: Some("  ".into()),
            writer: Some("A. Author".into()),
            language: Some("ja".into()),
            right_to_left: true,
            pages: vec![
                ComicPage { width: 800, height: 1200, size: 1000, bookmark: None },
                ComicPage { width: 1600, height: 1200, size: 2000, bookmark: Some("Chapter \"1\"\u{1}".into()) },
            ],
        };
        let xml = comic_info(&info);
        assert!(xml.contains("<Title>Tom &amp; Jerry &lt;1&gt;</Title>"));
        assert!(!xml.contains("<Summary>"), "blank values are left out");
        let order = ["<Title>", "<Writer>", "<PageCount>2<", "<LanguageISO>ja<", "<Manga>YesAndRightToLeft<", "<Pages>"];
        let at: Vec<usize> = order.iter().map(|t| xml.find(t).unwrap()).collect();
        assert!(at.windows(2).all(|w| w[0] < w[1]), "{xml}");
        assert!(xml.contains("<Page Image=\"0\" Type=\"FrontCover\" ImageSize=\"1000\" ImageWidth=\"800\" ImageHeight=\"1200\" />"));
        assert!(xml.contains("<Page Image=\"1\" DoublePage=\"true\" ImageSize=\"2000\" Bookmark=\"Chapter &quot;1&quot;\" ImageWidth=\"1600\""));
    }

    #[test]
    fn an_empty_comic_has_no_pages_element() {
        let xml = comic_info(&ComicInfo::default());
        assert!(xml.contains("<PageCount>0</PageCount>"));
        assert!(!xml.contains("<Pages>") && !xml.contains("<Manga>"));
    }
}
