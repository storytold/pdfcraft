# Bundle 8: PDF/UA Accessibility Remediation, Action Wizard & Universal CLI

## Overview
Bundle 8 implements ISO 14289 PDF/UA 32-rule accessibility verification and remediation, structural auto-tagging, table `/TH` scope remediation, the Action Wizard multi-step batch automation runner, 100% universal CLI command coverage, WCAG 2.1 focus trap prevention keyboard management, performance budget enforcement, and Fluent localization, directly targeting **Milestone M12** (Accessibility & Remediation), **Milestone M13** (Automation & Batch), and **Milestone M14** (1.0 Production Readiness).

---

## Parity Gaps Addressed

### Milestone M12 (Accessibility & Remediation)
* `[P1]` **Matterhorn Protocol PDF/UA 32-Rule Validation:** Validates tagged document requirements, natural language specification, document titles, image alternative text (`/Alt`), and table scopes.
* `[P1]` **Automatic Tagging & Artifact Marking:** Heuristic structural block classifier tagging `H1`, `H2`, `P`, and marking running headers/footers as `/Artifact`.
* `[P1]` **Table Remediation:** Automatic injection of `/Scope /Row` or `/Scope /Column` into `/TH` header cells.

### Milestone M13 (Automation & Batch)
* `[P0]` **CLI Covers Every Shipped Command:** Universal command registry exposing 100% of engine commands (`convert`, `ocr`, `sign`, `redact`, `preflight`, `optimize`, `split`, `merge`, `a11y-check`, `bates`) with JSON-RPC output.
* `[P1]` **Action Wizard Batch Runner:** Multi-step batch processing recipes ("Make Accessible", "Archive Documents", multi-file optimization) across folder hierarchies.

### Milestone M14 (1.0 Production Readiness & Platforms)
* `[P0]` **Full Keyboard Operation & Focus Trap Prevention:** WCAG 2.1 keyboard focus cycling (Tab, Shift-Tab, Esc) preventing focus traps across all modal dialogs.
* `[P0]` **Platform Performance Budgets:** Strictly limits page rasterization latency to <50ms and heap footprint to <150MB.
* `[P1]` **Schema-Driven Preferences:** Typed configuration model for general, display, security, and accessibility options.
* `[P1]` **Fluent Localization:** Multilingual UI message catalog.

---

## Crate Layout

```
bundle_8_accessibility_action_wizard_and_cli/
├── Cargo.toml
├── README.md
├── crates/
│   ├── pdf_ua_remediation/ # ISO 14289 32-rule validator, auto-tagger & table remediator
│   ├── action_wizard/      # Batch processing recipe runner & "Make Accessible" wizard
│   ├── universal_cli/      # 100% command registry CLI interface with JSON-RPC output
│   └── platform_readiness/ # Focus manager, performance budget watcher & Fluent i18n
└── tests/
    └── integration_tests.rs
```
