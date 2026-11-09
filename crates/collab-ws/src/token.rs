//! Session tokens for the collaboration transport (ADR-0008, RFC §7).
//!
//! The cabinet issues a short-lived JWT bound to one file, one application and
//! one session. The token travels in `hello` and nowhere else; the server
//! derives the actor (`sub`) and the role from it and never trusts the body.
//!
//! Two signatures are accepted, and a deployment uses exactly one of them:
//!
//! - **HS256** with the shared secret the cabinet signs its session tickets
//!   with (`CRAFT_SYNC_JWT_SECRET`). The cabinet's ticket names the file and the
//!   session as `doc`/`sid`; the RFC's `fileId`/`sessionId` spelling is accepted
//!   too.
//! - **RS256** with the cabinet's public key, kept for deployments that issue
//!   asymmetric tickets.
//!
//! The daemon refuses to start when it is given both, or neither: a silent
//! fallback would let a wrong configuration accept tickets it should not.

use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode};
use pdfcraft_collab_proto::Role;
use serde::Deserialize;

/// The shortest HS256 secret accepted. The cabinet refuses anything shorter
/// when it is configured (`internal/application/craft/service.go`), so a shorter
/// one here could only be a mistake.
pub const MIN_HS256_SECRET_BYTES: usize = 16;

/// The claims a connection needs. Only the fields the transport enforces are
/// kept; `iss`, `iat` and `ver` are validated by the signature and `exp`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claims {
    /// The cabinet user id. Becomes the session actor.
    pub subject: String,
    /// The cabinet that issued the token (`aud`).
    pub audience: String,
    /// The file the token was issued for (`doc`, or `fileId`).
    pub file_id: String,
    /// The application the token was issued for (`app`). The verifier fills in
    /// the expected application when the claim is absent; the audience binds it.
    pub app: String,
    /// The role for this session.
    pub role: Role,
    /// The cabinet session (`sid`, or `sessionId`), used for revocation.
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
    #[error("the HS256 secret is shorter than 16 bytes")]
    WeakSecret,
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
        claims(decode_raw(token, &self.key, Algorithm::RS256, &self.audience)?, &self.app)
    }
}

/// Symmetric HS256 verification with the cabinet's shared secret. This is the
/// scheme the cabinet signs its tickets with today.
pub struct Hs256Verifier {
    key: DecodingKey,
    audience: String,
    app: String,
}

impl Hs256Verifier {
    /// `secret` is the raw key: the environment value, or a file's bytes without
    /// the trailing whitespace an editor adds.
    pub fn from_secret(secret: &[u8], audience: impl Into<String>, app: impl Into<String>) -> Result<Self, TokenError> {
        if secret.len() < MIN_HS256_SECRET_BYTES {
            return Err(TokenError::WeakSecret);
        }
        Ok(Self { key: DecodingKey::from_secret(secret), audience: audience.into(), app: app.into() })
    }
}

impl Verifier for Hs256Verifier {
    fn verify(&self, token: &str) -> Result<Claims, TokenError> {
        claims(decode_raw(token, &self.key, Algorithm::HS256, &self.audience)?, &self.app)
    }
}

/// Signature, expiry and audience, the same for both schemes. The algorithm is
/// fixed by the verifier, so a ticket signed with the other one is refused
/// before any claim is read.
fn decode_raw(token: &str, key: &DecodingKey, algorithm: Algorithm, audience: &str) -> Result<RawClaims, TokenError> {
    let mut validation = Validation::new(algorithm);
    validation.set_audience(&[audience]);
    validation.set_required_spec_claims(&["exp", "sub", "aud"]);
    validation.leeway = 30;

    decode::<RawClaims>(token, key, &validation).map(|token| token.claims).map_err(|e| match e.kind() {
        jsonwebtoken::errors::ErrorKind::ExpiredSignature => TokenError::Expired,
        _ => TokenError::Invalid,
    })
}

/// The checks every scheme shares: the application binding and the two names
/// the room needs, each of which the cabinet may spell either way.
fn claims(raw: RawClaims, app: &str) -> Result<Claims, TokenError> {
    if let Some(claimed) = raw.app.as_deref()
        && claimed != app
    {
        return Err(TokenError::WrongApp);
    }
    if raw.sub.trim().is_empty() {
        return Err(TokenError::Invalid);
    }
    Ok(Claims {
        subject: raw.sub,
        audience: raw.aud,
        file_id: one_name([raw.doc, raw.file_id.map(IdClaim::into_string)])?,
        app: app.to_owned(),
        role: raw.role,
        session_id: one_name([raw.sid, raw.session_id])?,
        expires_at: raw.exp,
    })
}

