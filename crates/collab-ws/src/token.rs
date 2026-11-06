//! Session tokens for the collaboration transport (ADR-0008).
//!
//! The cabinet issues a short-lived JWT bound to one file, one application and
//! one session. The token travels in `hello` and nowhere else; the server
//! derives the actor (`sub`) and the role from it and never trusts the body.

use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode};
use pdfcraft_collab_proto::Role;
use serde::Deserialize;

/// The claims a connection needs. Only the fields the transport enforces are
/// kept; `iss`, `iat` and `ver` are validated by the signature and `exp`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claims {
    /// The cabinet user id. Becomes the session actor.
    pub subject: String,
    /// The cabinet that issued the token (`aud`).
    pub audience: String,
    /// The file the token was issued for (`fileId`).
    pub file_id: String,
    /// The application the token was issued for (`app`).
    pub app: String,
    /// The role for this session.
    pub role: Role,
    /// The cabinet session (`sessionId`), used for revocation.
    pub session_id: String,
    /// Expiry, seconds since the Unix epoch.
    pub expires_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TokenError {
    #[error("the session token has expired")]
    Expired,
    #[error("the session token is not valid")]
    Invalid,
    #[error("the session token names another file")]
    WrongFile,
    #[error("the session token names another application")]
    WrongApp,
}

/// Verifies `hello.jwt` and returns the claims the room must honour.
pub trait Verifier: Send + Sync {
    fn verify(&self, token: &str) -> Result<Claims, TokenError>;
}

/// Asymmetric RS256 verification with the cabinet's public key (ADR-0008).
pub struct JwtVerifier {
    key: DecodingKey,
    audience: String,
    app: String,
}

impl JwtVerifier {
    /// `public_pem` is the cabinet's RSA public key (PKCS#1 or PKCS#8 PEM).
    pub fn from_rsa_pem(public_pem: &[u8], audience: impl Into<String>, app: impl Into<String>) -> Result<Self, TokenError> {
        let key = DecodingKey::from_rsa_pem(public_pem).map_err(|_| TokenError::Invalid)?;
        Ok(Self { key, audience: audience.into(), app: app.into() })
    }
}

impl Verifier for JwtVerifier {
    fn verify(&self, token: &str) -> Result<Claims, TokenError> {
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_audience(&[self.audience.as_str()]);
        validation.set_required_spec_claims(&["exp", "sub", "aud"]);
        validation.leeway = 30;

        let raw = decode::<RawClaims>(token, &self.key, &validation)
            .map_err(|e| match e.kind() {
                jsonwebtoken::errors::ErrorKind::ExpiredSignature => TokenError::Expired,
                _ => TokenError::Invalid,
            })?
            .claims;
        if raw.app != self.app {
            return Err(TokenError::WrongApp);
        }
        if raw.file_id.trim().is_empty() || raw.session_id.trim().is_empty() || raw.sub.trim().is_empty() {
            return Err(TokenError::Invalid);
        }
        Ok(Claims {
            subject: raw.sub,
            audience: raw.aud,
            file_id: raw.file_id,
            app: raw.app,
            role: raw.role,
            session_id: raw.session_id,
            expires_at: raw.exp,
        })
    }
}

/// The wire claims. Names follow the cabinet contract (camelCase for
/// `fileId`/`sessionId`); unknown claims are ignored so the cabinet can extend
/// the token without breaking the server.
#[derive(Debug, Deserialize)]
struct RawClaims {
    sub: String,
    aud: String,
    #[serde(rename = "fileId")]
    file_id: String,
    app: String,
    role: Role,
    #[serde(rename = "sessionId")]
    session_id: String,
    exp: u64,
}

#[cfg(test)]
mod tests {
    use super::{Claims, JwtVerifier, TokenError, Verifier};
    use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
    use pdfcraft_collab_proto::Role;
    use rsa::RsaPrivateKey;
    use rsa::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
    use serde::Serialize;

    #[derive(Serialize)]
    struct TestClaims<'a> {
        iss: &'a str,
        sub: &'a str,
        aud: &'a str,
        #[serde(rename = "fileId")]
        file_id: &'a str,
        app: &'a str,
        role: &'a str,
        #[serde(rename = "sessionId")]
        session_id: &'a str,
        exp: u64,
    }

    struct Key {
        private: String,
        public: String,
    }

    fn keys() -> Result<Key, Box<dyn std::error::Error>> {
        let mut rng = rand::thread_rng();
        let key = RsaPrivateKey::new(&mut rng, 2048)?;
        Ok(Key { private: key.to_pkcs8_pem(LineEnding::LF)?.to_string(), public: key.to_public_key().to_public_key_pem(LineEnding::LF)? })
    }

    fn token(key: &Key, claims: &TestClaims<'_>) -> Result<String, Box<dyn std::error::Error>> {
        let encoding = EncodingKey::from_rsa_pem(key.private.as_bytes())?;
        Ok(encode(&Header::new(Algorithm::RS256), claims, &encoding)?)
    }

    fn claims(exp: u64) -> TestClaims<'static> {
        TestClaims {
            iss: "cabinet",
            sub: "user-7",
            aud: "lzmkm",
            file_id: "file-1",
            app: "pdfcraft",
            role: "commenter",
            session_id: "session-9",
            exp,
        }
    }

    fn now_secs() -> u64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
    }

    fn verifier(key: &Key) -> Result<JwtVerifier, Box<dyn std::error::Error>> {
        Ok(JwtVerifier::from_rsa_pem(key.public.as_bytes(), "lzmkm", "pdfcraft")?)
    }

    #[test]
    fn a_valid_token_yields_actor_and_role() -> Result<(), Box<dyn std::error::Error>> {
        let key = keys()?;
        let exp = now_secs().saturating_add(600);
        let signed = token(&key, &claims(exp))?;
        assert_eq!(
            verifier(&key)?.verify(&signed)?,
            Claims {
                subject: "user-7".into(),
                audience: "lzmkm".into(),
                file_id: "file-1".into(),
                app: "pdfcraft".into(),
                role: Role::Commenter,
                session_id: "session-9".into(),
                expires_at: exp,
            }
        );
        Ok(())
    }

    #[test]
    fn expiry_signature_audience_and_application_are_checked() -> Result<(), Box<dyn std::error::Error>> {
        let key = keys()?;
        let verifier = verifier(&key)?;
        let now = now_secs();

        let expired = token(&key, &claims(now.saturating_sub(3_600)))?;
        assert_eq!(verifier.verify(&expired), Err(TokenError::Expired));

        let mut forged = token(&key, &claims(now + 600))?;
        if let Some(last) = forged.pop() {
            forged.push(if last == 'A' { 'B' } else { 'A' });
        }
        assert_eq!(verifier.verify(&forged), Err(TokenError::Invalid));

        let mut wrong_audience = claims(now + 600);
        wrong_audience.aud = "another-cabinet";
        assert_eq!(verifier.verify(&token(&key, &wrong_audience)?), Err(TokenError::Invalid));

        let mut wrong_app = claims(now + 600);
        wrong_app.app = "wordcraft";
        assert_eq!(verifier.verify(&token(&key, &wrong_app)?), Err(TokenError::WrongApp));

        assert_eq!(verifier.verify("not a jwt"), Err(TokenError::Invalid));
        Ok(())
    }
}
