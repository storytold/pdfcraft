use pdf_typographic_converter::emitters::{export_docx, export_pptx, export_xlsx};
use pdf_typographic_converter::layout::{
    DocumentLayout, PageImage, PageLayout, ScriptType, TextLine, TextRun,
};
use std::fs::{self, File};
use std::io::Read;

fn create_test_layout() -> DocumentLayout {
    let runs = vec![
        TextRun {
            text: "Heading".to_string(),
            font_name: "Helvetica-Bold".to_string(),
            font_size: 20.0,
            is_bold: true,
            is_italic: false,
            script: ScriptType::Normal,
            has_leading_tab: false,
            tracking_delta: 0.0,
            bounds: (50.0, 750.0, 150.0, 770.0),
        },
        TextRun {
            text: "E = mc".to_string(),
            font_name: "Times-Italic".to_string(),
            font_size: 14.0,
            is_bold: false,
            is_italic: true,
            script: ScriptType::Normal,
            has_leading_tab: false,
            tracking_delta: 0.5,
            bounds: (50.0, 700.0, 95.0, 714.0),
        },
        TextRun {
            text: "2".to_string(),
            font_name: "Helvetica".to_string(),
            font_size: 9.0,
            is_bold: false,
            is_italic: false,
            script: ScriptType::Superscript,
            has_leading_tab: false,
            tracking_delta: 0.0,
            bounds: (96.0, 702.0, 102.0, 711.0),
        },
        TextRun {
            text: "H".to_string(),
            font_name: "Times-Roman".to_string(),
            font_size: 14.0,
            is_bold: false,
            is_italic: false,
            script: ScriptType::Normal,
            has_leading_tab: false,
            tracking_delta: 0.0,
            bounds: (50.0, 660.0, 62.0, 674.0),
        },
        TextRun {
            text: "2".to_string(),
            font_name: "Times-Roman".to_string(),
            font_size: 9.0,
            is_bold: false,
            is_italic: false,
            script: ScriptType::Subscript,
            has_leading_tab: false,
            tracking_delta: 0.0,
            bounds: (63.0, 659.0, 69.0, 668.0),
        },
        TextRun {
            text: "O".to_string(),
            font_name: "Times-Roman".to_string(),
            font_size: 14.0,
            is_bold: false,
            is_italic: false,
            script: ScriptType::Normal,
            has_leading_tab: false,
            tracking_delta: 0.0,
            bounds: (70.0, 660.0, 82.0, 674.0),
        },
        TextRun {
            text: "Tabbed Column".to_string(),
            font_name: "Helvetica".to_string(),
            font_size: 12.0,
            is_bold: false,
            is_italic: false,
            script: ScriptType::Normal,
            has_leading_tab: true,
            tracking_delta: 0.0,
            bounds: (250.0, 620.0, 350.0, 632.0),
        },
    ];

    let line = TextLine {
        baseline_y: 700.0,
        runs,
        line_height: 20.0,
        bounds: (50.0, 620.0, 350.0, 770.0),
    };

    let page1 = PageLayout {
        page_index: 0,
        width: 595.0,
        height: 842.0,
        lines: vec![line],
        images: vec![],
    };

    let page2 = PageLayout {
        page_index: 1,
        width: 595.0,
        height: 842.0,
        lines: vec![],
        images: vec![],
    };

    DocumentLayout {
        pages: vec![page1, page2],
    }
}

#[test]
fn test_export_docx() {
    let temp_dir = std::env::temp_dir();
    let docx_path = temp_dir.join("test_output.docx");
    let layout = create_test_layout();

    export_docx(&layout, &docx_path).expect("DOCX export failed");
    assert!(docx_path.exists());
    let metadata = fs::metadata(&docx_path).expect("Failed to get metadata");
    assert!(metadata.len() > 0, "DOCX file must not be empty");

    // Inspect the generated zip archive for word/document.xml
    let file = File::open(&docx_path).expect("Failed to open DOCX");
    let mut archive = zip::ZipArchive::new(file).expect("Failed to parse zip archive");
    let mut doc_xml_entry = archive
        .by_name("word/document.xml")
        .expect("word/document.xml must exist in DOCX");

    let mut xml_content = String::new();
    doc_xml_entry
        .read_to_string(&mut xml_content)
        .expect("Failed to read document.xml");

    assert!(
        xml_content.contains("Heading"),
        "DOCX should contain 'Heading'"
    );
    assert!(
        xml_content.contains("E = mc"),
        "DOCX should contain 'E = mc'"
    );
    assert!(
        xml_content.contains("Tabbed Column"),
        "DOCX should contain 'Tabbed Column'"
    );
    assert!(
        xml_content.contains("superscript"),
        "DOCX should contain superscript vertical alignment"
    );
    assert!(
        xml_content.contains("subscript"),
        "DOCX should contain subscript vertical alignment"
    );

    let _ = fs::remove_file(&docx_path);
}

