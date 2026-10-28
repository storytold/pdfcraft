# PR Proposal 25: Decode JavaScript Hex and Control Escapes in AF Script Strings

**Issue Reference:** #3623
**Crates Affected:** `crates/forms`

## Summary
The AcroForm AF* script parser (`crates/forms/src/af.rs`, `Parser::string`) read JavaScript `\xHH` escapes as literal text (e.g. `\x24` became `x24` instead of `$`). Control escapes like `\b`, `\f`, `\v`, and `\0` also dropped their leading backslash.

## Implementation Details
- In `crates/forms/src/af.rs`:
  - Added bounded 2-digit hex decoder `hex2(bytes: &[u8]) -> Option<u8>`.
  - In `Parser::string`, decoded `\xHH` into Latin-1/UTF-8 codepoints, and handled `\b` (`0x08`), `\f` (`0x0C`), `\v` (`0x0B`), and `\0` (`0x00`).
  - Added regression test `af::tests::javascript_escapes_are_decoded`.
