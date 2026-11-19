use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD};
use oauth2::basic::{BasicErrorResponse, BasicRevocationErrorResponse, BasicTokenIntrospectionResponse, BasicTokenType};
use oauth2::{
    AuthUrl, AuthorizationCode, Client, ClientId, CsrfToken, EndpointNotSet, ExtraTokenFields, PkceCodeChallenge, RedirectUrl, Scope,
    StandardRevocableToken, StandardTokenResponse, TokenResponse, TokenUrl,
};
use serde::{Deserialize, Serialize};

use crate::PurviewAccount;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
struct MicrosoftTokenExtraFields {
    #[serde(default)]
    id_token: Option<String>,
}

impl ExtraTokenFields for MicrosoftTokenExtraFields {}

type MicrosoftTokenResponse = StandardTokenResponse<MicrosoftTokenExtraFields, BasicTokenType>;

type MicrosoftClient<
    HasAuthUrl = EndpointNotSet,
    HasDeviceAuthUrl = EndpointNotSet,
    HasIntrospectionUrl = EndpointNotSet,
    HasRevocationUrl = EndpointNotSet,
    HasTokenUrl = EndpointNotSet,
> = Client<
    BasicErrorResponse,
    MicrosoftTokenResponse,
    BasicTokenIntrospectionResponse,
    StandardRevocableToken,
    BasicRevocationErrorResponse,
    HasAuthUrl,
    HasDeviceAuthUrl,
    HasIntrospectionUrl,
    HasRevocationUrl,
    HasTokenUrl,
>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PurviewToken {
    pub access_token: String,
    pub account: Option<PurviewAccount>,
}

#[derive(Debug, thiserror::Error)]
pub enum PurviewAuthError {
    #[error("invalid Microsoft Entra app registration: {0}")]
    InvalidAppId(String),
    #[error("invalid Microsoft Entra authority: {0}")]
    InvalidAuthority(String),
    #[error("authentication browser could not be opened: {0}")]
    Browser(String),
    #[error("authentication timed out")]
    Timeout,
    #[error("authentication callback failed: {0}")]
    Callback(String),
    #[error("Microsoft Entra token exchange failed: {0}")]
    Token(String),
}

fn validate_app_id(value: &str) -> Result<String, PurviewAuthError> {
    if value.is_empty() {
        return Err(PurviewAuthError::InvalidAppId("the app registration client ID is missing".into()));
    }
    let Ok(id) = value.parse::<uuid::Uuid>() else {
        return Err(PurviewAuthError::InvalidAppId(value.into()));
    };
    if id.is_nil() {
        return Err(PurviewAuthError::InvalidAppId(value.into()));
    }
    Ok(id.to_string())
}

fn normalize_authority(value: &str, username: &str) -> Result<(String, String), PurviewAuthError> {
    let authority = value.trim().trim_end_matches('/');
    if authority.is_empty() {
        return Err(PurviewAuthError::InvalidAuthority("the authority is missing".into()));
    }
    if authority.contains("://") {
        let url = url::Url::parse(authority).map_err(|e| PurviewAuthError::InvalidAuthority(e.to_string()))?;
        let mut base = format!("{}://{}", url.scheme(), url.host_str().unwrap_or(""));
        if let Some(port) = url.port() {
            base.push(':');
            base.push_str(&port.to_string());
        }
        let path = url.path().trim_end_matches('/');
        let path = path.trim_end_matches("/oauth2/v2.0/authorize").trim_end_matches("/oauth2/v2.0/token");
        if !path.is_empty() {
            base.push_str(path);
        }
        return Ok((format!("{base}/oauth2/v2.0/authorize"), format!("{base}/oauth2/v2.0/token")));
    }
    let lower = authority.to_ascii_lowercase();
    let tenant = if lower == "common" || lower == "organizations" || lower.ends_with("/common") || lower.ends_with("/organizations") {
        username.rsplit_once('@').map(|(_, domain)| domain.to_ascii_lowercase()).unwrap_or_else(|| "common".to_owned())
    } else {
        authority.rsplit('/').next().unwrap_or("common").to_ascii_lowercase()
    };
    let base = format!("https://login.microsoftonline.com/{tenant}");
    Ok((format!("{base}/oauth2/v2.0/authorize"), format!("{base}/oauth2/v2.0/token")))
}

fn claim_string(claims: &serde_json::Map<String, serde_json::Value>, names: &[&str]) -> Option<String> {
    names
        .iter()
        .find_map(|name| claims.get(*name).and_then(|value| value.as_str()).map(str::trim).filter(|value| !value.is_empty()).map(str::to_owned))
}

