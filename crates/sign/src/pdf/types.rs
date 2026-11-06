use super::Certificate;

/// Certificates the user trusts for signing (Acrobat: Trusted Certificates).
#[derive(Clone, Debug, Default)]
pub struct TrustStore {
    pub certs: Vec<Certificate>,
}

impl TrustStore {
    pub(super) fn trusts(&self, c: &Certificate) -> bool {
        self.certs.iter().any(|t| t.raw == c.raw)
    }
}

/// Acrobat's three verdicts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Intact, signed by a trusted identity, and any later changes are permitted.
    Valid,
    /// Intact, but the signer's identity can't be established (not trusted), or validity
    /// couldn't be determined.
    Unknown,
    /// The signed bytes changed, the signature doesn't verify, or later changes aren't permitted.
    Invalid,
}

/// What a change made after signing is called: the words [`Modification`] lists it by, and the
/// vocabulary a signature's details speak of changes in.
///
/// They are words of ours, not the document's, so an interface reading in another language
/// translates each of them; [`ALL`](kind::ALL) is the whole vocabulary.
pub mod kind {
    pub const PAGE_CONTENT: &str = "page content";
    pub const FORM_FILL: &str = "form fill";
    pub const COMMENTS: &str = "comments";
    pub const SIGNATURE: &str = "signature";
    pub const SECURITY_STORE: &str = "document security store";
    pub const METADATA: &str = "metadata";
    pub const LINKS: &str = "links";
    pub const STRUCTURE: &str = "document structure";
    pub const PAGES: &str = "pages added or removed";
    pub const OTHER: &str = "other changes";
    /// Not a change but the reason there is nothing to compare: the signed revision itself could not
    /// be read, so what came after it cannot be allowed.
    pub const UNREADABLE: &str = "the signed version could not be read";

    /// Every word above, for an interface to translate.
    pub const ALL: &[&str] = &[PAGE_CONTENT, FORM_FILL, COMMENTS, SIGNATURE, SECURITY_STORE, METADATA, LINKS, STRUCTURE, PAGES, OTHER, UNREADABLE];
}

/// Changes made after signing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Modification {
    /// The signature covers the whole file.
    None,
    /// Later revisions only make changes the signature permits, listed in the words of [`kind`].
    Allowed(Vec<&'static str>),
    /// Later revisions make changes it doesn't permit, listed in the words of [`kind`].
    Disallowed(Vec<&'static str>),
}

/// One of the sentences that explain a signature's verdict.
///
/// [`message`](Detail::message) is the sentence in English, ready to read. An interface that reads in
/// another language says it with [`Detail::said`] instead, out of [`pattern`](Detail::pattern) —
/// wording of ours, listed in [`WORDINGS`] — and [`fill`](Detail::fill), whatever goes in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Detail {
    /// The sentence in English.
    pub message: String,
    /// The sentence as it is worded here, with `{}` where the fill goes.
    pub pattern: &'static str,
    pub fill: Fill,
}

/// What goes where a [`Detail`]'s pattern has `{}`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fill {
    /// Nothing: the pattern is the whole sentence.
    Nothing,
    /// The changes made after signing, as [`Modification`] lists them: words of ours, each one in
    /// [`kind::ALL`], said one after another.
    Changes(Vec<&'static str>),
    /// What a reader could not make of the signature, in its own words. It goes in as it stands: it
    /// names the structure that is wrong, which is a developer's business and reads the same
    /// everywhere.
    Reason(String),
    /// A value out of the signature itself — a time, the evidence a revocation came from — written
    /// as it stands, the way a certificate's name or date is.
    Value(String),
    /// Two certificates' own names, for the wording that says one was not used to vouch for the
    /// other. They go in the order the wording puts its two holes in, which a language may reverse.
    Names(String, String),
}

impl Detail {
    /// The sentence said in the language `words` translates into.
    ///
    /// The wording is translated, and what fills it goes in afterwards — the changes translated too —
    /// so nothing put in can be mistaken for a hole of its own. With [`in_english`] this is
    /// [`message`](Detail::message).
    pub fn said(&self, words: Words) -> String {
        let said = words(self.pattern);
        match &self.fill {
            Fill::Nothing => said,
            Fill::Changes(c) => said.replacen("{}", &c.iter().map(|k| words(k)).collect::<Vec<_>>().join(", "), 1),
            Fill::Reason(r) | Fill::Value(r) => said.replacen("{}", r, 1),
            // Both holes are cut out of the wording first, so neither name can be taken for the other's.
            Fill::Names(a, b) => {
                let mut parts = said.splitn(3, "{}");
                match (parts.next(), parts.next(), parts.next()) {
                    (Some(p0), Some(p1), Some(p2)) => format!("{p0}{a}{p1}{b}{p2}"),
                    // A wording with one hole, or none: say it as it is rather than guess.
                    _ => said,
                }
            }
        }
    }

    /// The whole sentence, with no hole in it.
    pub(crate) fn plain(pattern: &'static str) -> Detail {
        Detail::of(pattern, Fill::Nothing)
    }

    /// The sentence and what fills its hole. The English is said once, here, so it and
    /// [`Detail::said`] cannot drift apart.
    pub(crate) fn of(pattern: &'static str, fill: Fill) -> Detail {
        let mut d = Detail { message: String::new(), pattern, fill };
        d.message = d.said(&in_english);
        d
    }
}

/// How an interface says one of this crate's wordings in its own language: it is handed the English
/// and gives back what it reads. [`in_english`] is the one that keeps the English.
pub type Words<'a> = &'a dyn Fn(&str) -> String;

/// [`Words`] that leaves the English as it is.
pub fn in_english(wording: &str) -> String {
    wording.to_owned()
}

