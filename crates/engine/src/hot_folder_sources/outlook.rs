//! Microsoft Graph delegated device-code flow. Tokens/device secrets stay in the selected protected store.
//! Global Microsoft cloud endpoints are fixed; pagination cannot redirect credentials.

use super::*;
use serde_json::{Value, json};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

const GRAPH: &str = "https://graph.microsoft.com/v1.0";
const LOGIN: &str = "https://login.microsoftonline.com";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeviceSignIn {
    pub flow_id: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_at: u64,
    pub interval_seconds: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SignInStatus {
    Pending { retry_after_seconds: u64 },
    Authorized,
    Expired,
    Declined,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MailFolder {
    pub id: String,
    pub name: String,
    pub child_count: u64,
}

#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
struct OAuthSecret {
    binding: String,
    access_token: String,
    refresh_token: String,
    expires_at: u64,
    account_id: String,
    scope: String,
}
#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
struct DeviceSecret {
    binding: String,
    device_code: String,
    expires_at: u64,
    next_poll: u64,
    interval: u64,
}

pub(super) fn validate(source: &OutlookSource) -> Result<(), String> {
    credentials::valid_id(&source.credential_id)?;
    if source.client_id.len() != 36
        || !source.client_id.bytes().enumerate().all(|(index, c)| if matches!(index, 8 | 13 | 18 | 23) { c == b'-' } else { c.is_ascii_hexdigit() })
    {
        return Err("Outlook needs your own Microsoft Entra Application (client) ID in GUID form; enable public-client/device-code flow".into());
    }
    if source.tenant.is_empty() || source.tenant.len() > 253 || !source.tenant.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'.'))
    {
        return Err("Outlook tenant must be common, consumers, organizations, a tenant GUID or tenant domain".into());
    }
    valid_graph_id(&source.folder_id)?;
    if let SourceAction::Move { destination } = &source.action {
        valid_graph_id(destination)?;
        if destination == &source.folder_id {
            return Err("Outlook archive folder must differ from the watched mail folder".into());
        }
    }
    Ok(())
}
fn valid_graph_id(id: &str) -> Result<(), String> {
    if id.is_empty() || id.len() > 2048 || matches!(id, "." | "..") || id.chars().any(char::is_control) {
        return Err("Outlook resource ID must be 1..2048 characters without control characters".into());
    }
    Ok(())
}
fn scope(source: &OutlookSource) -> &'static str {
    if matches!(source.action, SourceAction::Keep) { "offline_access User.Read Mail.Read" } else { "offline_access User.Read Mail.ReadWrite" }
}
fn binding(source: &OutlookSource) -> String {
    hash(format!("{}\n{}\n{}", source.client_id, source.tenant, scope(source)).as_bytes())
}
fn auth_url(source: &OutlookSource, endpoint: &str) -> String {
    format!("{LOGIN}/{}/oauth2/v2.0/{endpoint}", source.tenant)
}
fn form(fields: &[(&str, &str)]) -> Zeroizing<String> {
    Zeroizing::new(url::form_urlencoded::Serializer::new(String::new()).extend_pairs(fields.iter().copied()).finish())
}
fn graph_url(parts: &[&str]) -> Result<String, String> {
    let mut url = url::Url::parse(GRAPH).map_err(|_| "invalid Graph base URL")?;
    {
        let mut segments = url.path_segments_mut().map_err(|_| "invalid Graph base path")?;
        for part in parts {
            valid_graph_id(part)?;
            segments.push(part);
        }
    }
    Ok(url.into())
}
fn graph_url_ok(value: &str) -> Result<(), String> {
    let url = url::Url::parse(value).map_err(|_| "invalid Outlook pagination URL")?;
    if url.scheme() != "https"
        || url.host_str() != Some("graph.microsoft.com")
        || url.port().is_some_and(|port| port != 443)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || !url.path().starts_with("/v1.0/me/")
    {
        return Err("Outlook pagination URL left the permitted Microsoft Graph mailbox endpoint".into());
    }
    Ok(())
}

struct Response {
    status: u16,
    body: Zeroizing<Vec<u8>>,
    retry_after: u64,
}
impl Response {
    fn json(&self) -> Result<Value, String> {
        serde_json::from_slice(&self.body).map_err(|_| "Microsoft returned malformed or oversized JSON".into())
    }
    fn success(&self) -> Result<(), String> {
        match self.status {
            200..=299 => Ok(()),
            401 => Err("Outlook authorization expired or was revoked; sign in again".into()),
            403 => Err("Outlook permission denied; sign in with the required mail scope or check the organization's policy".into()),
            429 => Err(format!("Microsoft Graph throttled this source; retry after at least {} seconds", self.retry_after.max(1))),
            404 => Err("Outlook folder, message or attachment was not found".into()),
            500..=599 => Err("Microsoft Graph is temporarily unavailable; retry later".into()),
            _ => Err(format!("Microsoft request failed (HTTP {}); originals retained", self.status)),
        }
    }
}