fn account_from_id_token(id_token: Option<&str>, username_hint: &str) -> Option<PurviewAccount> {
    let id_token = id_token?;
    let payload = id_token.split('.').nth(1)?;
    let decoded = URL_SAFE_NO_PAD.decode(payload).or_else(|_| URL_SAFE.decode(payload)).ok()?;
    let claims: serde_json::Map<String, serde_json::Value> = serde_json::from_slice(&decoded).ok()?;

    let email = claim_string(&claims, &["email", "preferred_username", "upn", "unique_name"]).or_else(|| {
        let username = username_hint.trim();
        (!username.is_empty()).then(|| username.to_owned())
    });
    let principal_name = claim_string(&claims, &["preferred_username", "upn", "unique_name", "email"]).or_else(|| email.clone());
    let display_name = claim_string(&claims, &["name", "displayName"]).unwrap_or_else(|| {
        principal_name
            .as_deref()
            .and_then(|value| value.split('@').next())
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| "Microsoft account".to_owned())
    });
    let id = claim_string(&claims, &["oid", "sub"])
        .or_else(|| email.clone())
        .or_else(|| principal_name.clone())
        .unwrap_or_else(|| "microsoft-account".to_owned());
    Some(PurviewAccount { id, display_name, email: email.unwrap_or_default(), principal_name: principal_name.unwrap_or_default() })
}

fn scopes(resource: &str, challenge_scope: &str) -> Vec<String> {
    let challenge_scope = challenge_scope.trim();
    if !challenge_scope.is_empty() {
        let parsed: Vec<String> = challenge_scope
            .split(|ch: char| ch.is_whitespace() || ch == ',')
            .filter(|value| !value.trim().is_empty())
            .map(str::trim)
            .map(ToOwned::to_owned)
            .collect();
        if !parsed.is_empty() {
            return parsed;
        }
    }
    let resource = resource.trim().trim_end_matches('/');
    if resource.is_empty() {
        vec!["https://api.aadrm.com/.default".into()]
    } else if resource.ends_with("/.default") {
        vec![resource.to_owned()]
    } else {
        vec![format!("{resource}/.default")]
    }
}

