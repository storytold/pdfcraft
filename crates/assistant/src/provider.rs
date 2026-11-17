//! Talking to the model server: Ollama's own API, or the OpenAI chat API that LM Studio,
//! llama.cpp's server, Ollama's `/v1` and others speak.
//!
//! The agent never reads proxy settings from the environment and never follows redirects, so a
//! request meant for this computer can't be carried to another one. Only models already on the
//! server are listed; nothing here can ask a server to download one.

use std::io::{BufRead, BufReader};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::{Value, json};

use crate::Settings;

/// Which API the server speaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Ollama,
    OpenAi,
    ClaudeCode,
    Codex,
}

impl Kind {
    pub fn is_cli(self) -> bool {
        matches!(self, Self::ClaudeCode | Self::Codex)
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Ollama => "Ollama",
            Self::OpenAi => "API externa (clave propia)",
            Self::ClaudeCode => "Claude Code",
            Self::Codex => "Codex · cuenta ChatGPT",
        }
    }
}

#[derive(Clone, PartialEq)]
pub struct Endpoint {
    pub kind: Kind,
    /// e.g. `http://127.0.0.1:11434` (Ollama) or `http://127.0.0.1:1234/v1`.
    pub base_url: String,
    pub model: String,
    /// Sent as a bearer token, for an external OpenAI-compatible provider. Never stored.
    pub api_key: Option<String>,
}

impl std::fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Endpoint")
            .field("kind", &self.kind)
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("api_key", &self.api_key.as_ref().map(|_| "[redacted]"))
            .finish()
    }
}

impl Default for Endpoint {
    fn default() -> Self {
        Endpoint { kind: Kind::Ollama, base_url: "http://127.0.0.1:11434".into(), model: String::new(), api_key: None }
    }
}

/// A model the server has.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelInfo {
    pub name: String,
    pub size_bytes: u64,
    /// Loaded in memory now (Ollama `/api/ps`).
    pub resident: bool,
    /// The model reasons before answering (it is asked not to, to answer sooner).
    pub thinking: bool,
}

/// Bytes of a reply read at most.
const MAX_REPLY: u64 = 8 << 20;

/// The host of an `http(s)://host[:port]/...` address. User names (`user@host`) are refused:
/// they can disguise where a request goes.
fn authority(url: &str) -> Result<(String, String), String> {
    if url.chars().any(|c| c.is_control() || c == '\\') || url.contains(['?', '#']) {
        return Err("The provider URL must not contain control characters, backslashes, a query or a fragment.".into());
    }
    let (scheme, rest) = url.trim().split_once("://").ok_or_else(|| format!("dirección no válida: {url}"))?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return Err(format!("sólo se admiten direcciones http o https, no {scheme}"));
    }
    let auth = rest.split(['/', '?', '#']).next().unwrap_or("");
    if auth.contains('@') {
        return Err("la dirección no puede incluir usuario ni contraseña".into());
    }
    let host = if let Some(stripped) = auth.strip_prefix('[') {
        stripped.split(']').next().unwrap_or("").to_owned()
    } else {
        auth.rsplit_once(':').map_or(auth, |(h, _)| h).to_owned()
    };
    if host.is_empty() {
        return Err(format!("dirección sin servidor: {url}"));
    }
    Ok((scheme, host.to_ascii_lowercase()))
}

pub fn host_of(url: &str) -> Option<String> {
    authority(url).ok().map(|(_, h)| h)
}

/// Whether `url` points at this computer (a loopback address or `localhost`).
pub fn is_local(url: &str) -> Result<bool, String> {
    let (_, host) = authority(url)?;
    if host == "localhost" {
        return Ok(true);
    }
    Ok(host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback()))
}

fn agent(timeout: Duration) -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .tls_config(ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build())
        .proxy(None)
        .max_redirects(0)
        .http_status_as_error(false)
        .timeout_connect(Some(Duration::from_secs(5)))
        .timeout_global(Some(timeout))
        .build();
    ureq::Agent::new_with_config(config)
}

fn join(base: &str, path: &str) -> String {
    format!("{}/{}", base.trim_end_matches('/'), path.trim_start_matches('/'))
}

/// The OpenAI API root: the base as given when it already ends in `/v1`.
fn openai_root(base: &str) -> String {
    let b = base.trim_end_matches('/');
    if b.ends_with("/v1") { b.to_owned() } else { format!("{b}/v1") }
}

/// OpenAI's hosted API needs a separate API key; a Codex sign-in is not an API credential.
pub fn missing_api_key(ep: &Endpoint) -> bool {
    ep.kind == Kind::OpenAi && host_of(&ep.base_url).as_deref() == Some("api.openai.com") && ep.api_key.as_deref().is_none_or(|k| k.trim().is_empty())
}