/// Both spellings the cabinet may use (`doc`/`fileId`, `sid`/`sessionId`). A
/// value that is present but empty, or two that disagree, is refused rather
/// than guessed: the room compares this name with `hello.doc`.
fn one_name(names: [Option<String>; 2]) -> Result<String, TokenError> {
    let mut found: Option<String> = None;
    for name in names.into_iter().flatten() {
        let name = name.trim().to_owned();
        if name.is_empty() {
            return Err(TokenError::Invalid);
        }
        if let Some(existing) = &found
            && *existing != name
        {
            return Err(TokenError::Invalid);
        }
        found = Some(name);
    }
    found.ok_or(TokenError::Invalid)
}

/// `fileId` is a string in the RFC's token and a number in the cabinet's
/// (`INTEGRATION.md` §3.2); both name the same file.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum IdClaim {
    Text(String),
    Number(u64),
}

impl IdClaim {
    fn into_string(self) -> String {
        match self {
            Self::Text(text) => text,
            Self::Number(number) => number.to_string(),
        }
    }
}

/// The wire claims. Names follow the cabinet contract (`doc`/`sid` in today's
/// ticket, `fileId`/`sessionId` in the RFC); unknown claims are ignored so the
/// cabinet can extend the token without breaking the server.
#[derive(Debug, Deserialize)]
struct RawClaims {
    sub: String,
    aud: String,
    #[serde(default)]
    doc: Option<String>,
    #[serde(default, rename = "fileId")]
    file_id: Option<IdClaim>,
    #[serde(default)]
    app: Option<String>,
    role: Role,
    #[serde(default)]
    sid: Option<String>,
    #[serde(default, rename = "sessionId")]
    session_id: Option<String>,
    exp: u64,
}

#[cfg(test)]
mod tests {
    use super::{Claims, Hs256Verifier, JwtVerifier, MIN_HS256_SECRET_BYTES, TokenError, Verifier};
    use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
    use pdfcraft_collab_proto::Role;
    use rsa::RsaPrivateKey;
    use rsa::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
    use serde::Serialize;

    /// The shared secret in these tests is 32 bytes; the cabinet's minimum is 16.
    const SECRET: &[u8] = b"0123456789abcdef0123456789abcdef";

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

    /// The ticket the cabinet actually signs: HS256, `doc`/`sid`, and a numeric
    /// `fileId` when the file is on disk.
    #[derive(Serialize)]
    struct CabinetClaims<'a> {
        iss: &'a str,
        sub: &'a str,
        aud: &'a str,
        doc: &'a str,
        role: &'a str,
        #[serde(rename = "fileId", skip_serializing_if = "Option::is_none")]
        file_id: Option<u64>,
        app: &'a str,
        sid: &'a str,
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

    fn hs_token(secret: &[u8], claims: &CabinetClaims<'_>) -> Result<String, Box<dyn std::error::Error>> {
        Ok(encode(&Header::new(Algorithm::HS256), claims, &EncodingKey::from_secret(secret))?)
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

    fn cabinet_claims(exp: u64) -> CabinetClaims<'static> {
        CabinetClaims {
            iss: "mail.lzmkm.ru",
            sub: "user-7",
            aud: "lzmkm",
            doc: "file-1",
            role: "editor",
            file_id: None,
            app: "pdfcraft",
            sid: "session-9",
            exp,
        }
    }