#[test]
fn test_export_xlsx() {
    let temp_dir = std::env::temp_dir();
    let xlsx_path = temp_dir.join("test_output.xlsx");
    let layout = create_test_layout();

    export_xlsx(&layout, &xlsx_path).expect("XLSX export failed");
    assert!(xlsx_path.exists());
    let metadata = fs::metadata(&xlsx_path).expect("Failed to get metadata");
    assert!(metadata.len() > 0, "XLSX file must not be empty");

    // Inspect the generated zip archive for xl/workbook.xml
    let file = File::open(&xlsx_path).expect("Failed to open XLSX");
    let mut archive = zip::ZipArchive::new(file).expect("Failed to parse zip archive");
    let mut wb_xml = String::new();
    {
        let mut wb_entry = archive
            .by_name("xl/workbook.xml")
            .expect("xl/workbook.xml must exist in XLSX");
        wb_entry
            .read_to_string(&mut wb_xml)
            .expect("Failed to read workbook.xml");
    }

    assert!(
        wb_xml.contains("Export"),
        "XLSX should contain consolidated sheet 'Export'"
    );

    let mut ss_entry = archive
        .by_name("xl/sharedStrings.xml")
        .expect("xl/sharedStrings.xml must exist in XLSX");
    let mut ss_xml = String::new();
    ss_entry
        .read_to_string(&mut ss_xml)
        .expect("Failed to read sharedStrings.xml");
    assert!(ss_xml.contains("Heading"), "Shared strings should contain Page 1 text");
    assert!(ss_xml.contains("Page 2"), "Shared strings should contain Page 2 divider");

    let _ = fs::remove_file(&xlsx_path);
}

#[test]
fn test_export_pptx() {
    let temp_dir = std::env::temp_dir();
    let pptx_path = temp_dir.join("test_output.pptx");
    let layout = create_test_layout();

    export_pptx(&layout, &pptx_path).expect("PPTX export failed");
    assert!(pptx_path.exists());
    let metadata = fs::metadata(&pptx_path).expect("Failed to get metadata");
    assert!(metadata.len() > 0, "PPTX file must not be empty");

    // Inspect the generated zip archive for ppt/presentation.xml and slides
    let file = File::open(&pptx_path).expect("Failed to open PPTX");
    let mut archive = zip::ZipArchive::new(file).expect("Failed to parse zip archive");

    assert!(archive.by_name("[Content_Types].xml").is_ok());
    assert!(archive.by_name("_rels/.rels").is_ok());
    assert!(archive.by_name("ppt/presentation.xml").is_ok());
    assert!(archive.by_name("ppt/slides/slide1.xml").is_ok());

    let mut slide1_entry = archive
        .by_name("ppt/slides/slide1.xml")
        .expect("slide1.xml must exist");
    let mut slide1_xml = String::new();
    slide1_entry
        .read_to_string(&mut slide1_xml)
        .expect("Failed to read slide1.xml");

    assert!(
        slide1_xml.contains("Heading"),
        "Slide should contain 'Heading'"
    );
    assert!(
        slide1_xml.contains("baseline=\"30000\""),
        "Slide should contain superscript baseline offset"
    );
    assert!(
        slide1_xml.contains("baseline=\"-25000\""),
        "Slide should contain subscript baseline offset"
    );
    assert!(
        slide1_xml.contains("<a:spAutoFit/>"),
        "Slide should have text auto-fit"
    );

    let _ = fs::remove_file(&pptx_path);
}