fn get(ep: &Endpoint, url: &str) -> Result<Value, String> {
    if missing_api_key(ep) {
        return Err(
            "Falta la clave API de OpenAI. Para usar su sesión de Codex sin una clave API, elija Codex (OpenAI). No se realizó ninguna solicitud."
                .into(),
        );
    }
    if !is_local(url)? && !url.starts_with("https://") {
        return Err("Un proveedor externo requiere HTTPS.".into());
    }
    let mut req = agent(Duration::from_secs(10)).get(url);
    if let Some(k) = ep.api_key.as_deref().filter(|k| !k.is_empty()) {
        req = req.header("Authorization", format!("Bearer {k}"));
    }
    let mut resp = req.call().map_err(|e| format!("no se pudo conectar con {url}: {e}"))?;
    let status = resp.status().as_u16();
    let body = resp.body_mut().with_config().limit(MAX_REPLY).read_to_string().map_err(|e| e.to_string())?;
    if status == 401 {
        return Err("El proveedor rechazó la autenticación (401). Revise la clave API; el inicio de sesión de Codex no autentica esta API.".into());
    }
    if !(200..300).contains(&status) {
        return Err(format!("{url} respondió {status}: {}", body.chars().take(300).collect::<String>()));
    }
    serde_json::from_str(&body).map_err(|e| format!("respuesta ilegible de {url}: {e}"))
}

/// Ollama can expose cloud models through a loopback server; those are not local inference.
pub(crate) fn local_ollama_model(model: &Value) -> bool {
    let name = model.get("name").and_then(Value::as_str).unwrap_or_default().to_ascii_lowercase();
    !name.ends_with(":cloud")
        && !name.ends_with("-cloud")
        && model.get("remote_host").is_none_or(Value::is_null)
        && model.get("remote_model").is_none_or(Value::is_null)
}

