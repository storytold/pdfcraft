//! The host that serves this site also carries what a browser cannot: a font face to draw
//! Chinese with, and the OCR models. The browser has neither — it cannot read the machine's
//! installed fonts and it cannot read a file of models — so both come from the server.
//!
//! The calls are same-origin (`/api/...` on the site itself), so there is no CORS to arrange
//! and no second certificate: the browser already trusts this origin to show the page.

use js_sys::Uint8Array;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{Request, RequestInit};

/// The same-origin API the site is served with.
const BASE: &str = "/api";

/// Send a request and return the body, or say what came back.
async fn send(request: Request) -> Result<Vec<u8>, String> {
    let window = web_sys::window().ok_or("no window")?;
    let response = JsFuture::from(window.fetch_with_request(&request)).await.map_err(|e| format!("the request failed: {e:?}"))?;
    let response: web_sys::Response = response.dyn_into().map_err(|_| "the server did not answer with a response")?;
    let status = response.status();
    // `array_buffer` hands back a promise or throws, so the Result comes first.
    let promise = response.array_buffer().map_err(|e| format!("the answer could not be read: {e:?}"))?;
    let buffer = JsFuture::from(promise).await.map_err(|e| format!("the answer could not be read: {e:?}"))?;
    let bytes = Uint8Array::new(&buffer).to_vec();
    if !(200..300).contains(&status) {
        let detail = String::from_utf8_lossy(&bytes);
        let detail: String = detail.chars().take(200).collect();
        return Err(format!("the server answered {status}: {detail}"));
    }
    Ok(bytes)
}

fn post(url: &str, body: Option<Vec<u8>>) -> Result<Request, String> {
    let mut init = RequestInit::new();
    init.method("POST");
    if let Some(bytes) = body {
        let array = Uint8Array::from(bytes.as_slice());
        let value: JsValue = array.into();
        init.body(Some(&value));
    }
    Request::new_with_str_and_init(url, &init).map_err(|e| format!("{e:?}"))
}

/// Hand the document to the server and get a token for it back.
pub async fn upload(pdf: Vec<u8>) -> Result<(String, usize), String> {
    let bytes = send(post(&format!("{BASE}/upload"), Some(pdf))?).await?;
    let text = String::from_utf8_lossy(&bytes);
    let id = text.split("\"id\":").nth(1).and_then(|rest| rest.split(|c: char| c == '"' || c.is_whitespace()).nth(1)).unwrap_or_default().to_string();
    let pages = text
        .split("\"pages\":")
        .nth(1)
        .and_then(|rest| rest.split(|c: char| c == ',' || c == '}' || c.is_whitespace()).next())
        .and_then(|n| n.parse::<usize>().ok())
        .unwrap_or(0);
    if id.is_empty() {
        return Err("the server did not return a document id".into());
    }
    Ok((id, pages))
}

/// One page as a PNG, drawn by the server with an installed face, so a document whose fonts
/// are not embedded is still readable here.
pub async fn render_page(id: &str, page: usize, dpi: u32) -> Result<Vec<u8>, String> {
    let url = format!("{BASE}/render?id={id}&page={page}&dpi={dpi}");
    send(Request::new_with_str(&url).map_err(|e| format!("{e:?}"))?).await
}

/// The document with an invisible text layer over its pages: the server recognized it.
pub async fn ocr_pdf(id: &str) -> Result<Vec<u8>, String> {
    send(post(&format!("{BASE}/ocr?id={id}"), None)?).await
}

/// A copy of the document whose pages the server draws: a browser has no face for a font the
/// document does not embed, and the server has one (AGENTS.md §1.2, §1.4).
pub async fn display_copy(pdf: Vec<u8>) -> Result<Vec<u8>, String> {
    send(post(&format!("{BASE}/display"), Some(pdf))?).await
}
