//! X.509 certificates (RFC 5280): the fields signing and validation need, signature checks
//! along a chain, and a self-signed certificate builder for new digital IDs.

use crate::SignError;
use crate::der::{self, Time, Tlv, tag};
use crate::keys::{self, DigestAlg, PrivateKey, PublicKey};

const CN: &str = "2.5.4.3";
const C: &str = "2.5.4.6";
const O: &str = "2.5.4.10";
const OU: &str = "2.5.4.11";
const EMAIL: &str = "1.2.840.113549.1.9.1";

/// A distinguished name: (attribute OID, value) in order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Name {
    pub attrs: Vec<(String, String)>,
    /// The DER encoding, compared byte for byte when matching issuers to subjects.
    pub raw: Vec<u8>,
}

impl Name {
    fn parse(t: &Tlv<'_>) -> Result<Name, SignError> {
        let mut attrs = Vec::new();
        for rdn in t.expect(tag::SEQUENCE, "Name")?.children()? {
            for atv in rdn.children()? {
                let parts = atv.children()?;
                if let [o, v] = parts.as_slice() {
                    attrs.push((o.oid()?, v.text().unwrap_or_default()));
                }
            }
        }
        Ok(Name { attrs, raw: t.raw.to_vec() })
    }

    fn get(&self, oid: &str) -> Option<&str> {
        self.attrs.iter().find(|(o, _)| o == oid).map(|(_, v)| v.as_str())
    }

    pub fn common_name(&self) -> Option<&str> {
        self.get(CN)
    }

    pub fn organization(&self) -> Option<&str> {
        self.get(O)
    }

    pub fn unit(&self) -> Option<&str> {
        self.get(OU)
    }

    pub fn email(&self) -> Option<&str> {
        self.get(EMAIL)
    }

    pub fn country(&self) -> Option<&str> {
        self.get(C)
    }