/// The models the server already has. Nothing is downloaded.
pub fn list_models(ep: &Endpoint) -> Result<Vec<ModelInfo>, String> {
    if ep.kind.is_cli() {
        return Ok(vec![ModelInfo { name: "Modelo predeterminado del programa".into(), size_bytes: 0, resident: false, thinking: false }]);
    }
    match ep.kind {
        Kind::ClaudeCode | Kind::Codex => Err("Proveedor CLI no disponible".into()),
        Kind::Ollama => {
            let tags = get(ep, &join(&ep.base_url, "api/tags"))?;
            let resident: Vec<String> = get(ep, &join(&ep.base_url, "api/ps"))
                .ok()
                .and_then(|ps| ps.get("models").and_then(Value::as_array).cloned())
                .unwrap_or_default()
                .iter()
                .filter_map(|m| m.get("name").and_then(Value::as_str).map(str::to_owned))
                .collect();
            let models = tags.get("models").and_then(Value::as_array).cloned().unwrap_or_default();
            Ok(models
                .iter()
                .filter(|m| local_ollama_model(m))
                .filter_map(|m| {
                    let name = m.get("name").and_then(Value::as_str)?.to_owned();
                    let caps: Vec<&str> =
                        m.get("capabilities").and_then(Value::as_array).map(|c| c.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
                    // Embedding-only models can't answer.
                    if !caps.is_empty() && !caps.contains(&"completion") {
                        return None;
                    }
                    Some(ModelInfo {
                        resident: resident.contains(&name),
                        size_bytes: m.get("size").and_then(Value::as_u64).unwrap_or(0),
                        thinking: caps.contains(&"thinking"),
                        name,
                    })
                })
                .collect())
        }
        Kind::OpenAi => {
            let v = get(ep, &join(&openai_root(&ep.base_url), "models"))?;
            Ok(v.get("data")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
                .iter()
                .filter_map(|m| m.get("id").and_then(Value::as_str))
                .map(|name| ModelInfo { name: name.to_owned(), size_bytes: 0, resident: false, thinking: false })
                .collect())
        }
    }
}

/// Send one request and return the model's reply text. The reply is streamed and `cancel` is
/// checked at every fragment: cancelling closes the connection, which makes Ollama (and the other
/// servers) stop generating, so the model and the shared lock are freed at once.
pub fn chat(ep: &Endpoint, system: &str, user: &str, schema: &Value, settings: &Settings, cancel: &AtomicBool) -> Result<String, String> {
    if ep.kind.is_cli() {
        return crate::cli::chat(ep, system, user, schema, cancel, settings.request_timeout);
    }
    if ep.model.trim().is_empty() {
        return Err("Elija un modelo instalado.".into());
    }
    if !is_local(&ep.base_url)? && !ep.base_url.trim().starts_with("https://") {
        return Err("Un proveedor externo requiere HTTPS para proteger el texto y la clave.".into());
    }
    // The model must already be there: a missing one is an error, never a download.
    let models = list_models(ep)?;
    let Some(info) = models.iter().find(|m| m.name == ep.model) else {
        return Err(format!("El modelo {} no está instalado en este servidor. No se descarga ningún modelo: elija uno de la lista.", ep.model));
    };
    let messages = json!([{ "role": "system", "content": system }, { "role": "user", "content": user }]);
    let (url, mut body) = match ep.kind {
        Kind::ClaudeCode | Kind::Codex => return Err("Proveedor CLI no disponible".into()),
        Kind::Ollama => (
            join(&ep.base_url, "api/chat"),
            json!({
                "model": ep.model,
                "messages": messages,
                "stream": true,
                "format": schema,
                "keep_alive": settings.keep_alive,
                "options": { "temperature": 0.1, "num_ctx": settings.num_ctx }
            }),
        ),
        Kind::OpenAi => (
            join(&openai_root(&ep.base_url), "chat/completions"),
            json!({ "model": ep.model, "messages": messages, "temperature": 0.1, "stream": true, "response_format": { "type": "json_object" } }),
        ),
    };
    if ep.kind == Kind::Ollama && info.thinking {
        body["think"] = json!(false);
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(CANCELLED.into());
    }
    let mut out = String::new();
    // One streamed line of the reply: Ollama's JSON lines, or OpenAI's `data:` events. Returns
    // whether more is expected.
    let kind = ep.kind;
    let mut on_line = |line: &str| -> Result<bool, String> {
        let line = line.trim();
        let data = match kind {
            Kind::ClaudeCode | Kind::Codex => return Err("Proveedor CLI no disponible".into()),
            Kind::Ollama => line,
            Kind::OpenAi => match line.strip_prefix("data:") {
                Some(d) => d.trim(),
                None => return Ok(true),
            },
        };
        if data.is_empty() {
            return Ok(true);
        }
        if data == "[DONE]" {
            return Ok(false);
        }
        let v: Value = serde_json::from_str(data).map_err(|e| format!("respuesta ilegible: {e}"))?;
        if let Some(e) = v.get("error") {
            return Err(format!("el servidor informó un error: {e}"));
        }
        let piece = match kind {
            Kind::ClaudeCode | Kind::Codex => None,
            Kind::Ollama => v.pointer("/message/content"),
            Kind::OpenAi => v.pointer("/choices/0/delta/content"),
        };
        if let Some(p) = piece.and_then(Value::as_str) {
            out.push_str(p);
        }
        Ok(!(kind == Kind::Ollama && v.get("done").and_then(Value::as_bool) == Some(true)))
    };
    let plain_local = url.starts_with("http://") && is_local(&url) == Ok(true);
    if plain_local && ep.api_key.as_deref().is_none_or(str::is_empty) {
        // On this computer: a client that closes the connection as soon as the person cancels,
        // even while the model is still loading.
        crate::local_http::post_lines(&url, &body.to_string(), ep.api_key.as_deref(), settings.request_timeout, MAX_REPLY, cancel, &mut on_line)?;
    } else {
        let mut req = agent(settings.request_timeout).post(&url).header("Content-Type", "application/json");
        if let Some(k) = ep.api_key.as_deref().filter(|k| !k.is_empty()) {
            req = req.header("Authorization", format!("Bearer {k}"));
        }
        let mut resp = req.send(body.to_string()).map_err(|e| format!("el modelo no respondió: {e}"))?;
        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            let text = resp.body_mut().with_config().limit(MAX_REPLY).read_to_string().unwrap_or_default();
            return Err(format!("el servidor respondió {status}: {}", text.chars().take(400).collect::<String>()));
        }
        let reader = BufReader::new(resp.body_mut().with_config().limit(MAX_REPLY).reader());
        for line in reader.lines() {
            // Returning drops the response, which closes the connection: the server stops.
            if cancel.load(Ordering::Relaxed) {
                return Err(CANCELLED.into());
            }
            if !on_line(&line.map_err(|e| format!("se cortó la respuesta: {e}"))?)? {
                break;
            }
        }
    }
    if out.trim().is_empty() {
        return Err("la respuesta no trae texto".into());
    }
    Ok(out)
}

/// What a cancelled request returns.
pub const CANCELLED: &str = crate::local_http::CANCELLED;
