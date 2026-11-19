# pdfcraft-purview

Microsoft Purview Information Protection integration for PdfCraft.

The crate is intentionally split into a platform-neutral Rust policy model and a native MIP
SDK adapter. Ordinary builds do not ship or link the Microsoft SDK. Builds that enable the
`mip-sdk` feature require an official MIP SDK 1.18.x-compatible package and an approved Entra
public-client application registration.

The engine and automation layers use this crate's rights model. They never independently
decrypt files or strip protection.
