//! Background translation service using Google Translate GTX endpoint.

#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;

/// Result of a translation attempt: (translated_text, detected_language_code).
pub type TranslationResult = Result<(String, String), String>;

#[cfg(not(target_arch = "wasm32"))]
fn os_roots() -> Result<ureq::tls::RootCerts, String> {
    let found = rustls_native_certs::load_native_certs();
    let certs: Vec<ureq::tls::Certificate<'static>> = found.certs.iter().map(|c| ureq::tls::Certificate::from_der(c.as_ref()).to_owned()).collect();
    if certs.is_empty() {
        return Err("no trusted certificates found on this system".into());
    }
    Ok(ureq::tls::RootCerts::new_with_certs(&certs))
}

/// Request translation for `query` from `sl` (source language, e.g. "auto")
/// into `tl` (target language, e.g. "tr").
#[cfg(not(target_arch = "wasm32"))]
pub fn fetch_translation(query: &str, sl: &str, tl: &str) -> TranslationResult {
    let encoded_query = urlencoding(query);
    let url = format!("https://translate.googleapis.com/translate_a/single?client=gtx&sl={sl}&tl={tl}&dt=t&q={encoded_query}");

    let mut builder = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(10)));

    if let Ok(roots) = os_roots() {
        builder = builder.tls_config(ureq::tls::TlsConfig::builder().root_certs(roots).build());
    }

    let agent = builder.build().new_agent();

    let mut resp = agent
        .get(&url)
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64)")
        .call()
        .map_err(|e| format!("Network request failed: {e}"))?;

    let body = resp.body_mut().read_to_string().map_err(|e| format!("Failed to read response: {e}"))?;

    parse_gtx_json(&body)
}

#[cfg(target_arch = "wasm32")]
pub fn fetch_translation(_query: &str, _sl: &str, _tl: &str) -> TranslationResult {
    Err("Translation is not supported in the web build".into())
}

/// Parse Google Translate GTX JSON format:
/// `[[["Translated text", "Original text", ...], ...], null, "detected_lang", ...]`
fn parse_gtx_json(json_str: &str) -> TranslationResult {
    let value: serde_json::Value = serde_json::from_str(json_str).map_err(|e| format!("Invalid JSON response: {e}"))?;

    let sentences = value.get(0).and_then(|v| v.as_array()).ok_or_else(|| "Unexpected response structure".to_string())?;

    let mut translated = String::new();
    for sentence in sentences {
        if let Some(part) = sentence.get(0).and_then(|p| p.as_str()) {
            translated.push_str(part);
        }
    }

    if translated.is_empty() {
        return Err("No translation returned".into());
    }

    let detected_lang = value.get(2).and_then(|v| v.as_str()).unwrap_or("auto").to_string();

    Ok((translated, detected_lang))
}

/// Percent-encode a URL query parameter string.
fn urlencoding(input: &str) -> String {
    let mut encoded = String::with_capacity(input.len() * 3);
    for byte in input.bytes() {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            b' ' => encoded.push_str("%20"),
            _ => {
                use std::fmt::Write;
                let _ = write!(encoded, "%{:02X}", byte);
            }
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_urlencoding() {
        assert_eq!(urlencoding("hello world"), "hello%20world");
        assert_eq!(urlencoding("pdf&craft=true"), "pdf%26craft%3Dtrue");
    }

    #[test]
    fn test_parse_gtx_json() {
        let sample = r#"[[["PdfCraft bir PDF çalışma tezgahıdır. ","PdfCraft is a PDF workbench. ",null,null,3],["Saf Rust ile yazılmıştır.","It is written in pure Rust.",null,null,3]],null,"en"]"#;
        let res = parse_gtx_json(sample);
        if let Ok((translated, detected)) = res {
            assert_eq!(translated, "PdfCraft bir PDF çalışma tezgahıdır. Saf Rust ile yazılmıştır.");
            assert_eq!(detected, "en");
        } else {
            assert!(false, "parse failed");
        }
    }
}
