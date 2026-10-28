# Bundle 5: Enterprise Security, PKCS#11 Signatures & PAdES LTV

## Overview
Bundle 5 implements public-key certificate encryption (§7.6.5), per-recipient permissions, PKCS#11 v2.40 hardware smart card digital signatures, ETSI EN 319 142 PAdES Long-Term Validation (LTV) with `/DSS` Document Security Store embedding, RFC 3161 timestamps, and per-revision visual rollback inspection, directly targeting **Milestone M8** (Protection & Redaction) and **Milestone M9** (Digital Signatures & LTV).

---

## Parity Gaps Addressed

### Milestone M8 (Protection & Security)
* `[P1]` **Open Certificate-Encrypted Documents:** Public-key security handler (§7.6.5) decrypting Document File Encryption Keys from recipient CMS `EnvelopedData` structures.
* `[P1]` **Per-Recipient Permissions:** Independent permission masks for each certificate recipient (high-res printing, text extraction, form filling).

### Milestone M9 (Digital Signatures & LTV)
* `[P0]` **Signatures Panel with Per-Revision Tree:** Multi-revision `%%EOF` scanner extracting historical document snapshots as they existed at the moment each signature was created.
* `[P0]` **Signature Properties Summary:** Cryptographic integrity and validity summary (digest integrity, trust anchors, expiration, revocation status).
* `[P1]` **PKCS#11 Smart Card & Hardware Signing:** OASIS PKCS#11 v2.40 session and hardware token manager for USB crypto tokens and smart cards without exposing private keys.
* `[P1]` **RFC 3161 Timestamping & PAdES LTV:** Document Timestamp (`/DocTimeStamp`) generation, `/DSS` Document Security Store embedding, and OCSP/CRL revocation caching for offline verification.
* `[P1]` **FieldMDP Locks:** Document and field locking policies (`/Lock << /Action /All >>`) preventing post-signing alterations.

---

## Crate Layout

```
bundle_5_security_pkcs11_and_pades_ltv/
├── Cargo.toml
├── README.md
├── crates/
│   ├── public_key_crypt/   # Public-key security handler & CMS EnvelopedData
│   ├── pades_ltv/          # PAdES B-LT/B-LTA, /DSS builder, RFC 3161 timestamps
│   ├── pkcs11_bridge/      # OASIS PKCS#11 Cryptoki hardware token interface
│   └── signature_history/  # Per-revision rollback tree, /ByteRange validator & FieldMDP
└── tests/
    └── integration_tests.rs
```
