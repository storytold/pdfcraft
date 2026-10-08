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
