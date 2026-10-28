//! Comprehensive Integration Tests for Bundle 1: Core Decoders and Rasterizer.

use pdfcraft_codecs::{decode_ccitt, decode_dct, decode_jpx, CcittParams, FilterPipeline, FilterType};
use pdfcraft_storage::{AttachmentDecryptor, BoundedDocumentReader, BoundedLruCache, DeterministicPdfWriter, DocumentSecurityPolicy};
use pdfcraft_rasterizer::{AxialShading, BlendMode, ColorStop, PageRasterizer, ShadingRenderer, Type3Font};
use pdfcraft_arlington::{ArlingtonType, ArlingtonValidator, PdfVersion};
use std::collections::HashMap;
use std::io::Cursor;

#[test]
fn test_end_to_end_image_decoding_and_rasterization() {
    // 1. Decode JPEG/DCT stream
    let jpeg_data = vec![
        0xFF, 0xD8, // SOI
        0xFF, 0xC0, // SOF0
        0x00, 0x11, 0x08, 0x00, 0x20, 0x00, 0x20, 0x03, 0x01, 0x11, 0x00, 0x02, 0x11, 0x01, 0x03, 0x11, 0x01,
        0xFF, 0xD9, // EOI
    ];
    let dct_img = decode_dct(&jpeg_data, None).expect("JPEG decode failed");
    assert_eq!(dct_img.width, 32);
    assert_eq!(dct_img.height, 32);
    let rgba = dct_img.to_rgba32();

    // 2. Rasterize onto a page
    let mut rasterizer = PageRasterizer::new(64, 64, 1.0);
    rasterizer.draw_image(&rgba, 32, 32);

    // Verify canvas is rendered
    let px = rasterizer.target_buffer.get_pixel(10, 10).unwrap();
    assert_eq!(px[3], 255); // Alpha is fully opaque
}

#[test]
fn test_bounded_storage_and_eff_attachment_security() {
    // 1. Verify /EFF attachment-only encryption policy
    let encrypt_dict = "<< /Filter /Standard /V 5 /R 6 /StmF /Identity /StrF /Identity /EFF /StdCF >>";
    let policy = DocumentSecurityPolicy::parse_from_dict(encrypt_dict);
    assert!(policy.is_attachment_only);
    assert!(!policy.requires_password_to_view_document());
    assert!(policy.requires_password_for_attachments());

    // 2. Decrypt attachment with password
    let decryptor = AttachmentDecryptor::from_password("WorkstationSecurePass", b"eff_salt_bytes");
    let test_data = b"Confidential Corporate Attachment";
    let mut cipher = vec![0u8; 16]; // IV
    for (i, &b) in test_data.iter().enumerate() {
        cipher.push(b ^ decryptor.decrypt_stream(&vec![0u8; 16]).unwrap_or_default().get(0).unwrap_or(&0x55));
    }

    // 3. Test BoundedDocumentReader lazy loading
    let pdf_stream = b"%PDF-1.7\n\
1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
2 0 obj\n<< /Type /Pages /Count 1 /Kids [3 0 R] >>\nendobj\n\
3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>\nendobj\n\
xref\n\
0 4\n\
0000000000 65535 f \r\n\
0000000009 00000 n \r\n\
0000000057 00000 n \r\n\
0000000115 00000 n \r\n\
trailer\n<< /Size 4 /Root 1 0 R >>\n\
startxref\n190\n%%EOF";

    let cursor = Cursor::new(pdf_stream);
    let mut reader = BoundedDocumentReader::open(cursor, 512).expect("Reader failed to open");
    assert_eq!(reader.total_objects(), 4);

    let cat = reader.get_object(1).expect("Failed to get catalog");
    assert!(String::from_utf8_lossy(&cat.payload).contains("/Catalog"));
}

#[test]
fn test_blend_modes_and_axial_shadings() {
    let mut rasterizer = PageRasterizer::new(100, 100, 1.0);

    // Render axial gradient
    let shading = AxialShading {
        x0: 0.0,
        y0: 0.0,
        x1: 100.0,
        y1: 100.0,
        extend_start: true,
        extend_end: true,
        stops: vec![
            ColorStop { t: 0.0, rgba: [255, 255, 0, 255] }, // Yellow
            ColorStop { t: 1.0, rgba: [0, 255, 255, 255] }, // Cyan
        ],
    };
    ShadingRenderer::render_axial(&shading, &mut rasterizer.target_buffer);

    // Composite overlay shape with Multiply blend mode
    rasterizer.current_state.blend_mode = BlendMode::Multiply;
    rasterizer.current_state.fill_color = [128, 128, 128, 255];
    rasterizer.rectangle(20.0, 20.0, 40.0, 40.0);
    rasterizer.fill_path();

    let blended_px = rasterizer.target_buffer.get_pixel(30, 30).unwrap();
    // Multiplied yellow is darker than original yellow
    assert!(blended_px[0] < 255);
}

#[test]
fn test_arlington_model_structural_verification() {
    let validator = ArlingtonValidator::new();

    // Verify Page dictionary conforms to ISO 32000
    let mut page_entries = HashMap::new();
    page_entries.insert("Type".into(), ArlingtonType::Name);
    page_entries.insert("Parent".into(), ArlingtonType::IndirectReference);
    page_entries.insert("MediaBox".into(), ArlingtonType::Array);

    let errors = validator.validate_dict("Page", PdfVersion::V1_7, &page_entries);
    assert!(errors.is_empty(), "Page dictionary failed Arlington validation: {:?}", errors);
}

#[test]
fn test_deterministic_full_save_reproducibility() {
    let mut writer = DeterministicPdfWriter::new(1);
    writer.add_object(1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec());
    writer.add_object(2, b"<< /Type /Pages /Count 0 >>".to_vec());

    let bytes1 = writer.serialize();
    let bytes2 = writer.serialize();

    assert_eq!(bytes1, bytes2, "Deterministic full-save produced divergent byte sequences");
}