    fn now_secs() -> u64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
    }

    fn verifier(key: &Key) -> Result<JwtVerifier, Box<dyn std::error::Error>> {
        Ok(JwtVerifier::from_rsa_pem(key.public.as_bytes(), "lzmkm", "pdfcraft")?)
    }

    fn hs_verifier(secret: &[u8]) -> Result<Hs256Verifier, Box<dyn std::error::Error>> {
        Ok(Hs256Verifier::from_secret(secret, "lzmkm", "pdfcraft")?)
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

    #[test]
    fn an_hs256_cabinet_ticket_yields_actor_and_role() -> Result<(), Box<dyn std::error::Error>> {
        let exp = now_secs().saturating_add(600);
        let signed = hs_token(SECRET, &cabinet_claims(exp))?;
        assert_eq!(
            hs_verifier(SECRET)?.verify(&signed)?,
            Claims {
                subject: "user-7".into(),
                audience: "lzmkm".into(),
                file_id: "file-1".into(),
                app: "pdfcraft".into(),
                role: Role::Editor,
                session_id: "session-9".into(),
                expires_at: exp,
            }
        );
        Ok(())
    }

    #[test]
    fn the_rfc_names_and_a_numeric_file_id_are_accepted() -> Result<(), Box<dyn std::error::Error>> {
        let exp = now_secs().saturating_add(600);
        // `fileId`/`sessionId` as strings, as INTEGRATION.md §3.2 spells them.
        let signed = encode(&Header::new(Algorithm::HS256), &claims(exp), &EncodingKey::from_secret(SECRET))?;
        let verified = hs_verifier(SECRET)?.verify(&signed)?;
        assert_eq!((verified.file_id.as_str(), verified.session_id.as_str()), ("file-1", "session-9"));

        // `fileId` as the number the cabinet marshals.
        let mut numeric = cabinet_claims(exp);
        numeric.doc = "7";
        numeric.file_id = Some(7);
        let verified = hs_verifier(SECRET)?.verify(&hs_token(SECRET, &numeric)?)?;
        assert_eq!(verified.file_id, "7");
        Ok(())
    }

    #[test]
    fn hs256_expiry_signature_audience_and_application_are_checked() -> Result<(), Box<dyn std::error::Error>> {
        let verifier = hs_verifier(SECRET)?;
        let now = now_secs();

        assert_eq!(verifier.verify(&hs_token(SECRET, &cabinet_claims(now.saturating_sub(3_600)))?), Err(TokenError::Expired));

        let mut forged = hs_token(SECRET, &cabinet_claims(now + 600))?;
        if let Some(last) = forged.pop() {
            forged.push(if last == 'A' { 'B' } else { 'A' });
        }
        assert_eq!(verifier.verify(&forged), Err(TokenError::Invalid));

        let other_secret = b"ffffffffffffffffffffffffffffffff";
        assert_eq!(verifier.verify(&hs_token(other_secret, &cabinet_claims(now + 600))?), Err(TokenError::Invalid));

        let mut wrong_audience = cabinet_claims(now + 600);
        wrong_audience.aud = "another-cabinet";
        assert_eq!(verifier.verify(&hs_token(SECRET, &wrong_audience)?), Err(TokenError::Invalid));

        let mut wrong_app = cabinet_claims(now + 600);
        wrong_app.app = "wordcraft";
        assert_eq!(verifier.verify(&hs_token(SECRET, &wrong_app)?), Err(TokenError::WrongApp));

        assert_eq!(verifier.verify("not a jwt"), Err(TokenError::Invalid));
        Ok(())
    }

    #[test]
    fn a_ticket_without_a_file_or_a_session_is_refused() -> Result<(), Box<dyn std::error::Error>> {
        let verifier = hs_verifier(SECRET)?;
        let exp = now_secs().saturating_add(600);

        let mut no_file = cabinet_claims(exp);
        no_file.doc = "";
        assert_eq!(verifier.verify(&hs_token(SECRET, &no_file)?), Err(TokenError::Invalid));

        let mut no_session = cabinet_claims(exp);
        no_session.sid = "  ";
        assert_eq!(verifier.verify(&hs_token(SECRET, &no_session)?), Err(TokenError::Invalid));

        // The two spellings name the same file, so two different ones are a
        // contradiction, not a preference.
        let mut conflicting = cabinet_claims(exp);
        conflicting.file_id = Some(7);
        assert_eq!(verifier.verify(&hs_token(SECRET, &conflicting)?), Err(TokenError::Invalid));
        Ok(())
    }

    #[test]
    fn the_two_schemes_do_not_mix() -> Result<(), Box<dyn std::error::Error>> {
        let key = keys()?;
        let exp = now_secs().saturating_add(600);

        // An HS256 ticket is not an RS256 ticket...
        assert_eq!(verifier(&key)?.verify(&hs_token(SECRET, &cabinet_claims(exp))?), Err(TokenError::Invalid));
        // ...and the secret does not verify an RSA ticket.
        assert_eq!(hs_verifier(SECRET)?.verify(&token(&key, &claims(exp))?), Err(TokenError::Invalid));
        Ok(())
    }

    #[test]
    fn a_short_secret_is_refused() {
        assert_eq!(Hs256Verifier::from_secret(&[b'x'; MIN_HS256_SECRET_BYTES - 1], "lzmkm", "pdfcraft").err(), Some(TokenError::WeakSecret));
        assert!(Hs256Verifier::from_secret(&[b'x'; MIN_HS256_SECRET_BYTES], "lzmkm", "pdfcraft").is_ok());
    }
}