fn read_callback(listener: &TcpListener) -> Result<(AuthorizationCode, CsrfToken), PurviewAuthError> {
    listener.set_nonblocking(true).map_err(|e| PurviewAuthError::Callback(e.to_string()))?;
    let deadline = std::time::Instant::now() + Duration::from_secs(300);
    loop {
        match listener.accept() {
            Ok((mut stream, _)) => return parse_callback(&mut stream),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if std::time::Instant::now() >= deadline {
                    return Err(PurviewAuthError::Timeout);
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(e) => return Err(PurviewAuthError::Callback(e.to_string())),
        }
    }
}

fn parse_callback(stream: &mut TcpStream) -> Result<(AuthorizationCode, CsrfToken), PurviewAuthError> {
    stream.set_read_timeout(Some(Duration::from_secs(10))).map_err(|e| PurviewAuthError::Callback(e.to_string()))?;
    let mut request = [0_u8; 4096];
    let read = stream.read(&mut request).map_err(|e| PurviewAuthError::Callback(e.to_string()))?;
    let text = String::from_utf8_lossy(&request[..read]);
    let first = text.lines().next().unwrap_or_default();
    let target = first.split_whitespace().nth(1).unwrap_or("/");
    let full = format!("http://localhost{target}");
    let url = url::Url::parse(&full).map_err(|e| PurviewAuthError::Callback(e.to_string()))?;
    let query: std::collections::HashMap<String, String> = url.query_pairs().into_owned().collect();
    if let Some(error) = query.get("error") {
        let description = query.get("error_description").map(String::as_str).unwrap_or(error);
        let body = "Authentication was cancelled or denied.";
        let _ = stream.write_all(
            format!("HTTP/1.1 400 Bad Request\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body)
                .as_bytes(),
        );
        return Err(PurviewAuthError::Callback(description.into()));
    }
    let code = query.get("code").ok_or_else(|| PurviewAuthError::Callback("authorization callback did not include a code".into()))?;
    let state = query.get("state").ok_or_else(|| PurviewAuthError::Callback("authorization callback did not include state".into()))?;
    let body = "PdfCraft authentication is complete. You can close this window.";
    let _ = stream.write_all(
        format!("HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).as_bytes(),
    );
    Ok((AuthorizationCode::new(code.clone()), CsrfToken::new(state.clone())))
}

pub fn acquire_token(
    app_id_value: &str,
    username: &str,
    authority: &str,
    resource: &str,
    challenge_scope: &str,
    claims: &str,
) -> Result<PurviewToken, PurviewAuthError> {
    let app_id = validate_app_id(app_id_value)?;
    let (authorize_url, token_url) = normalize_authority(authority, username)?;
    let scopes = scopes(resource, challenge_scope);
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| PurviewAuthError::Callback(e.to_string()))?;
    let port = listener.local_addr().map_err(|e| PurviewAuthError::Callback(e.to_string()))?.port();
    let redirect = format!("http://localhost:{port}");

    let client = MicrosoftClient::new(ClientId::new(app_id))
        .set_auth_uri(AuthUrl::new(authorize_url).map_err(|e| PurviewAuthError::InvalidAuthority(e.to_string()))?)
        .set_token_uri(TokenUrl::new(token_url).map_err(|e| PurviewAuthError::InvalidAuthority(e.to_string()))?)
        .set_redirect_uri(RedirectUrl::new(redirect).map_err(|e| PurviewAuthError::Callback(e.to_string()))?);

    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
    let mut request = client.authorize_url(CsrfToken::new_random).set_pkce_challenge(pkce_challenge);
    request =
        request.add_scope(Scope::new("openid".to_owned())).add_scope(Scope::new("profile".to_owned())).add_scope(Scope::new("email".to_owned()));
    for scope in scopes {
        request = request.add_scope(Scope::new(scope));
    }
    if !claims.trim().is_empty() {
        request = request.add_extra_param("claims", claims);
    }
    if !username.trim().is_empty() {
        request = request.add_extra_param("login_hint", username);
    } else {
        request = request.add_extra_param("prompt", "select_account");
    }
    let (url, expected_state) = request.url();
    if webbrowser::open(url.as_str()).is_err() {
        return Err(PurviewAuthError::Browser(url.to_string()));
    }
    let (code, returned_state) = read_callback(&listener)?;
    if returned_state.secret() != expected_state.secret() {
        return Err(PurviewAuthError::Callback("OAuth state mismatch".into()));
    }
    let http_client = reqwest::blocking::ClientBuilder::new().use_rustls_tls().build().map_err(|e| PurviewAuthError::Token(e.to_string()))?;
    let token =
        client.exchange_code(code).set_pkce_verifier(pkce_verifier).request(&http_client).map_err(|e| PurviewAuthError::Token(e.to_string()))?;
    let account = account_from_id_token(token.extra_fields().id_token.as_deref(), username);
    Ok(PurviewToken { access_token: token.access_token().secret().clone(), account })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_id_must_be_a_non_nil_uuid() {
        assert!(validate_app_id("").is_err());
        assert!(validate_app_id("not-a-guid").is_err());
        assert!(validate_app_id("00000000-0000-0000-0000-000000000000").is_err());
        assert!(validate_app_id("e16b2bcd-0a3c-414a-8255-0bcc5f6a7246").is_ok());
    }

    #[test]
    fn challenge_scopes_are_space_or_comma_separated() {
        assert_eq!(scopes("https://api.aadrm.com", "https://api.aadrm.com/user_impersonation"), vec!["https://api.aadrm.com/user_impersonation"]);
        assert_eq!(
            scopes("https://api.aadrm.com", "https://api.aadrm.com/.default https://psor.api.microsoft.com/.default"),
            vec!["https://api.aadrm.com/.default", "https://psor.api.microsoft.com/.default"]
        );
    }

    #[test]
    fn resource_scopes_fall_back_to_default_scope() {
        assert_eq!(scopes("", ""), vec!["https://api.aadrm.com/.default"]);
        assert_eq!(scopes("https://api.aadrm.com", ""), vec!["https://api.aadrm.com/.default"]);
        assert_eq!(scopes("https://psor.api.microsoft.com", ""), vec!["https://psor.api.microsoft.com/.default"]);
    }

    #[test]
    fn authority_uses_username_domain_for_organizations() {
        let (authorize, token) = normalize_authority("organizations", "person@example.com").expect("organizations authority should resolve");
        assert_eq!(authorize, "https://login.microsoftonline.com/example.com/oauth2/v2.0/authorize");
        assert_eq!(token, "https://login.microsoftonline.com/example.com/oauth2/v2.0/token");

        let (authorize, token) = normalize_authority("common", "person@MacCyberDoc.onmicrosoft.com").expect("common authority should resolve");
        assert_eq!(authorize, "https://login.microsoftonline.com/maccyberdoc.onmicrosoft.com/oauth2/v2.0/authorize");
        assert_eq!(token, "https://login.microsoftonline.com/maccyberdoc.onmicrosoft.com/oauth2/v2.0/token");
    }

    #[test]
    fn id_token_profile_prefers_display_name_and_email() {
        let payload = serde_json::json!({
            "oid": "account-object-id",
            "name": "Ada Lovelace",
            "preferred_username": "ada@example.com",
            "email": "ada@example.com"
        })
        .to_string();
        let segment = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload.as_bytes());
        let account = account_from_id_token(Some(&format!("header.{segment}.signature")), "").expect("profile");
        assert_eq!(account.id, "account-object-id");
        assert_eq!(account.display_name, "Ada Lovelace");
        assert_eq!(account.email, "ada@example.com");
        assert_eq!(account.principal_name, "ada@example.com");
        assert_eq!(account.label(), "Ada Lovelace - ada@example.com");
    }
}
