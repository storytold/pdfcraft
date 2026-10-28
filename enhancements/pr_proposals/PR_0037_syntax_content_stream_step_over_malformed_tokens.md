# PR Proposal 0037: Step Over Malformed Tokens and Overlong Operands in Content Streams

- **PR ID**: `PR-0037`
- **Issue Reference**: Fixes #3568, #3636
- **Severity**: High (Parser Resilience / Data Loss Prevention)
- **Component**: `vendor/hayro-syntax/src/content/mod.rs`

## Description
When encountering a malformed token (e.g. malformed name sequence `/C#2`), a stray delimiter (e.g. a stray closing parenthesis `)` outside string context), or an instruction with more operands than the stack capacity (e.g., 11 operands for `g`), the content stream iterator previously called `jump_to_end()` or returned `None`, aborting parsing for the entire remainder of the page. This caused legitimate text, images, and vector artwork following the malformed token to vanish from rendering and text extraction.

## Resolution
- In `UntypedIter::next`:
  - When `stack.push(object)` fails due to exceeding the operand limit, clear the stack and drop the overlong instruction rather than aborting the stream.
  - When an unreadable token begins with `/` (malformed name), consume the token up to the next delimiter or whitespace and continue.
  - When an unreadable operator is encountered (e.g. stray delimiter), step over the single byte and continue rather than calling `jump_to_end()`.
- Added unit test `steps_over_malformed_tokens_and_stray_delimiters` in `vendor/hayro-syntax/src/content/mod.rs`.