/// Every sentence this crate says about a signature: what [`SignatureInfo::details`] can hold, and
/// the summaries [`SignatureInfo::summary`] chooses between.
///
/// An interface reading in another language translates these and the words in [`kind::ALL`], which
/// go into the two sentences that list changes. The rest of what it shows comes from elsewhere and
/// goes through as it stands: the certificate's names and dates, and a reader's own detail in
/// [`Fill::Reason`]. A test in this crate keeps the list in step with the code.
pub const WORDINGS: &[&str] = &[
    // What the signature's own structure says, before anything is verified.
    "The signature has no valid byte range.",
    "The signature's byte range does not match the file.",
    "The signature's byte range does not exclude exactly its contents.",
    "The signature contents are not hexadecimal.",
    "The signature could not be read ({}).",
    "PdfCraft can't check this signature yet: {}.",
    "The signature uses the legacy adbe.x509.rsa_sha1 format.",
    // What was read leniently on the way, each sentence worded where the reading happens.
    "The signature is BER-encoded (indefinite lengths) rather than DER; it was read leniently.",
    "The signed content is a constructed (segmented) OCTET STRING, as BER allows; it was read anyway.",
    "The RSA signature value does not have the length of the key's modulus (leading zero bytes dropped or added); it was read anyway.",
    "The RSA signature's DigestInfo has no NULL parameter (older signers write it so); it was read anyway.",
    "The ECDSA signature is not in canonical DER (trailing bytes, padding or a missing sign byte); it was read anyway.",
    "The ECDSA signature is the raw r and s values instead of a DER sequence; it was read anyway.",
    "The signature algorithm and the SignerInfo's digest algorithm name different hashes; the signature was made with the SignerInfo's.",
    "The signature's signed attributes have no content type, which CMS requires; it was read anyway.",
    // What a signature that validates with an old hash or a small key still does not prove.
    "This signature uses SHA-1, a weak hash algorithm: collisions in it have been demonstrated. It still validates because many documents signed years ago use it, but it should not be relied on.",
    "This signature uses RIPEMD-160, a legacy 160-bit hash algorithm that is no longer recommended for signatures. It still validates, but it should not be relied on.",
    "This signature uses an RSA key shorter than 2048 bits, which is no longer considered strong enough for signatures. It still validates, but it should not be relied on.",
    // Whether the signed bytes and the signature still agree.
    "The signature's signed attributes have no message digest, so the signature does not cover the document.",
    "The signature's signed attributes name a content type other than data, so it is not a signature of the document.",
    "The document has been altered or corrupted since the signature was applied.",
    "The signer's certificate is not in the signature.",
    "The signature value does not match the signer's certificate: the signature is corrupt.",
    "The signature could not be checked ({}).",
    "The signature could not be verified ({}).",
    // What came after the signature.
    "This document has not been modified since this signature was applied.",
    "The document has been modified since this signature was applied, but the changes are permitted ({}).",
    "The document has been altered since this signature was applied in ways it does not permit ({}).",
    // Who signed, and when.
    "The signer's certificate was not valid at the time of signing.",
    "The signer's certificate does not allow digital signatures (its key usage has neither digital signature nor non-repudiation).",
    "The signer's certificate is not issued for signing documents (its extended key usage has no document-signing purpose).",
    "The signer's certificate has critical extensions PdfCraft does not recognize ({}).",
    // Why a certificate that matched the issuer was not used to build the chain.
    "The certificate of {} is not a CA certificate that may issue certificates, so it is not used to vouch for {}.",
    "The certificate of {} has critical extensions PdfCraft does not recognize, so it is not used to vouch for {}.",
    "The certificate of {} does not allow this many CA certificates below it (path length), so it is not used to vouch for {}.",
    "The certificate of {} was not valid at the time of signing, so it is not used to vouch for {}.",
    "The signer's identity is unknown because it has not been included in your list of trusted certificates and none of its parent certificates are trusted certificates.",
    "The signer's identity is valid.",
    "The signature includes an embedded timestamp.",
    "Signing time is from the clock on the signer's computer.",
    // What the signature's own timestamp token proves, if it carries one.
    "The signature's timestamp token does not cover the signature value.",
    "The signature's timestamp token could not be verified ({}).",
    "The embedded timestamp token is valid and its authority is trusted; trusted time is {}.",
    "The signature carries an unverified timestamp ({}): the timestamp authority is not in your list of trusted certificates, so the signing time claimed by the signer is used instead.",
    // What the document's own revocation evidence says about the signer.
    "The signer's certificate has been revoked ({}).",
    "The document's embedded revocation information shows that the signer's certificate has not been revoked.",
    // A standalone document timestamp, which is checked on its own.
    "The timestamp has no valid byte range.",
    "The timestamp's byte range does not match the file.",
    "The timestamp's byte range does not exclude exactly its contents.",
    "The timestamp contents are not hexadecimal.",
    "The timestamp token could not be read ({}).",
    "The document has been altered or corrupted since the timestamp was applied.",
    "The timestamp authority's certificate was not valid at the time of timestamping.",
    "The timestamp authority's certificate is not issued for time stamping.",
    "This document has not been modified since this timestamp was applied.",
    "The document has been modified since this timestamp was applied, but the changes are permitted ({}).",
    "The document has been altered since this timestamp was applied in ways it does not permit ({}).",
    "The timestamp token is valid and its authority is trusted.",
    "The timestamp token is valid, but the timestamp authority is not in your list of trusted certificates.",
    // The one line that stands for all of it.
    "Unsigned signature field",
    "Signature is valid",
    "Signature validity is unknown",
    "Signature is invalid",
];
