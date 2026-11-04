//! pdfcraft-plugin-sdk — write a PdfCraft plug-in in Rust.
//!
//! A plug-in is **one `.wasm` file** (the convention shared with
//! PhotoCraft and VectorCraft): the manifest is embedded in the module
//! and returned through `pdf_manifest`; there is no package format.
//! Declare your plug-in with [`plugin!`], giving the manifest JSON and
//! one handler that answers a command with a [`Response`].
//!
//! ```ignore
//! use pdfcraft_plugin_sdk::{plugin, Response};
//!
//! plugin! {
//!     manifest: r#"{"id":"org.example.hello","name":"Hello","version":"0.1.0","kind":"command",
//!                    "commands":[{"id":"greet","label":"Greet"}]}"#;
//!     run: |command, context| {
//!         let pages = context["document"]["page_count"].as_u64().unwrap_or(0);
//!         Response::message(format!("This document has {pages} pages."))
//!     }
//! }
//! ```

use std::cell::RefCell;

pub use serde_json::Value;

/// An `invoke` request from the host.
#[derive(Clone, Debug)]
pub struct Request {
    pub command: String,
    pub context: Value,
}

/// The answer sent back to the host.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Response {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    pub actions: Vec<Action>,
}

impl Response {
    /// Answer with just a message for the user.
    pub fn message(text: impl Into<String>) -> Self {
        Self { message: Some(text.into()), actions: Vec::new() }
    }

    /// Ask the host to set one document information key.
    pub fn set_document_info(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.actions.push(Action::SetDocumentInfo { key: key.into(), value: value.into() });
        self
    }

    /// Ask the host to open PDF bytes as a new document.
    pub fn open_document(mut self, name: impl Into<String>, pdf_bytes: &[u8]) -> Self {
        use base64::Engine as _;
        self.actions.push(Action::OpenDocument {
            name: name.into(),
            data_base64: base64::engine::general_purpose::STANDARD.encode(pdf_bytes),
        });
        self
    }
}

/// Effects a plug-in may request (mirrors the host's protocol).
#[derive(Clone, Debug, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    ShowMessage { text: String },
    SetDocumentInfo { key: String, value: String },
    OpenDocument { name: String, data_base64: String },
}

/// Declare the plug-in: its embedded manifest JSON and its command handler.
#[macro_export]
macro_rules! plugin {
    (manifest: $manifest:expr; run: $run:expr) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn pdf_abi_version() -> i32 {
            1
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn pdf_manifest() -> i64 {
            $crate::__packed($manifest.as_bytes())
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn pdf_alloc(size: u32) -> *mut u8 {
            $crate::__alloc(size)
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn pdf_run(ptr: *const u8, len: u32, _params: *const u8, _params_len: u32) -> i64 {
            $crate::__run(ptr, len, $run)
        }
    };
}

thread_local! {
    static INPUT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    static OUTPUT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

#[doc(hidden)]
pub fn __packed(bytes: &[u8]) -> i64 {
    OUTPUT.with(|out| {
        let mut buf = out.borrow_mut();
        buf.clear();
        buf.extend_from_slice(bytes);
        ((buf.len() as i64) << 32) | (buf.as_ptr() as i64)
    })
}

#[doc(hidden)]
pub fn __alloc(size: u32) -> *mut u8 {
    // Allocateur à progression : l'hôte écrit l'entrée puis les paramètres
    // dans deux blocs distincts ; aucun ne doit écraser l'autre.
    INPUT.with(|input| {
        let mut buf = input.borrow_mut();
        let start = buf.len();
        buf.resize(start + size as usize, 0);
        unsafe { buf.as_mut_ptr().add(start) }
    })
}

#[derive(serde::Deserialize)]
struct RawInput {
    #[serde(default)]
    command: String,
    #[serde(default)]
    context: Value,
}

#[doc(hidden)]
pub fn __run(ptr: *const u8, len: u32, handler: fn(&str, &Value) -> Response) -> i64 {
    // The host wrote `len` bytes at `ptr` into the buffer `pdf_alloc`
    // handed it; reading them back is the ABI's core contract.
    let payload: &[u8] = if ptr.is_null() { &[] } else { unsafe { std::slice::from_raw_parts(ptr, len as usize) } };
    let Ok(raw) = serde_json::from_slice::<RawInput>(payload) else {
        return -1;
    };
    let response = handler(&raw.command, &raw.context);
    match serde_json::to_vec(&response) {
        Ok(bytes) => __packed(&bytes),
        Err(_) => -2,
    }
}