trait Http {
    #[allow(clippy::too_many_arguments)]
    fn request(
        &self,
        method: &str,
        url: &str,
        bearer: Option<&str>,
        body: Option<&str>,
        is_form: bool,
        limit: u64,
        control: &Control<'_>,
    ) -> Result<Response, String>;
}
struct Network(ureq::Agent);
impl Network {
    fn new() -> Result<Self, String> {
        let certs = rustls_native_certs::load_native_certs().certs;
        let certs = certs.iter().map(|cert| ureq::tls::Certificate::from_der(cert.as_ref()).to_owned()).collect::<Vec<_>>();
        if certs.is_empty() {
            return Err("no trusted OS certificate authorities are available for Outlook".into());
        }
        let agent = ureq::Agent::config_builder()
            .https_only(true)
            .max_redirects(0)
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(45)))
            .tls_config(ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::new_with_certs(&certs)).build())
            .build()
            .new_agent();
        Ok(Self(agent))
    }
}
/// Graph calls carry the bearer and Graph's ID preference. Sign-in and token refresh have no
/// bearer and get neither, rather than an empty Authorization header.
fn graph_headers<B>(request: ureq::RequestBuilder<B>, auth: Option<&str>) -> ureq::RequestBuilder<B> {
    match auth {
        Some(auth) => request.header("Authorization", auth).header("Prefer", "IdType=\"ImmutableId\""),
        None => request,
    }
}

impl Http for Network {
    fn request(
        &self,
        method: &str,
        url: &str,
        bearer: Option<&str>,
        body: Option<&str>,
        is_form: bool,
        limit: u64,
        control: &Control<'_>,
    ) -> Result<Response, String> {
        control.check()?;
        let auth = bearer.map(|value| Zeroizing::new(format!("Bearer {value}")));
        let auth = auth.as_ref().map(|value| value.as_str());
        let mut response = match method {
            "GET" => graph_headers(self.0.get(url), auth).call(),
            "DELETE" => graph_headers(self.0.delete(url), auth).call(),
            "POST" => graph_headers(self.0.post(url), auth)
                .header("Content-Type", if is_form { "application/x-www-form-urlencoded" } else { "application/json" })
                .send(body.unwrap_or_default()),
            _ => return Err("unsupported Outlook request method".into()),
        }
        .map_err(|_| "Microsoft connection failed, timed out or failed TLS verification; check the network and retry".to_owned())?;
        let status = response.status().as_u16();
        let retry_after =
            response.headers().get("Retry-After").and_then(|value| value.to_str().ok()).and_then(|value| value.parse().ok()).unwrap_or(60);
        let mut bytes = Zeroizing::new(Vec::new());
        copy_limited(&mut response.body_mut().as_reader(), &mut *bytes, limit, control)?;
        Ok(Response { status, body: bytes, retry_after })
    }
}

pub fn outlook_begin_sign_in(store: &CredentialStore, source: &OutlookSource) -> Result<DeviceSignIn, String> {
    guarded(|| {
        validate(source)?;
        let control = Control::new(&|| false);
        let network = Network::new()?;
        let body = form(&[("client_id", &source.client_id), ("scope", scope(source))]);
        let response = network.request("POST", &auth_url(source, "devicecode"), None, Some(&body), true, MAX_JSON, &control)?;
        response.success()?;
        let data = response.json()?;
        let (public, secret) = parse_device(source, &data, crate::hot_folder::now())?;
        let encoded = Zeroizing::new(serde_json::to_vec(&secret).map_err(|_| "cannot encode sign-in state")?);
        credentials::put(store, &public.flow_id, &encoded)?;
        Ok(public)
    })
}
fn parse_device(source: &OutlookSource, data: &Value, now: u64) -> Result<(DeviceSignIn, DeviceSecret), String> {
    let device_code = required(data, "device_code")?;
    let user_code = required(data, "user_code")?;
    let verification_uri = required(data, "verification_uri")?;
    let url = url::Url::parse(&verification_uri).map_err(|_| "invalid Microsoft verification URL")?;
    if url.scheme() != "https"
        || !matches!(url.host_str(), Some("microsoft.com" | "www.microsoft.com" | "login.microsoftonline.com"))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some_and(|port| port != 443)
        || url.fragment().is_some()
    {
        return Err("Microsoft sign-in returned an unexpected verification URL".into());
    }
    let expires_at = now.saturating_add(
        data.get("expires_in").and_then(Value::as_u64).filter(|seconds| *seconds > 0 && *seconds <= 3600).ok_or("invalid sign-in expiry")?,
    );
    let interval = data.get("interval").and_then(Value::as_u64).unwrap_or(5).clamp(5, 60);
    let flow_id = format!("flow-{}", hash(device_code.as_bytes()));
    Ok((
        DeviceSignIn { flow_id, user_code, verification_uri, expires_at, interval_seconds: interval },
        DeviceSecret { binding: binding(source), device_code, expires_at, next_poll: now.saturating_add(interval), interval },
    ))
}

