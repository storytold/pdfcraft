//! ISO/IEC 29500 PresentationML (.pptx) Slide Generator.

pub struct PptxSlideBuilder {
    slide_xml: String,
}

impl Default for PptxSlideBuilder {
    fn default() -> Self {
        Self {
            slide_xml: String::new(),
        }
    }
}

impl PptxSlideBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_text_box(&mut self, text: &str, x_emu: u64, y_emu: u64, cx_emu: u64, cy_emu: u64) {
        let shape = format!(
            "<p:sp><p:spPr><a:xfrm><a:off x=\"{}\" y=\"{}\"/><a:ext cx=\"{}\" cy=\"{}\"/></a:xfrm></p:spPr>\
<p:txBody><a:bodyPr/><a:p><a:r><a:t>{}</a:t></a:r></a:p></p:txBody></p:sp>",
            x_emu, y_emu, cx_emu, cy_emu, Self::escape_xml(text)
        );
        self.slide_xml.push_str(&shape);
    }

    pub fn build_slide_xml(&self) -> String {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
<p:sld xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" \
xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\">\
<p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id=\"1\" name=\"\"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>\
<p:grpSpPr/>{}</p:spTree></p:cSld></p:sld>",
            self.slide_xml
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
