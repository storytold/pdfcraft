# Signing test data

Test-only keys and certificates, generated for these tests with OpenSSL 3. They protect nothing.
The password of every `.p12` is `test`.

| File | What |
|---|---|
| `rsa-aes.p12` | RSA 2048 self-signed "Test Signer RSA"; PBES2 AES-256-CBC, SHA-256 MAC (OpenSSL 3 default) |
| `rsa-legacy.p12` | the same key and certificate; RC2-40 certificates, 3DES key, SHA-1 MAC (`-legacy`) |
| `ec-p256.p12` | P-256 self-signed "Test Signer EC", no friendly name |
| `ec-legacy.p12` | `ec-p256.p12` re-exported with `-legacy` (3DES, SHA-1 MAC): the format macOS `security import` accepts |
| `ec-p384.p12` | P-384 self-signed "Test Signer P384"; AES-128-CBC, SHA-1 MAC |
| `chain.p12` | RSA 2048 "Ada Lovelace" issued by a P-256 test root, chain included |
| `rsa.crt.pem`, `ca.crt.pem` | the RSA certificate and the test root alone |

Regenerate with:

```sh
openssl req -x509 -newkey rsa:2048 -nodes -keyout rsa.key -out rsa.crt -days 3650 -subj "/CN=Test Signer RSA/O=PrintCraft Tests/C=US" -set_serial 1001
openssl pkcs12 -export -inkey rsa.key -in rsa.crt -out rsa-aes.p12 -passout pass:test -name "Test Signer RSA"
openssl pkcs12 -export -legacy -inkey rsa.key -in rsa.crt -out rsa-legacy.p12 -passout pass:test -name "Test Signer RSA"
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -keyout ec.key -out ec.crt -days 3650 -subj "/CN=Test Signer EC/O=PrintCraft Tests" -set_serial 2002
openssl pkcs12 -export -inkey ec.key -in ec.crt -out ec-p256.p12 -passout pass:test
openssl pkcs12 -export -legacy -inkey ec.key -in ec.crt -out ec-legacy.p12 -passout pass:test -name "Test Signer EC"
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-384 -nodes -keyout ec3.key -out ec3.crt -days 3650 -subj "/CN=Test Signer P384" -set_serial 3003
openssl pkcs12 -export -inkey ec3.key -in ec3.crt -out ec-p384.p12 -passout pass:test -certpbe AES-128-CBC -keypbe AES-128-CBC -macalg sha1
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -keyout ca.key -out ca.crt -days 3650 -subj "/CN=PrintCraft Test Root CA/O=PrintCraft Tests" -set_serial 1 -addext "basicConstraints=critical,CA:TRUE" -addext "keyUsage=critical,keyCertSign,cRLSign"
openssl req -newkey rsa:2048 -nodes -keyout leaf.key -out leaf.csr -subj "/CN=Ada Lovelace/O=PrintCraft Tests/emailAddress=ada@example.com"
printf "basicConstraints=CA:FALSE\nkeyUsage=critical,digitalSignature,nonRepudiation\n" > leaf.ext
openssl x509 -req -in leaf.csr -CA ca.crt -CAkey ca.key -set_serial 77 -days 3650 -extfile leaf.ext -out leaf.crt
openssl pkcs12 -export -inkey leaf.key -in leaf.crt -certfile ca.crt -out chain.p12 -passout pass:test -name "Ada Lovelace"
```

`openssl-signed.pdf` is a hand-written one-page PDF with a `/Contents` placeholder, signed with
`openssl cms -sign -binary -md sha256 -outform DER -signer rsa.crt -inkey rsa.key` over its
byte ranges (`adbe.pkcs7.detached`, with OpenSSL's signing-time attribute). It checks the
validator against a signature PdfCraft did not make; poppler's `pdfsig` reports it valid.

### `x509-rsa-sha1.pdf`

A synthetic one-page PDF signed the legacy way (`/SubFilter /adbe.x509.rsa_sha1`, `/Cert` = `rsa.crt.pem`,
`/Contents` = a DER OCTET STRING with the PKCS #1 signature of the SHA-1 digest of the byte ranges), with
the key of `rsa-aes.p12`. Built by writing the objects with a fixed-width `/ByteRange` and zero-filled
`/Contents`, patching the byte range, then `openssl dgst -sha1 -sign rsa.key` over the two ranges.

### RSA signature variants (hex constants in `tests/crypto.rs`)

Over SHA-256("hello") with the key of `rsa-aes.p12`:
`openssl dgst -sha256 -sign rsa.key` (standard DigestInfo); `openssl pkeyutl -sign -pkeyopt rsa_padding_mode:pkcs1`
over a hand-built DigestInfo without the NULL parameter, and over the bare digest;
`openssl dgst -sha256 -sigopt rsa_padding_mode:pss -sigopt rsa_pss_saltlen:0 -sign rsa.key`.

### Other algorithms (hex constants in `tests/algorithms.rs`)

All sign the message `hello` with throw-away keys, OpenSSL 3.6. RSA ones use the key of `rsa-aes.p12`.

```sh
cms() { openssl cms -sign -binary -in msg -outform DER "$@"; }
cms -md sha3-256   -signer rsa.crt -inkey rsa.key -out c_rsa_sha3_256.der
cms -md sha512-256 -signer rsa.crt -inkey rsa.key -out c_rsa_sha512_256.der
cms -provider legacy -provider default -md ripemd160 -signer rsa.crt -inkey rsa.key -out c_rsa_ripemd160.der
cms -md sha256 -signer rsa.crt -inkey rsa.key -keyopt rsa_padding_mode:pss -keyopt rsa_mgf1_md:sha1 -keyopt rsa_pss_saltlen:20 -out c_pss_sha256_mgf1sha1_salt20.der
cms -md sha384 -signer rsa.crt -inkey rsa.key -keyopt rsa_padding_mode:pss -keyopt rsa_mgf1_md:sha256 -keyopt rsa_pss_saltlen:0 -out c_pss_sha384_mgf1sha256_salt0.der
openssl req -x509 -newkey ed25519 -nodes -keyout ed.key -out ed.crt -subj "/CN=Test Signer Ed25519"
cms -md sha512 -signer ed.crt -inkey ed.key -out c_ed25519.der
openssl ecparam -name brainpoolP512r1 -genkey -noout -out bp512.key
openssl req -x509 -new -key bp512.key -sha512 -out bp512.crt -subj "/CN=Test Signer BP512"
cms -md sha512   -signer bp512.crt -inkey bp512.key -out c_bp512_sha512.der
cms -md sha3-256 -signer bp512.crt -inkey bp512.key -out c_bp512_sha3_256.der
openssl dgst -sha3-256   -sigopt rsa_padding_mode:pss -sigopt rsa_pss_saltlen:32 -sign rsa.key -out pss_sha3_256.sig msg
openssl dgst -sha512-256 -sigopt rsa_padding_mode:pss -sigopt rsa_pss_saltlen:0  -sign rsa.key -out pss_sha512_256.sig msg
```