pub fn outlook_poll_sign_in(store: &CredentialStore, source: &OutlookSource, sign_in: &DeviceSignIn) -> Result<SignInStatus, String> {
    guarded(|| {
        validate(source)?;
        let bytes = credentials::get(store, &sign_in.flow_id)?.ok_or("this sign-in is missing or already finished; begin a new sign-in")?;
        let mut secret: DeviceSecret = serde_json::from_slice(&bytes).map_err(|_| "invalid sign-in state")?;
        let network = Network::new()?;
        let control = Control::new(&|| false);
        match poll_device(&network, source, &mut secret, crate::hot_folder::now(), &control)? {
            DevicePoll::Authorized(token) => {
                store_token(store, source, &token)?;
                credentials::forget_credentials(store, &sign_in.flow_id)?;
                Ok(SignInStatus::Authorized)
            }
            DevicePoll::Finished(status) => {
                credentials::forget_credentials(store, &sign_in.flow_id)?;
                Ok(status)
            }
            DevicePoll::Pending(seconds) => {
                credentials::put(store, &sign_in.flow_id, &Zeroizing::new(serde_json::to_vec(&secret).map_err(|_| "cannot encode sign-in state")?))?;
                Ok(SignInStatus::Pending { retry_after_seconds: seconds })
            }
        }
    })
}

enum DevicePoll {
    Pending(u64),
    Authorized(OAuthSecret),
    Finished(SignInStatus),
}
fn poll_device(network: &dyn Http, source: &OutlookSource, secret: &mut DeviceSecret, now: u64, control: &Control<'_>) -> Result<DevicePoll, String> {
    control.check()?;
    if secret.binding != binding(source) {
        return Err("sign-in belongs to another Outlook application or permission scope".into());
    }
    if now >= secret.expires_at {
        return Ok(DevicePoll::Finished(SignInStatus::Expired));
    }
    if now < secret.next_poll {
        return Ok(DevicePoll::Pending(secret.next_poll - now));
    }
    let body = form(&[
        ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ("client_id", &source.client_id),
        ("device_code", &secret.device_code),
    ]);
    let response = network.request("POST", &auth_url(source, "token"), None, Some(&body), true, MAX_JSON, control)?;
    let data = response.json()?;
    if response.status == 200 {
        let mut token = parse_token(source, &data, None, now)?;
        token.account_id = account_id(network, &token.access_token, control)?;
        return Ok(DevicePoll::Authorized(token));
    }
    match data.get("error").and_then(Value::as_str).unwrap_or_default() {
        "authorization_pending" => {}
        "slow_down" => secret.interval = secret.interval.saturating_add(5).min(300),
        "authorization_declined" | "access_denied" => return Ok(DevicePoll::Finished(SignInStatus::Declined)),
        "expired_token" => return Ok(DevicePoll::Finished(SignInStatus::Expired)),
        _ => {
            response.success()?;
            return Err("Microsoft sign-in could not be completed; begin a new sign-in".into());
        }
    }
    secret.next_poll = now.saturating_add(secret.interval);
    Ok(DevicePoll::Pending(secret.interval))
}

