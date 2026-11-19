use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

use oauth2::basic::BasicClient;
use oauth2::{AuthUrl, AuthorizationCode, ClientId, CsrfToken, PkceCodeChallenge, RedirectUrl, Scope, TokenResponse, TokenUrl};

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
) -> Result<String, PurviewAuthError> {
    let app_id = validate_app_id(app_id_value)?;
    let (authorize_url, token_url) = normalize_authority(authority, username)?;
    let scopes = scopes(resource, challenge_scope);
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| PurviewAuthError::Callback(e.to_string()))?;
    let port = listener.local_addr().map_err(|e| PurviewAuthError::Callback(e.to_string()))?.port();
    let redirect = format!("http://localhost:{port}");

    let client = BasicClient::new(ClientId::new(app_id))
        .set_auth_uri(AuthUrl::new(authorize_url).map_err(|e| PurviewAuthError::InvalidAuthority(e.to_string()))?)
        .set_token_uri(TokenUrl::new(token_url).map_err(|e| PurviewAuthError::InvalidAuthority(e.to_string()))?)
        .set_redirect_uri(RedirectUrl::new(redirect).map_err(|e| PurviewAuthError::Callback(e.to_string()))?);

    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
    let mut request = client.authorize_url(CsrfToken::new_random).set_pkce_challenge(pkce_challenge);
    for scope in scopes {
        request = request.add_scope(Scope::new(scope));
    }
    if !claims.trim().is_empty() {
        request = request.add_extra_param("claims", claims);
    }
    if !username.trim().is_empty() {
        request = request.add_extra_param("login_hint", username);
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
    Ok(token.access_token().secret().clone())
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
}
