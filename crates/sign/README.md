# pdfcraft-sign

Layer L4: digital signatures (execution plan M9; ISO 32000-2 §12.8; PAdES, ETSI EN 319 142).

```rust
let id = pkcs12::open(&std::fs::read("me.p12")?, "password")?;          // a digital ID
let signed = sign(&doc, &id, &SignOptions { page: 0, rect: Some(r), date, ..Default::default() })?;
for s in signatures(&doc, &bytes, &trust) {                                // list + validate
    println!("{}: {} — {:?}", s.field, s.summary(), s.details);
}
```

- **Formats, on RustCrypto primitives:** a small DER reader and writer (`der`), X.509
  certificates (`x509`), CMS SignedData (`cms`), and PKCS #12 digital ID files (`pkcs12`):
  PBES2 (PBKDF2 + AES/3DES), the legacy SHA-1 3DES/RC2 schemes, and MAC checks. Writing uses
  OpenSSL 3's defaults (AES-256-CBC, HMAC-SHA-256).
- **Keys:** RSA, ECDSA P-256 and P-384. Verification uses RustCrypto everywhere. RSA
  private-key operations (signing, key generation) use `aws-lc-rs` on native targets and are
  refused in the browser (ADR-0009: the `rsa` crate's Marvin advisory, RUSTSEC-2023-0071, has
  no fix, so `rsa` is only used to verify). ECDSA signs with deterministic nonces (RFC 6979).
- **Signing:** PAdES B-B (`ETSI.CAdES.detached`, signing-certificate-v2, SHA-256/384, no SHA-1).
  An existing unsigned field or a new one (visible with Acrobat's name-and-details appearance,
  or invisible); certification with DocMDP P=1/2/3. The document is written incrementally
  with a zero-filled `/Contents` and fixed-width `/ByteRange`, which are then patched in
  place. Encrypted documents are refused for now.
- **Timestamps (PAdES B-T):** the `timestamp` module builds and parses RFC 3161 requests,
  responses and TSTInfo tokens (size-capped, imprint- and signature-checked). `sign_with_timestamp`
  attaches the TSA's token as an unsigned attribute over the signature value; the transport is
  the caller's (`TimestampAuthority` — this crate never opens a socket). Validation verifies
  embedded tokens; when the TSA chains to the trust store it reports the trusted time in
  `SignatureInfo::timestamp_time`, which then anchors certificate-validity and revocation
  checks. An untrusted TSA's time is reported as an unverified timestamp and never used as the
  validation time (the signer's claimed time is). `timestamp::respond` is the TSA-side signer behind the
  deterministic test authority.
- **Document timestamps and LTV:** `timestamp_document` appends a standalone RFC 3161
  document timestamp (`/ETSI.RFC3161`) covering the whole file; validation discovers and
  verifies these dictionaries (imprint over the signed bytes, token signature, TSA cert
  validity). `dss::embed` merges revocation evidence into the catalog's `/DSS` with `/VRI`
  entries keyed per signature (uppercase-hex SHA-1 of `/Contents`), deduplicating
  byte-identical blobs — sign → DSS → timestamp makes a B-LTA file, and the change classifier
  treats the store as a permitted change. `revocation` parses and verifies RFC 5280 CRLs and
  RFC 6960 OCSP responses (responder identity, OCSP-signing EKU for delegated responders,
  CertID hash matching, validity windows); validation checks embedded evidence against the
  signer's chain and a verified revocation invalidates the signature.

Not yet: revocation fetching (AIA/CRLDP extraction and a fetcher), timestamp-server
configuration, FieldMDP locks, certificate security, smart cards and PKCS #11 tokens.
macOS Keychain and Windows Current User Personal (My) store identities sign through
`ExternalKey` without exporting private keys. Windows CNG supports RSA PKCS #1 v1.5 and
ECDSA P-256/P-384; the store integration is tested with software-backed keys.
- **Validation:** `/ByteRange` and the CMS are read from the file's own bytes; the digest,
  the signature value and the signer's chain (against a `TrustStore`) are checked. Later
  revisions are diffed against the signed one, and the changes are classified (signing, form
  fill, comments, metadata, page content, document structure) under the DocMDP permissions.
  The verdict follows Acrobat: valid, unknown (intact but the identity isn't trusted) or invalid.
- **Chains are checked, not just linked.** `x509::build_chain` only takes an issuer whose key
  verifies the certificate below it *and* that may issue: `basicConstraints` CA:TRUE (an old v1
  self-signed root without constraints counts), `keyCertSign` when it has a key usage, a
  `pathLenConstraint` that allows the CAs below it, and, given the signing time, validity then.
  Certificates embedded in a document are in the pool too, so an ordinary subscriber certificate
  can never vouch for another one; when an issuer is refused, the signature's details say why.

Oracles: poppler's `pdfsig` reports our signatures valid; OpenSSL reads our `.p12` files and
verifies our CMS; `tests/data/openssl-signed.pdf` is a signature OpenSSL made, which we validate.