fn required(value: &Value, key: &str) -> Result<String, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && value.len() <= 32 * 1024)
        .map(str::to_owned)
        .ok_or_else(|| format!("Microsoft response is missing {key}"))
}
fn parse_token(source: &OutlookSource, data: &Value, old: Option<&OAuthSecret>, now: u64) -> Result<OAuthSecret, String> {
    if data.get("token_type").and_then(Value::as_str).is_none_or(|value| !value.eq_ignore_ascii_case("bearer")) {
        return Err("Microsoft returned an unsupported token type".into());
    }
    let refresh_token = data
        .get("refresh_token")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| old.map(|old| old.refresh_token.clone()))
        .ok_or("Microsoft did not grant offline access; sign in with offline_access")?;
    let scopes = data.get("scope").and_then(Value::as_str).unwrap_or_default().to_owned();
    let write = !matches!(source.action, SourceAction::Keep);
    let has_read_write = scopes
        .split_whitespace()
        .any(|scope| scope.eq_ignore_ascii_case("Mail.ReadWrite") || scope.eq_ignore_ascii_case("https://graph.microsoft.com/Mail.ReadWrite"));
    let has_read = scopes
        .split_whitespace()
        .any(|scope| scope.eq_ignore_ascii_case("Mail.Read") || scope.eq_ignore_ascii_case("https://graph.microsoft.com/Mail.Read"));
    if !has_read_write && (write || !has_read) {
        return Err("Microsoft did not grant the required mail permission; check app consent and sign in again".into());
    }
    let expires =
        data.get("expires_in").and_then(Value::as_u64).filter(|value| *value > 0 && *value <= 86400).ok_or("invalid Microsoft token expiry")?;
    Ok(OAuthSecret {
        binding: binding(source),
        access_token: required(data, "access_token")?,
        refresh_token,
        expires_at: now.saturating_add(expires),
        account_id: old.map(|value| value.account_id.clone()).unwrap_or_default(),
        scope: scopes,
    })
}
fn store_token(store: &CredentialStore, source: &OutlookSource, token: &OAuthSecret) -> Result<(), String> {
    let bytes = Zeroizing::new(serde_json::to_vec(token).map_err(|_| "cannot encode Outlook credentials")?);
    credentials::put(store, &source.credential_id, &bytes)
}
fn account_id(network: &dyn Http, token: &str, control: &Control<'_>) -> Result<String, String> {
    let response = network.request("GET", &format!("{GRAPH}/me?$select=id"), Some(token), None, false, MAX_JSON, control)?;
    response.success()?;
    required(&response.json()?, "id")
}

fn refresh_token(network: &dyn Http, source: &OutlookSource, old: &OAuthSecret, control: &Control<'_>) -> Result<OAuthSecret, String> {
    let body =
        form(&[("grant_type", "refresh_token"), ("client_id", &source.client_id), ("refresh_token", &old.refresh_token), ("scope", scope(source))]);
    let response = network.request("POST", &auth_url(source, "token"), None, Some(&body), true, MAX_JSON, control)?;
    if response.status != 200 {
        return Err("Outlook refresh was rejected or unavailable; sign in again if retry does not recover".into());
    }
    let mut token = parse_token(source, &response.json()?, Some(old), crate::hot_folder::now())?;
    let account = account_id(network, &token.access_token, control)?;
    if account != old.account_id {
        return Err("Outlook refresh resolved to another account; original retained".into());
    }
    token.account_id = account;
    Ok(token)
}

