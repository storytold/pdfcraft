//! Comprehensive Integration Tests for Bundle 8: PDF/UA Remediation, Action Wizard, Universal CLI, and Platform Readiness.

use pdfcraft_pdf_ua_remediation::{AutoTagClassifier, ContentBlock, PdfUaValidator, StandardStructureTag, TableHeaderScope, TableRemediator};
use pdfcraft_action_wizard::{ActionRecipe, ActionStep, ActionWizardRunner};
use pdfcraft_universal_cli::CommandRegistry;
use pdfcraft_platform_readiness::{FluentLocalizationCatalog, KeyboardFocusManager, PerformanceBudgetWatcher, PreferencesManager};
use std::collections::HashMap;

#[test]
fn test_pdf_ua_remediation_and_autotagging() {
    // 1. Validator checkpoint checking
    let doc_str = "%PDF-1.7\n1 0 obj\n<< /Type /Catalog /Lang (en-US) /MarkInfo << /Marked true >> >>\nendobj\n%%EOF";
    let violations = PdfUaValidator::validate_document(doc_str);
    // Lang and Marked are present, only Title is flagged
    assert!(!violations.iter().any(|v| v.checkpoint == "01-001"));
    assert!(!violations.iter().any(|v| v.checkpoint == "02-001"));

    // 2. Auto-tag classification
    let heading_block = ContentBlock {
        text: "Chapter 1: Quantum Computing".into(),
        font_size: 24.0,
        is_bold: true,
        bbox: [50.0, 700.0, 300.0, 730.0],
        is_running_header_or_footer: false,
    };
    let tag = AutoTagClassifier::classify_block(&heading_block, 12.0);
    assert_eq!(tag, StandardStructureTag::H1);

    // 3. Table /TH scope remediation
    let th_raw = "<< /Type /StructElem /S /TH >>";
    let th_fixed = TableRemediator::remediate_header_cell(th_raw, TableHeaderScope::Row);
    assert!(th_fixed.contains("/Scope /Row"));
}

#[test]
fn test_action_wizard_batch_runner() {
    let recipe = ActionWizardRunner::recipe_archive_documents();
    let files = vec!["contracts/contract_01.pdf".into(), "contracts/contract_02.pdf".into()];
    let results = ActionWizardRunner::run_recipe_over_files(&recipe, &files);
    assert_eq!(results.len(), 2);
    assert!(results[0].success);
    assert_eq!(results[0].steps_completed, 3);
}

#[test]
fn test_universal_cli_full_coverage() {
    let registry = CommandRegistry::new();
    assert!(registry.get("convert").is_some());
    assert!(registry.get("ocr").is_some());
    assert!(registry.get("sign").is_some());
    assert!(registry.get("redact").is_some());
    assert!(registry.get("preflight").is_some());
    assert!(registry.get("optimize").is_some());
    assert!(registry.get("split").is_some());
    assert!(registry.get("merge").is_some());
    assert!(registry.get("a11y-check").is_some());
    assert!(registry.get("bates").is_some());

    let mut sign_args = HashMap::new();
    sign_args.insert("input".into(), "doc.pdf".into());
    let res = registry.dispatch("sign", &sign_args).unwrap();
    assert!(res.contains("\"status\": \"ok\""));
}

#[test]
fn test_platform_readiness_keyboard_budget_preferences() {
    // 1. Keyboard focus cycle without trap
    let mut focus = KeyboardFocusManager::new();
    focus.register_control(10, "First", 1);
    focus.register_control(20, "Second", 2);
    assert_eq!(focus.on_tab_pressed(), Some(10));
    assert_eq!(focus.on_tab_pressed(), Some(20));
    assert_eq!(focus.on_tab_pressed(), Some(10)); // wrap

    // 2. Budget limits
    let watcher = PerformanceBudgetWatcher::default();
    assert!(watcher.check_render_duration(35)); // 35ms <= 50ms budget
    assert!(!watcher.check_render_duration(75)); // 75ms > 50ms budget
    assert!(watcher.check_heap_usage(120));      // 120MB <= 150MB budget

    // 3. Preferences schema
    let mut prefs = PreferencesManager::new();
    assert_eq!(prefs.get_bool("display.smooth_text"), Some(true));
    prefs.set_bool("display.smooth_text", false);
    assert_eq!(prefs.get_bool("display.smooth_text"), Some(false));

    // 4. Fluent localization
    let i18n = FluentLocalizationCatalog::new();
    assert_eq!(i18n.get_message("de", "btn-save"), "Speichern");
    assert_eq!(i18n.get_message("fr", "btn-save"), "Enregistrer");
}