#[test]
fn test_export_docx_with_images() {
    let temp_dir = std::env::temp_dir();
    let docx_path = temp_dir.join("test_docx_with_images.docx");

    let tiny_png = vec![
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8,
        6, 0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 120, 156, 99, 248, 207, 192,
        240, 31, 0, 5, 0, 1, 255, 137, 91, 38, 59, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];

    let page = PageLayout {
        page_index: 0,
        width: 612.0,
        height: 792.0,
        lines: vec![],
        images: vec![PageImage {
            bytes: tiny_png,
            bounds: (50.0, 700.0, 150.0, 600.0),
            width_pt: 100.0,
            height_pt: 100.0,
        }],
    };

    let doc = DocumentLayout { pages: vec![page] };

    export_docx(&doc, &docx_path).expect("DOCX export with image failed");
    assert!(docx_path.exists());
    let metadata = fs::metadata(&docx_path).expect("Failed to get metadata");
    assert!(metadata.len() > 0, "DOCX file must not be empty");

    let file = File::open(&docx_path).expect("Failed to open DOCX");
    let mut archive = zip::ZipArchive::new(file).expect("Failed to parse zip archive");
    assert!(
        archive.by_name("word/document.xml").is_ok(),
        "document.xml must exist"
    );

    let mut doc_xml_entry = archive
        .by_name("word/document.xml")
        .expect("word/document.xml must exist");
    let mut xml_content = String::new();
    doc_xml_entry
        .read_to_string(&mut xml_content)
        .expect("Failed to read document.xml");
    assert!(
        xml_content.contains("drawing") || xml_content.contains("pic"),
        "DOCX should contain drawing/pic element"
    );

    let _ = fs::remove_file(&docx_path);
}

#[test]
fn test_export_pptx_with_images() {
    let temp_dir = std::env::temp_dir();
    let pptx_path = temp_dir.join("test_pptx_with_images.pptx");

    let tiny_png = vec![
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8,
        6, 0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 120, 156, 99, 248, 207, 192,
        240, 31, 0, 5, 0, 1, 255, 137, 91, 38, 59, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];

    let page = PageLayout {
        page_index: 0,
        width: 612.0,
        height: 792.0,
        lines: vec![],
        images: vec![PageImage {
            bytes: tiny_png,
            bounds: (50.0, 500.0, 150.0, 600.0),
            width_pt: 100.0,
            height_pt: 100.0,
        }],
    };

    let doc = DocumentLayout { pages: vec![page] };

    export_pptx(&doc, &pptx_path).expect("PPTX export with image failed");
    assert!(pptx_path.exists());
    let metadata = fs::metadata(&pptx_path).expect("Failed to get metadata");
    assert!(metadata.len() > 0, "PPTX file must not be empty");

    let file = File::open(&pptx_path).expect("Failed to open PPTX");
    let mut archive = zip::ZipArchive::new(file).expect("Failed to parse zip archive");
    assert!(
        archive.by_name("ppt/slides/slide1.xml").is_ok(),
        "slide1.xml must exist"
    );
    assert!(
        archive.by_name("ppt/media/image_1_1.png").is_ok(),
        "ppt/media/image_1_1.png must exist"
    );
    assert!(
        archive.by_name("ppt/slides/_rels/slide1.xml.rels").is_ok(),
        "slide1.xml.rels must exist"
    );

    let mut slide_xml = String::new();
    archive
        .by_name("ppt/slides/slide1.xml")
        .expect("slide1.xml must exist")
        .read_to_string(&mut slide_xml)
        .expect("Failed to read slide1.xml");
    assert!(
        slide_xml.contains("<p:pic>"),
        "Slide should contain <p:pic> element"
    );

    let _ = fs::remove_file(&pptx_path);
}

#[test]
fn test_export_xlsx_with_images() {
    let temp_dir = std::env::temp_dir();
    let xlsx_path = temp_dir.join("test_xlsx_with_images.xlsx");

    let tiny_png = vec![
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8,
        6, 0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 120, 156, 99, 248, 207, 192,
        240, 31, 0, 5, 0, 1, 255, 137, 91, 38, 59, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];

    let page = PageLayout {
        page_index: 0,
        width: 612.0,
        height: 792.0,
        lines: vec![],
        images: vec![PageImage {
            bytes: tiny_png,
            bounds: (50.0, 500.0, 150.0, 600.0),
            width_pt: 100.0,
            height_pt: 100.0,
        }],
    };

    let doc = DocumentLayout { pages: vec![page] };

    export_xlsx(&doc, &xlsx_path).expect("XLSX export with image failed");
    assert!(xlsx_path.exists());
    let metadata = fs::metadata(&xlsx_path).expect("Failed to get metadata");
    assert!(metadata.len() > 0, "XLSX file must not be empty");

    let file = File::open(&xlsx_path).expect("Failed to open XLSX");
    let archive = zip::ZipArchive::new(file).expect("Failed to parse zip archive");
    // In OpenXML XLSX, inserted images produce a drawing relationship or xl/media/image1.png
    let has_image = archive
        .file_names()
        .any(|name| name.contains("drawing") || name.contains("media") || name.contains("image"));
    assert!(has_image, "XLSX should contain drawing or media image entries");

    let _ = fs::remove_file(&xlsx_path);
}