pub(super) struct Client<'a> {
    store: &'a CredentialStore,
    source: &'a OutlookSource,
    network: Box<dyn Http>,
    token: OAuthSecret,
}
impl<'a> Client<'a> {
    pub(super) fn connect(store: &'a CredentialStore, source: &'a OutlookSource) -> Result<Self, String> {
        validate(source)?;
        let bytes = credentials::get(store, &source.credential_id)?.ok_or("Outlook is not connected; start sign-in explicitly")?;
        let token: OAuthSecret = serde_json::from_slice(&bytes).map_err(|_| "credential ID does not contain Outlook credentials")?;
        if token.binding != binding(source) {
            return Err("Outlook application or permissions changed; sign in again".into());
        }
        Ok(Self { store, source, network: Box::new(Network::new()?), token })
    }
    fn refresh(&mut self, control: &Control<'_>) -> Result<(), String> {
        let token = refresh_token(self.network.as_ref(), self.source, &self.token, control)?;
        store_token(self.store, self.source, &token)?;
        self.token = token;
        Ok(())
    }
    fn request(&mut self, method: &str, url: &str, body: Option<&str>, limit: u64, control: &Control<'_>) -> Result<Response, String> {
        graph_url_ok(url)?;
        if self.token.expires_at <= crate::hot_folder::now().saturating_add(60) {
            self.refresh(control)?;
        }
        let response = self.network.request(method, url, Some(&self.token.access_token), body, false, limit, control)?;
        // Retry only reads: an uncertain successful POST/DELETE must be reconciled first.
        if response.status == 401 && method == "GET" {
            self.refresh(control)?;
            return self.network.request(method, url, Some(&self.token.access_token), body, false, limit, control);
        }
        Ok(response)
    }
    fn collection(&mut self, url: String, control: &Control<'_>) -> Result<Vec<Value>, String> {
        let mut next = Some(url);
        let mut seen = std::collections::BTreeSet::new();
        let mut output = Vec::new();
        while let Some(url) = next.take() {
            control.check()?;
            if seen.len() >= 200 || !seen.insert(url.clone()) {
                return Err("Outlook pagination exceeded its limit or repeated a page".into());
            }
            let response = self.request("GET", &url, None, MAX_JSON, control)?;
            response.success()?;
            let data = response.json()?;
            let values = data.get("value").and_then(Value::as_array).ok_or("Outlook collection response has no value array")?;
            if output.len().saturating_add(values.len()) > MAX_ITEMS {
                return Err("Outlook source exceeds 10000 items; choose a smaller mail folder".into());
            }
            output.extend(values.iter().cloned());
            next = data.get("@odata.nextLink").and_then(Value::as_str).map(str::to_owned);
        }
        Ok(output)
    }
    fn attachments(&mut self, message: &str, control: &Control<'_>) -> Result<Vec<RemoteFile>, String> {
        let url = format!("{}?$select=id,name,size,isInline,lastModifiedDateTime&$top=100", graph_url(&["me", "messages", message, "attachments"])?);
        let values = self.collection(url, control)?;
        let mut result = Vec::new();
        for value in values {
            if value.get("@odata.type").and_then(Value::as_str) != Some("#microsoft.graph.fileAttachment")
                || value.get("isInline").and_then(Value::as_bool) == Some(true)
            {
                continue;
            }
            let name = required(&value, "name")?;
            let relative = PathBuf::from(hash(message.as_bytes()).get(..16).ok_or("invalid message ID hash")?).join(safe_filename(&name));
            if !supported(&relative) {
                continue;
            }
            let id = required(&value, "id")?;
            let modified = required(&value, "lastModifiedDateTime")?;
            let size = value
                .get("size")
                .and_then(Value::as_u64)
                .filter(|size| *size > 0 && *size <= MAX_FILE)
                .ok_or("Outlook attachment is empty or exceeds 512 MiB")?;
            result.push(RemoteFile {
                key: id,
                relative,
                revision: format!("{}:{modified}", self.token.account_id),
                size,
                group: Some(message.into()),
            });
        }
        result.sort_by(|left, right| left.key.cmp(&right.key));
        if result.len() > MAX_BATCH || result.iter().fold(0_u64, |sum, file| sum.saturating_add(file.size)) > MAX_FILE {
            return Err("one Outlook message exceeds the 128-attachment / 512 MiB batch limit; retain the mail and split its attachments".into());
        }
        Ok(result)
    }
    fn download(&mut self, file: &RemoteFile, control: &Control<'_>) -> Result<Response, String> {
        let message = file.group.as_deref().ok_or("Outlook receipt is missing its message ID")?;
        let url = graph_url(&["me", "messages", message, "attachments", &file.key, "$value"])?;
        let response = self.request("GET", &url, None, file.size, control)?;
        response.success()?;
        if response.body.len() as u64 != file.size {
            return Err("Outlook attachment changed size during download".into());
        }
        Ok(response)
    }
}

pub fn outlook_folders(
    store: &CredentialStore,
    source: &OutlookSource,
    parent: Option<&str>,
    should_stop: &dyn Fn() -> bool,
) -> Result<Vec<MailFolder>, String> {
    guarded(|| {
        let mut client = Client::connect(store, source)?;
        let control = Control::new(should_stop);
        let url =
            if let Some(parent) = parent { graph_url(&["me", "mailFolders", parent, "childFolders"])? } else { graph_url(&["me", "mailFolders"])? };
        let values = client.collection(format!("{url}?$select=id,displayName,childFolderCount&$top=100"), &control)?;
        values
            .iter()
            .map(|value| {
                Ok(MailFolder {
                    id: required(value, "id")?,
                    name: required(value, "displayName")?,
                    child_count: value.get("childFolderCount").and_then(Value::as_u64).unwrap_or(0),
                })
            })
            .collect()
    })
}

