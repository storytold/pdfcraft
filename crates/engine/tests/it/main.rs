//! The crate's integration tests, linked into one test binary.

mod support {
    pub mod certificate;
    pub mod font_fixture;
}

mod certificate_actions;
mod certificate_display;
mod certificate_scoped_refresh;
mod certificate_working_file;
mod document_fonts;
mod form_rotation;
mod marked_content_isolation;
mod ocr_edit;
mod office_export_images;
mod private_image_resize;
mod private_text_glyphs;
