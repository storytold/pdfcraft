use super::*;
use proptest::prelude::*;

/// Supply a valid baseline so each test can isolate one arithmetic or validation condition.
fn draft() -> Draft {
    Draft {
        number: "RE-2026-001".into(),
        issue_date: "2026-10-23".into(),
        currency: "EUR".into(),
        seller: Party { name: "Example Seller".into(), country_code: "DE".into() },
        buyer: Party { name: "Example Buyer".into(), country_code: "DE".into() },
        lines: vec![line("1", "100", "19")],
    }
}

/// Construct a standard-rated line from exact decimal literals owned by the test.
fn line(quantity: &str, price: &str, rate: &str) -> Line {
    Line { description: "Service".into(), quantity: quantity.parse().unwrap(), unit_price: price.parse().unwrap(), vat_rate: rate.parse().unwrap() }
}

/// Pin mixed-rate results and verify the calculator leaves its input unchanged.
#[test]
fn mixed_rates_are_grouped_with_exact_line_and_tax_totals() {
    let mut input = draft();
    input.lines = vec![line("2.5", "12.3456", "19"), line("1", "100", "7"), line("3", "0.1", "19")];
    let original = input.clone();
    let report = calculate(&input);
    assert!(report.issues.is_empty());
    assert_eq!(report.scope, "draft_arithmetic_only");
    assert_eq!(report.standards_validation, "not_performed");
    let totals = report.totals.unwrap();
    assert_eq!(totals.line_net_amounts, ["30.86", "100.00", "0.30"]);
    assert_eq!(totals.net_amount, "131.16");
    assert_eq!(totals.tax_amount, "12.92");
    assert_eq!(totals.payable_amount, "144.08");
    assert_eq!(totals.tax_breakdown[0].vat_rate.to_string(), "7");
    assert_eq!(totals.tax_breakdown[1].taxable_amount, "31.16");
    assert_eq!(totals.tax_breakdown[1].tax_amount, "5.92");
    assert_eq!(input, original);
}

/// Distinguish group VAT rounding from per-line VAT rounding at half-cent boundaries.
#[test]
fn rounding_is_half_up_per_line_and_then_per_tax_group() {
    let mut input = draft();
    input.lines = vec![line("1", "0.025", "19"), line("1", "0.025", "19")];
    let totals = calculate(&input).totals.unwrap();
    assert_eq!(totals.line_net_amounts, ["0.03", "0.03"]);
    // Rounding VAT per line would incorrectly yield 0.02, not 0.01.
    assert_eq!(totals.net_amount, "0.06");
    assert_eq!(totals.tax_amount, "0.01");
    assert_eq!(totals.payable_amount, "0.07");
    input.lines = vec![line("0.000001", "5000", "19")];
    assert_eq!(calculate(&input).totals.unwrap().net_amount, "0.01");
}

/// Report every invalid field while withholding a misleading partial total.
#[test]
fn unsupported_drafts_return_field_paths_and_no_totals() {
    let mut input = draft();
    input.number.clear();
    input.issue_date = "2026-02-29".into();
    input.currency = "JPY".into();
    input.buyer.country_code = "FR".into();
    input.seller.name = "bad\u{0}".into();
    input.lines[0] = line("0", "10", "0");
    let report = calculate(&input);
    assert!(report.totals.is_none());
    for path in ["number", "issue_date", "currency", "buyer.country_code", "seller.name", "lines[0].quantity", "lines[0].vat_rate"] {
        assert!(report.issues.iter().any(|i| i.path == path), "{path}: {report:?}");
    }
}

/// Reject noncanonical dates, oversized UTF-8 text and unsupported VAT precision.
#[test]
fn calendar_dates_text_limits_and_rate_precision_are_checked() {
    let mut input = draft();
    for date in ["2026-1-01", "2026-01-1", "2026-1-023", "0000-01-01", "2026-13-01", "2026-01-01 "] {
        input.issue_date = date.into();
        assert!(calculate(&input).issues.iter().any(|i| i.path == "issue_date"), "{date}");
    }
    input.issue_date = "2024-02-29".into();
    assert!(calculate(&input).issues.is_empty());
    input.lines[0].description = "ä".repeat(MAX_TEXT_BYTES / 2 + 1);
    assert!(calculate(&input).issues.iter().any(|i| i.code == "text_limit"));
    input.lines[0].description = "Service".into();
    for rate in ["0", "100.01", "19.001"] {
        input.lines[0].vat_rate = rate.parse().unwrap();
        assert!(calculate(&input).totals.is_none());
    }
}

/// Exercise both line-count boundaries and the largest accepted numeric values.
#[test]
fn line_limits_and_largest_supported_inputs_are_bounded() {
    let mut input = draft();
    input.lines.clear();
    assert!(calculate(&input).totals.is_none());
    input.lines = vec![line("999999999999.999999", "999999999999.999999", "100"); MAX_LINES];
    let report = calculate(&input);
    assert!(report.issues.is_empty(), "{report:?}");
    assert_eq!(report.totals.as_ref().unwrap().line_net_amounts.len(), MAX_LINES);
    input.lines.push(line("1", "1", "19"));
    let report = calculate(&input);
    assert!(report.totals.is_none());
    assert_eq!(report.issues.len(), 1);
    assert_eq!(report.issues[0].code, "line_limit");
}

/// Ensure unimplemented financial terms cannot disappear during deserialization.
#[test]
fn unknown_financial_fields_are_refused_at_every_model_level() {
    let value = serde_json::to_value(draft()).unwrap();
    for location in ["draft", "seller", "line"] {
        let mut altered = value.clone();
        let target = match location {
            "seller" => &mut altered["seller"],
            "line" => &mut altered["lines"][0],
            _ => &mut altered,
        };
        target["discount"] = serde_json::json!("5");
        assert!(serde_json::from_value::<Draft>(altered).is_err());
    }
}

proptest! {
    /// Compare whole-euro cases with an independent integer-cent expectation.
    #[test]
    fn integer_euro_lines_conserve_money(quantity in 1_u32..100_000, euros in 0_u32..100_000, rate in 1_u32..100) {
        let mut input = draft();
        input.lines = vec![line(&quantity.to_string(), &euros.to_string(), &rate.to_string())];
        let totals = calculate(&input).totals.unwrap();
        let net = u128::from(quantity) * u128::from(euros) * 100;
        let tax = (net * u128::from(rate) + 50) / 100;
        prop_assert_eq!(totals.net_amount, money(net));
        prop_assert_eq!(totals.tax_amount, money(tax));
        prop_assert_eq!(totals.payable_amount, money(net + tax));
    }
}