    /// "CN=Ada Lovelace, O=Example, E=ada@example.com"
    pub fn display(&self) -> String {
        self.attrs
            .iter()
            .map(|(o, v)| {
                let k = match o.as_str() {
                    CN => "CN",
                    C => "C",
                    O => "O",
                    OU => "OU",
                    EMAIL => "E",
                    "2.5.4.7" => "L",
                    "2.5.4.8" => "ST",
                    other => other,
                };
                format!("{k}={v}")
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// A name from its parts (empty ones left out), in the usual order.
    pub fn build(cn: &str, ou: &str, o: &str, email: &str, country: &str) -> Name {
        let mut rdns: Vec<Vec<u8>> = Vec::new();
        let mut attrs = Vec::new();
        let mut add = |oid: &str, enc: Vec<u8>, v: &str| {
            if !v.trim().is_empty() {
                rdns.push(der::set_of(&[&der::seq(&[&der::oid(oid), &enc])]));
                attrs.push((oid.to_string(), v.trim().to_string()));
            }
        };
        let country = country.trim().to_ascii_uppercase();
        if country.len() == 2 {
            add(C, der::printable(&country), &country);
        }
        add(O, der::utf8(o.trim()), o);
        add(OU, der::utf8(ou.trim()), ou);
        add(CN, der::utf8(cn.trim()), cn);
        add(EMAIL, der::ia5(email.trim()), email);
        let raw = der::tlv(tag::SEQUENCE, &rdns.concat());
        Name { attrs, raw }
    }
}

/// A parsed certificate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Certificate {
    /// The whole DER encoding.
    pub raw: Vec<u8>,
    tbs: Vec<u8>,
    sig_alg: Vec<u8>,
    signature: Vec<u8>,
    pub serial: Vec<u8>,
    pub issuer: Name,
    pub subject: Name,
    pub not_before: Time,
    pub not_after: Time,
    pub public_key: PublicKey,
    /// Basic constraints: a CA certificate.
    pub is_ca: bool,
    /// Key usage bits (bit 0 = digitalSignature, 1 = nonRepudiation, 5 = keyCertSign), if present.
    pub key_usage: Option<u16>,
    pub subject_key_id: Option<Vec<u8>>,
    /// Authority key identifier (2.5.29.35), when present.
    pub authority_key_id: Option<Vec<u8>>,
    /// Extended key usage OIDs (2.5.29.37), when present.
    pub extended_key_usage: Option<Vec<String>>,
    /// OCSP responder URLs from the Authority Information Access (1.3.6.1.5.5.7.1.1).
    pub ocsp_urls: Vec<String>,
    /// CRL distribution point URLs (2.5.29.31).
    pub crl_urls: Vec<String>,
}

impl Certificate {
    pub fn parse(raw: &[u8]) -> Result<Certificate, SignError> {
        let cert = Tlv::parse_all(raw)?.expect(tag::SEQUENCE, "Certificate")?;
        let parts = cert.children()?;
        let [tbs, sig_alg, sig] = parts.as_slice() else { return Err(SignError::Malformed("Certificate".into())) };
        let mut f = tbs.children()?.into_iter().peekable();
        if f.peek().is_some_and(|t| t.tag == tag::ctx(0)) {
            f.next();
        }
        let serial = f.next().ok_or_else(|| SignError::Malformed("serial".into()))?.expect(tag::INTEGER, "serial")?.value.to_vec();
        let _inner_alg = f.next();
        let issuer = Name::parse(&f.next().ok_or_else(|| SignError::Malformed("issuer".into()))?)?;
        let validity = f.next().ok_or_else(|| SignError::Malformed("validity".into()))?.children()?;
        let [nb, na] = validity.as_slice() else { return Err(SignError::Malformed("validity".into())) };
        let subject = Name::parse(&f.next().ok_or_else(|| SignError::Malformed("subject".into()))?)?;
        let public_key = PublicKey::from_spki(&f.next().ok_or_else(|| SignError::Malformed("public key".into()))?)?;
        let mut is_ca = false;
        let mut key_usage = None;
        let mut subject_key_id = None;
        let mut authority_key_id = None;
        let mut extended_key_usage = None;
        let mut ocsp_urls = Vec::new();
        let mut crl_urls = Vec::new();
        for t in f {
            if t.tag != tag::ctx(3) {
                continue;
            }
            for ext in t.inner()?.children()? {
                let e = ext.children()?;
                let Some(o) = e.first().and_then(|o| o.oid().ok()) else { continue };
                let Some(value) = e.last().filter(|v| v.tag == tag::OCTET_STRING) else { continue };
                match o.as_str() {
                    "2.5.29.19" => {
                        is_ca = Tlv::parse_all(value.value)?.children()?.first().is_some_and(|b| b.tag == tag::BOOLEAN && b.value != [0]);
                    }
                    "2.5.29.15" => {
                        let bits = Tlv::parse_all(value.value)?;
                        let v = bits.value;
                        if let Some((_, b)) = v.split_first() {
                            // Bit 0 is the most significant bit of the first byte.
                            let mut u = 0u16;
                            for (i, byte) in b.iter().take(2).enumerate() {
                                for j in 0..8 {
                                    if byte & (0x80 >> j) != 0 {
                                        u |= 1 << (i * 8 + j);
                                    }
                                }
                            }
                            key_usage = Some(u);
                        }
                    }
                    "2.5.29.14" => subject_key_id = Some(Tlv::parse_all(value.value)?.value.to_vec()),
                    // AuthorityKeyIdentifier: the [0] keyIdentifier inside.
                    "2.5.29.35" => {
                        authority_key_id =
                            Tlv::parse_all(value.value)?.children()?.into_iter().find(|t| t.tag == tag::ctx(0)).map(|t| t.value.to_vec());
                    }
                    // ExtendedKeyUsage: a SEQUENCE OF OID.
                    "2.5.29.37" => {
                        let oids: Option<Vec<String>> =
                            Tlv::parse_all(value.value)?.children()?.into_iter().map(|t| t.oid()).collect::<Result<_, _>>().ok();
                        extended_key_usage = oids;
                    }
                    // Authority Information Access: OCSP and CA-issuer locations.
                    "1.3.6.1.5.5.7.1.1" => {
                        for access in Tlv::parse_all(value.value)?.children()? {
                            let a = access.children()?;
                            if a.len() >= 2 && a[0].oid().ok().as_deref() == Some("1.3.6.1.5.5.7.48.1") && a[1].tag == tag::ctx_prim(6) {
                                ocsp_urls.push(String::from_utf8_lossy(a[1].value).into_owned());
                            }
                        }
                    }
                    // CRL distribution points: full-name URIs.
                    "2.5.29.31" => {
                        for dp in Tlv::parse_all(value.value)?.children()? {
                            for field in dp.children()? {
                                if field.tag != tag::ctx(0) {
                                    continue;
                                }
                                // distributionPoint [0] DistributionPointName, whose fullName
                                // choice is [0] IMPLICIT GeneralNames.
                                let Ok(name) = field.inner() else { continue };
                                if name.tag != tag::ctx(0) {
                                    continue;
                                }
                                let Ok(names) = Tlv::parse_all(name.value) else { continue };
                                for n in names.children()? {
                                    if n.tag == tag::ctx_prim(6) {
                                        crl_urls.push(String::from_utf8_lossy(n.value).into_owned());
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        Ok(Certificate {
            raw: raw.to_vec(),
            tbs: tbs.raw.to_vec(),
            sig_alg: sig_alg.raw.to_vec(),
            signature: sig.bits()?.to_vec(),
            serial,
            issuer,
            subject,
            not_before: nb.time()?,
            not_after: na.time()?,
            public_key,
            is_ca,
            key_usage,
            subject_key_id,
            authority_key_id,
            extended_key_usage,
            ocsp_urls,
            crl_urls,
        })
    }

    /// Issued by itself (subject = issuer and its own key verifies it).
    pub fn is_self_signed(&self) -> bool {
        self.issuer.raw == self.subject.raw && self.signed_by(&self.public_key)
    }

    /// Whether `key` verifies this certificate's signature.
    pub fn signed_by(&self, key: &PublicKey) -> bool {
        let Ok(alg) = Tlv::parse_all(&self.sig_alg) else { return false };
        let Ok((scheme, Some(digest))) = keys::signature_algorithm(&alg) else { return false };
        key.verify(scheme, digest, &digest.digest(&[&self.tbs]), &self.signature).unwrap_or(false)
    }

    /// Valid at `t`.
    pub fn valid_at(&self, t: Time) -> bool {
        self.not_before <= t && t <= self.not_after
    }

    /// The display name Acrobat uses: the common name, else the organization, else the DN.
    pub fn display_name(&self) -> String {
        self.subject.common_name().or(self.subject.organization()).map(str::to_string).unwrap_or_else(|| self.subject.display())
    }

    /// SHA-256 fingerprint as upper-case hex pairs.
    pub fn fingerprint(&self) -> String {
        DigestAlg::Sha256.digest(&[&self.raw]).iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
    }

    pub fn serial_hex(&self) -> String {
        self.serial.iter().map(|b| format!("{b:02X}")).collect()
    }

    /// A self-signed certificate for `key`: what Acrobat's "Create a new digital ID ▸ save to
    /// file" makes. Valid from `from` for `years`; usable for signing (digital signature and
    /// non-repudiation).
    pub fn self_signed(name: &Name, key: &PrivateKey, from: Time, years: u32, serial: &[u8]) -> Result<Certificate, SignError> {
        let alg = key.preferred_digest();
        let sig_alg = key.signature_algorithm(alg);
        let until = Time { year: from.year + years, ..from };
        let spki = key.public_key().spki();
        let ski = DigestAlg::Sha1.digest(&[&spki]);
        let ext = |o: &str, critical: bool, value: &[u8]| {
            if critical {
                der::seq(&[&der::oid(o), &der::boolean(true), &der::octets(value)])
            } else {
                der::seq(&[&der::oid(o), &der::octets(value)])
            }
        };
        // keyUsage: digitalSignature (bit 0) and nonRepudiation (bit 1) → 0b1100_0000, 6 unused bits.
        let key_usage = der::tlv(tag::BIT_STRING, &[6, 0b1100_0000]);
        let extensions = der::seq(&[
            &ext("2.5.29.15", true, &key_usage),
            &ext("2.5.29.14", false, &der::octets(&ski)),
            // extKeyUsage: emailProtection and Adobe's document signing (1.2.840.113583.1.1.5).
            &ext("2.5.29.37", false, &der::seq(&[&der::oid("1.3.6.1.5.5.7.3.4"), &der::oid("1.2.840.113583.1.1.5")])),
        ]);
        let tbs = der::seq(&[
            &der::explicit(0, &der::int(2)),
            &der::uint(serial),
            &sig_alg,
            &name.raw,
            &der::seq(&[&from.encode(), &until.encode()]),
            &name.raw,
            &spki,
            &der::explicit(3, &extensions),
        ]);
        let signature = key.sign(alg, &tbs)?;
        let cert = der::seq(&[&tbs, &sig_alg, &der::bit_string(&signature)]);
        Certificate::parse(&cert)
    }
}

/// The chain from `leaf` up through `pool`, as far as issuers can be found and their keys
/// verify the certificate below. Stops at a self-signed certificate.
pub fn build_chain<'a>(leaf: &'a Certificate, pool: &'a [Certificate]) -> Vec<&'a Certificate> {
    let mut chain = vec![leaf];
    while chain.len() < 10 {
        let Some(&last) = chain.last() else { break };
        if last.issuer.raw == last.subject.raw {
            break;
        }
        let Some(issuer) = pool.iter().find(|c| c.subject.raw == last.issuer.raw && !chain.contains(c) && last.signed_by(&c.public_key)) else {
            break;
        };
        chain.push(issuer);
    }
    chain
}

/// Certificates from a file: DER, or PEM with one or more `CERTIFICATE` blocks (`.cer`, `.crt`,
/// `.pem`, Acrobat's `.fdf`-free exports).
pub fn load_certificates(bytes: &[u8]) -> Result<Vec<Certificate>, SignError> {
    if bytes.first() == Some(&0x30) {
        return Ok(vec![Certificate::parse(bytes)?]);
    }
    let text = String::from_utf8_lossy(bytes);
    let mut out = Vec::new();
    let mut block: Option<String> = None;
    for line in text.lines().map(str::trim) {
        if line == "-----BEGIN CERTIFICATE-----" {
            block = Some(String::new());
        } else if line == "-----END CERTIFICATE-----" {
            if let Some(b) = block.take() {
                out.push(Certificate::parse(&base64(&b).ok_or_else(|| SignError::Malformed("PEM base64".into()))?)?);
            }
        } else if let Some(b) = block.as_mut() {
            b.push_str(line);
        }
    }
    if out.is_empty() {
        return Err(SignError::Malformed("no certificate found (expected DER or PEM)".into()));
    }
    Ok(out)
}

/// The certificate as PEM text (Export certificate).
pub fn to_pem(cert: &Certificate) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for c in cert.raw.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            s.push(if i <= c.len() { T[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    let body: Vec<String> = s.as_bytes().chunks(64).map(|l| String::from_utf8_lossy(l).into_owned()).collect();
    format!("-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n", body.join("\n"))
}

fn base64(s: &str) -> Option<Vec<u8>> {
    let val = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    };
    let digits: Vec<u8> = s.bytes().filter(|c| !c.is_ascii_whitespace() && *c != b'=').collect();
    let mut out = Vec::with_capacity(digits.len() * 3 / 4);
    for chunk in digits.chunks(4) {
        let mut n = 0u32;
        for (i, c) in chunk.iter().enumerate() {
            n |= val(*c)? << (18 - 6 * i);
        }
        for i in 0..chunk.len().saturating_sub(1) {
            out.push((n >> (16 - 8 * i)) as u8);
        }
    }
    Some(out)
}