impl Backend for Client<'_> {
    fn list(&mut self, control: &Control<'_>) -> Result<Vec<RemoteFile>, String> {
        let url = format!(
            "{}?$select=id,hasAttachments&$filter=hasAttachments%20eq%20true&$top=100",
            graph_url(&["me", "mailFolders", &self.source.folder_id, "messages"])?
        );
        let messages = self.collection(url, control)?;
        let mut files = Vec::new();
        for message in messages {
            files.extend(self.attachments(&required(&message, "id")?, control)?);
            if files.len() > MAX_ITEMS {
                return Err("Outlook source exceeds 10000 attachments; choose a smaller folder".into());
            }
        }
        Ok(files)
    }
    fn read(&mut self, file: &RemoteFile, writer: &mut dyn Write, control: &Control<'_>) -> Result<(), String> {
        let message = file.group.as_deref().ok_or("Outlook receipt is missing its message ID")?;
        let current = self.attachments(message, control)?;
        if !current.iter().any(|current| current.key == file.key && current.revision == file.revision && current.size == file.size) {
            return Err("Outlook attachment changed before download; retry after it settles".into());
        }
        let response = self.download(file, control)?;
        writer.write_all(&response.body).map_err(io_error)
    }
    fn finish(&mut self, records: &[Record], action: &SourceAction, control: &Control<'_>) -> Result<(), String> {
        let first = records.first().ok_or("empty Outlook acknowledgement")?;
        let message = first.remote.group.as_deref().ok_or("missing Outlook message ID")?;
        let url = format!("{}?$select=id,parentFolderId", graph_url(&["me", "messages", message])?);
        let response = self.request("GET", &url, None, MAX_JSON, control)?;
        if response.status == 404 && matches!(action, SourceAction::Delete) {
            return Ok(());
        }
        response.success()?;
        let message_data = response.json()?;
        let destination = match action {
            SourceAction::Move { destination } => Some(destination.as_str()),
            SourceAction::Delete => Some("deleteditems"),
            SourceAction::Keep => None,
        };
        if let Some(destination) = destination {
            let folder_url = format!("{}?$select=id", graph_url(&["me", "mailFolders", destination])?);
            let destination_response = self.request("GET", &folder_url, None, MAX_JSON, control)?;
            destination_response.success()?;
            if required(&message_data, "parentFolderId")? == required(&destination_response.json()?, "id")? {
                return Ok(());
            }
        }
        let watched = format!("{}?$select=id", graph_url(&["me", "mailFolders", &self.source.folder_id])?);
        let watched = self.request("GET", &watched, None, MAX_JSON, control)?;
        watched.success()?;
        if required(&message_data, "parentFolderId")? != required(&watched.json()?, "id")? {
            return Err("Outlook original left the watched folder; it was not moved or deleted".into());
        }
        let current = self.attachments(message, control)?;
        if current.len() != records.len()
            || !current.iter().all(|file| {
                records.iter().any(|record| {
                    record.ready && record.remote.key == file.key && record.remote.revision == file.revision && record.remote.size == file.size
                })
            })
        {
            return Err("Outlook message has unprocessed or changed attachments; the whole message was retained".into());
        }
        for record in records {
            if hash(&self.download(&record.remote, control)?.body) != record.source.sha256 {
                return Err("Outlook attachment content changed; the whole message was retained".into());
            }
        }
        match action {
            SourceAction::Keep => Ok(()),
            SourceAction::Delete | SourceAction::Move { .. } => {
                // "Delete" is a recoverable move to Deleted Items, not permanent deletion;
                // an interrupted retry cannot delete the same mail a second time.
                let destination = destination.ok_or("missing Outlook archive destination")?;
                let body = json!({ "destinationId": destination }).to_string();
                self.request("POST", &graph_url(&["me", "messages", message, "move"])?, Some(&body), MAX_JSON, control)?.success()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};
    struct FakeHttp {
        replies: RefCell<VecDeque<Response>>,
        calls: Arc<Mutex<Vec<(String, String)>>>,
    }
    impl Http for FakeHttp {
        fn request(
            &self,
            method: &str,
            url: &str,
            _: Option<&str>,
            body: Option<&str>,
            _: bool,
            _: u64,
            control: &Control<'_>,
        ) -> Result<Response, String> {
            control.check()?;
            self.calls.lock().unwrap().push((format!("{method} {url}"), body.unwrap_or_default().into()));
            Ok(self.replies.borrow_mut().pop_front().expect("unexpected HTTP request"))
        }
    }
    fn reply(value: Value) -> Response {
        Response { status: 200, body: Zeroizing::new(serde_json::to_vec(&value).unwrap()), retry_after: 0 }
    }
    fn attachments() -> Value {
        json!({"value":[
            {"@odata.type":"#microsoft.graph.fileAttachment","id":"a","name":"one.pdf","size":3,"lastModifiedDateTime":"2026-10-11T00:00:00Z"},
            {"@odata.type":"#microsoft.graph.fileAttachment","id":"b","name":"two.png","size":3,"lastModifiedDateTime":"2026-10-11T00:00:00Z"},
            {"@odata.type":"#microsoft.graph.referenceAttachment","id":"c","name":"cloud.pdf","size":3},
            {"@odata.type":"#microsoft.graph.fileAttachment","id":"d","name":"signature.png","size":3,"isInline":true}
        ]})
    }
    fn client<'a>(source: &'a OutlookSource, replies: Vec<Response>, calls: Arc<Mutex<Vec<(String, String)>>>) -> Client<'a> {
        Client {
            source,
            store: &CredentialStore::OsVault,
            network: Box::new(FakeHttp { replies: RefCell::new(replies.into()), calls }),
            token: OAuthSecret {
                binding: binding(source),
                access_token: "test-only-access".into(),
                refresh_token: "test-only-refresh".into(),
                account_id: "account".into(),
                scope: "Mail.ReadWrite".into(),
                expires_at: crate::hot_folder::now().saturating_add(3600),
            },
        }
    }
    fn records() -> Vec<Record> {
        ["a", "b"]
            .iter()
            .map(|id| Record {
                source: StagedSource {
                    source_id: (*id).into(),
                    relative: format!("{id}.pdf").into(),
                    staged_relative: format!("stage-{id}.pdf").into(),
                    sha256: hash(b"abc"),
                },
                remote: RemoteFile {
                    key: (*id).into(),
                    relative: format!("{id}.pdf").into(),
                    revision: "account:2026-10-11T00:00:00Z".into(),
                    size: 3,
                    group: Some("immutable-message".into()),
                },
                ready: true,
                acknowledged: false,
            })
            .collect()
    }
    fn before_finish() -> Vec<Response> {
        vec![reply(json!({"parentFolderId":"inbox-id"})), reply(json!({"id":"deleted-id"})), reply(json!({"id":"inbox-id"})), reply(attachments())]
    }
    #[test]
    fn outlook_keeps_whole_message_until_every_supported_attachment_is_ready() {
        let source = OutlookSource { action: SourceAction::Delete, ..source() };
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut client = client(&source, before_finish(), calls.clone());
        let mut records = records();
        records.pop();
        assert!(client.finish(&records, &SourceAction::Delete, &Control::new(&|| false)).is_err());
        assert!(calls.lock().unwrap().iter().all(|(method, _)| method.starts_with("GET ")));
    }
    #[test]
    fn outlook_checks_each_attachment_then_moves_to_deleted_items_and_reconciles_retry() {
        let source = OutlookSource { action: SourceAction::Delete, ..source() };
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut responses = before_finish();
        for _ in 0..2 {
            responses.push(Response { status: 200, body: Zeroizing::new(b"abc".to_vec()), retry_after: 0 });
        }
        responses.push(reply(json!({"id":"immutable-message","parentFolderId":"deleted-id"})));
        responses.push(reply(json!({"id":"immutable-message","parentFolderId":"deleted-id"})));
        responses.push(reply(json!({"id":"deleted-id"})));
        let mut client = client(&source, responses, calls.clone());
        client.finish(&records(), &SourceAction::Delete, &Control::new(&|| false)).unwrap();
        client.finish(&records(), &SourceAction::Delete, &Control::new(&|| false)).unwrap();
        let calls = calls.lock().unwrap();
        assert_eq!(calls.iter().filter(|(method, _)| method.starts_with("POST ")).count(), 1);
        assert_eq!(calls.iter().find(|(method, _)| method.starts_with("POST ")).unwrap().1, "{\"destinationId\":\"deleteditems\"}");
        assert!(calls.iter().all(|(method, _)| !method.starts_with("DELETE ")));
    }
    #[test]
    fn outlook_pagination_escapes_and_http_failures_never_echo_response_secrets() {
        let source = source();
        let mut client =
            client(&source, vec![reply(json!({"value":[],"@odata.nextLink":"https://attacker.test/token"}))], Arc::new(Mutex::new(Vec::new())));
        assert!(client.collection(format!("{GRAPH}/me/mailFolders"), &Control::new(&|| false)).is_err());
        let response = Response { status: 403, body: Zeroizing::new(b"response containing secret NEVER-ECHO".to_vec()), retry_after: 0 };
        assert!(!response.success().unwrap_err().contains("NEVER-ECHO"));
    }
    fn source() -> OutlookSource {
        OutlookSource { client_id: "01234567-89ab-cdef-0123-456789abcdef".into(), credential_id: "test".into(), ..Default::default() }
    }
    #[test]
    fn device_flow_obeys_poll_interval_slow_down_consent_and_refresh_account_binding() {
        let source = source();
        let control = Control::new(&|| false);
        let mut secret = DeviceSecret { binding: binding(&source), device_code: "secret+=&".into(), expires_at: 1000, next_poll: 105, interval: 5 };
        let calls = Arc::new(Mutex::new(Vec::new()));
        let error = |name| Response { status: 400, body: Zeroizing::new(serde_json::to_vec(&json!({"error":name})).unwrap()), retry_after: 0 };
        let token = || {
            reply(
                json!({"token_type":"Bearer","access_token":"test-access","refresh_token":"test-refresh","scope":"Mail.Read User.Read","expires_in":3600}),
            )
        };
        let network = FakeHttp {
            calls: calls.clone(),
            replies: RefCell::new(vec![error("authorization_pending"), error("slow_down"), token(), reply(json!({"id":"account"}))].into()),
        };
        assert!(matches!(poll_device(&network, &source, &mut secret, 101, &control).unwrap(), DevicePoll::Pending(4)));
        assert!(calls.lock().unwrap().is_empty());
        assert!(matches!(poll_device(&network, &source, &mut secret, 105, &control).unwrap(), DevicePoll::Pending(5)));
        assert_eq!(secret.next_poll, 110);
        assert!(matches!(poll_device(&network, &source, &mut secret, 110, &control).unwrap(), DevicePoll::Pending(10)));
        assert_eq!(secret.next_poll, 120);
        assert!(matches!(poll_device(&network, &source, &mut secret, 119, &control).unwrap(), DevicePoll::Pending(1)));
        let DevicePoll::Authorized(authorized) = poll_device(&network, &source, &mut secret, 120, &control).unwrap() else {
            panic!("expected authorization")
        };
        assert_eq!(authorized.account_id, "account");
        let log = calls.lock().unwrap();
        assert_eq!(log.len(), 4);
        assert!(log[0].0.starts_with("POST https://login.microsoftonline.com/common/oauth2/v2.0/token"));
        assert!(log[0].1.contains("device_code=secret%2B%3D%26"));
        drop(log);
        let network = FakeHttp {
            calls: calls.clone(),
            replies: RefCell::new(vec![token(), reply(json!({"id":"account"})), token(), reply(json!({"id":"other-account"}))].into()),
        };
        assert_eq!(refresh_token(&network, &source, &authorized, &control).unwrap().account_id, "account");
        assert!(refresh_token(&network, &source, &authorized, &control).is_err());
        assert!(matches!(poll_device(&network, &source, &mut secret, 1000, &control).unwrap(), DevicePoll::Finished(SignInStatus::Expired)));
        let network = FakeHttp { calls, replies: RefCell::new(vec![error("access_denied")].into()) };
        assert!(matches!(poll_device(&network, &source, &mut secret, 130, &control).unwrap(), DevicePoll::Finished(SignInStatus::Declined)));
    }
    #[test]
    fn device_flow_never_serializes_the_device_secret_and_limits_urls() {
        let data = json!({ "device_code": "DO-NOT-EXPOSE", "user_code": "ABCD-EFGH", "verification_uri": "https://microsoft.com/devicelogin", "expires_in":900, "interval":5 });
        let (public, secret) = parse_device(&source(), &data, 100).unwrap();
        assert_eq!(public.expires_at, 1000);
        assert_eq!(secret.next_poll, 105);
        assert!(!serde_json::to_string(&public).unwrap().contains("DO-NOT-EXPOSE"));
        let mut wrong = data;
        wrong["verification_uri"] = json!("https://attacker.example/login");
        assert!(parse_device(&source(), &wrong, 100).is_err());
    }
    #[test]
    fn pagination_keeps_bearer_tokens_on_the_expected_https_mailbox_host() {
        assert!(graph_url_ok("https://graph.microsoft.com/v1.0/me/mailFolders/inbox/messages?$skiptoken=a").is_ok());
        for url in [
            "http://graph.microsoft.com/v1.0/me/messages",
            "https://graph.microsoft.com.attacker.test/v1.0/me/messages",
            "https://user@graph.microsoft.com/v1.0/me/messages",
            "https://graph.microsoft.com:123/v1.0/me/messages",
            "https://graph.microsoft.com/v1.0/users/other/messages",
        ] {
            assert!(graph_url_ok(url).is_err());
        }
        let url = graph_url(&["me", "messages", "a/b+="]).unwrap();
        assert!(url.contains("a%2Fb+="));
    }
    #[test]
    fn tokens_require_mail_consent_and_keep_refresh_on_rotation_omission() {
        let source = source();
        let data = json!({ "token_type":"Bearer", "access_token":"opaque-access", "refresh_token":"opaque-refresh", "expires_in":3600, "scope":"User.Read Mail.Read" });
        let old = parse_token(&source, &data, None, 100).unwrap();
        let mut refreshed = data.clone();
        refreshed.as_object_mut().unwrap().remove("refresh_token");
        assert_eq!(parse_token(&source, &refreshed, Some(&old), 200).unwrap().refresh_token, "opaque-refresh");
        let write = OutlookSource { action: SourceAction::Delete, ..source };
        assert!(parse_token(&write, &data, None, 100).is_err());
    }
}
