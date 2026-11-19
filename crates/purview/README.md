# pdfcraft-purview

Microsoft Purview Information Protection integration for PdfCraft.

The crate is intentionally split into a platform-neutral Rust policy model and a native MIP
SDK adapter. Ordinary builds do not ship or link the Microsoft SDK. Builds that enable the
`mip-sdk` feature require an official MIP SDK 1.18.x-compatible package and an approved Entra
public-client application registration.

The engine and automation layers use this crate's rights model. They never independently
decrypt files or strip protection.

## Native and dependency boundary

The MIP implementation uses Microsoft's native MIP SDK C++ library through a native helper.
It does not shell out to Azure CLI, require `az login`, reuse Azure CLI tokens, or use a
third-party MIP SDK wrapper. The Rust layer is limited to authentication state, PKCE browser
callback, profile/tenant JSON decoding, staging and the rights policy seam.

Official SDK packages are used for macOS arm64/x86_64, Windows amd64/x86/arm64 where
supported, and supported Linux distributions/architectures. The helper and SDK redistributables
are staged and signed only in explicitly configured SDK builds.

The least auth dependencies are the PKCE/OAuth implementation, an HTTPS client, browser
launcher, JSON decoding, URL parsing and the existing workspace platform helper on Windows.
UUID and generic Base64 crates are not runtime dependencies; app-ID validation and ID-token
Base64URL decoding are bounded local implementation details.
