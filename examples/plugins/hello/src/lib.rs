//! Example PdfCraft plug-in, deliberately generic: it knows nothing about
//! any specific document feature. It follows the Craft plug-in convention
//! shared with PhotoCraft and VectorCraft: one `.wasm` file, the manifest
//! embedded in the module, no host imports.

use pdfcraft_plugin_sdk::{Response, plugin};

plugin! {
    manifest: r#"{
        "id": "org.pdfcraft.example.hello",
        "name": "Hello (example plugin)",
        "version": "0.1.0",
        "kind": "command",
        "author": "PdfCraft contributors",
        "description": "Example plugin: summarizes the document and can mark it reviewed.",
        "commands": [
            {"id": "summarize", "label": "Summarize document", "description": "Say how large the current document is."},
            {"id": "mark_reviewed", "label": "Mark as reviewed", "description": "Set the document's Keywords info key to 'reviewed'."}
        ]
    }"#;
    run: |command, context| {
        let document = &context["document"];
        match command {
            "summarize" => {
                let name = document["name"].as_str().unwrap_or("untitled");
                let pages = document["page_count"].as_u64().unwrap_or(0);
                let page = document["current_page"].as_u64().unwrap_or(0);
                Response::message(format!("{name}: {pages} pages, you are on page {}.", page + 1))
            }
            "mark_reviewed" => Response::message("Marked as reviewed.").set_document_info("Keywords", "reviewed"),
            _ => Response::message("Unknown command."),
        }
    }
}
