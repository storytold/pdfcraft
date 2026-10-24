use pdf_typographic_converter::extract::init_pdfium;
use pdfium_render::prelude::*;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let pdfium = init_pdfium()?;
    let mut document = pdfium.create_new_pdf()?;

    // Page 1: Headings and styled paragraphs with super/subscripts and tabs
    {
        let mut page = document
            .pages_mut()
            .create_page_at_end(PdfPagePaperSize::a4())?;

        // 1. Heading
        let helvetica_bold = document.fonts_mut().helvetica_bold();
        page.objects_mut().create_text_object(
            PdfPoints::new(50.0),
            PdfPoints::new(750.0),
            "Typographic Layout Engine",
            helvetica_bold,
            PdfPoints::new(24.0),
        )?;

        // 2. Normal text
        let helvetica = document.fonts_mut().helvetica();
        page.objects_mut().create_text_object(
            PdfPoints::new(50.0),
            PdfPoints::new(710.0),
            "Extracting glyph metrics and reconstructing lines.",
            helvetica,
            PdfPoints::new(12.0),
        )?;

        // 3. Formula with superscript: E = mc^2
        let times_italic = document.fonts_mut().times_italic();
        page.objects_mut().create_text_object(
            PdfPoints::new(50.0),
            PdfPoints::new(670.0),
            "E = mc",
            times_italic,
            PdfPoints::new(14.0),
        )?;

        // Superscript '2': smaller font (9pt < 14 * 0.78 = 10.92), elevated by 1.8pt (<= 2.0 line cluster, > 1.0 superscript)
        let helvetica_small = document.fonts_mut().helvetica();
        page.objects_mut().create_text_object(
            PdfPoints::new(92.0),
            PdfPoints::new(671.8),
            "2",
            helvetica_small,
            PdfPoints::new(9.0),
        )?;

        // 4. Formula with subscript: H_2O
        let times_roman = document.fonts_mut().times_roman();
        page.objects_mut().create_text_object(
            PdfPoints::new(50.0),
            PdfPoints::new(630.0),
            "H",
            times_roman,
            PdfPoints::new(14.0),
        )?;

        // Subscript '2': smaller font (9pt), lowered by 1pt (y = 629.0 < 630 - 0.5)
        page.objects_mut().create_text_object(
            PdfPoints::new(63.0),
            PdfPoints::new(629.0),
            "2",
            times_roman,
            PdfPoints::new(9.0),
        )?;

        page.objects_mut().create_text_object(
            PdfPoints::new(71.0),
            PdfPoints::new(630.0),
            "O",
            times_roman,
            PdfPoints::new(14.0),
        )?;

        // 5. Line with tab stop
        page.objects_mut().create_text_object(
            PdfPoints::new(50.0),
            PdfPoints::new(590.0),
            "Left Column",
            helvetica,
            PdfPoints::new(12.0),
        )?;
        // Large gap (tab stop): next text object starts at 250.0 (gap = 250 - (50 + ~65) = ~135 > 12 * 1.5)
        page.objects_mut().create_text_object(
            PdfPoints::new(250.0),
            PdfPoints::new(590.0),
            "Right Column",
            helvetica,
            PdfPoints::new(12.0),
        )?;
    }

    // Page 2: Empty page
    {
        let _empty = document
            .pages_mut()
            .create_page_at_end(PdfPagePaperSize::a4())?;
    }

    document.save_to_file("sample.pdf")?;
    println!("Successfully regenerated sample.pdf with superscripts, subscripts, and tabs.");
    Ok(())
}
