//! PdfCraft's window and page titles, with an explicitly selected channel name.
//!
//! The window names the active document (with • while it has unsaved changes) followed by the
//! app's own name, so a Nightly window says so wherever the stable one would say "PdfCraft".

pub(crate) fn os_title(app_name: &str, document: Option<(&str, bool)>) -> String {
    let Some((name, dirty)) = document else { return app_name.into() };
    format!("{name}{} \u{2014} {app_name}", if dirty { " \u{2022}" } else { "" })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nightly_titles_keep_the_channel_across_document_dirty_and_named_updates() {
        let channel = "PdfCraft Nightly";
        assert_eq!(os_title(channel, None), channel);
        for dirty in [false, true] {
            let shown = os_title(channel, Some(("my document.pdf", dirty)));
            assert!(shown.contains("my document.pdf"));
            assert!(shown.ends_with(channel));
            assert_eq!(shown.contains('\u{2022}'), dirty);
        }
    }

    #[test]
    fn stable_titles_are_the_document_then_the_app_name() {
        assert_eq!(os_title("PdfCraft", None), "PdfCraft");
        assert_eq!(os_title("PdfCraft", Some(("work.pdf", false))), "work.pdf \u{2014} PdfCraft");
        assert_eq!(os_title("PdfCraft", Some(("work.pdf", true))), "work.pdf \u{2022} \u{2014} PdfCraft");
    }

    #[test]
    fn an_unsaved_title_marks_only_the_document() {
        let dirty = os_title("PdfCraft", Some(("work.pdf", true)));
        assert!(dirty.starts_with("work.pdf"));
        assert!(!dirty.starts_with('\u{2022}'), "the channel's own name is never marked unsaved");
    }
}
