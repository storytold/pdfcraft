//! Media types for embedded files.

/// A media type from the file's extension, for an embedded file stream's `/Subtype` (ISO
/// 32000-2 §7.11.4); `None` when it is not one of the common kinds (the file is still fine
/// without it). One table for both kinds of attachment: the document's `/EmbeddedFiles` and a
/// FileAttachment comment's file.
pub fn mime_for_name(name: &str) -> Option<&'static str> {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase())?;
    Some(match ext.as_str() {
        "pdf" => "application/pdf",
        "txt" | "log" | "md" => "text/plain",
        "csv" => "text/csv",
        "htm" | "html" => "text/html",
        "xml" => "text/xml",
        "json" => "application/json",
        "zip" => "application/zip",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "tif" | "tiff" => "image/tiff",
        "svg" => "image/svg+xml",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "rtf" => "application/rtf",
        "eml" => "message/rfc822",
        "msg" => "application/vnd.ms-outlook",
        _ => return None,
    })
}
