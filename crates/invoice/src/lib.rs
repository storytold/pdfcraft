//! Invoice draft foundation (L4): exact arithmetic and actionable draft diagnostics.
//!
//! This is deliberately a small EUR/DE, positive-quantity, standard-rated subset.
//! It does not validate EN 16931, XRechnung or ZUGFeRD and does not create invoices
//! in those formats. A successful calculation must never be presented as conformance.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod decimal;

use std::collections::BTreeMap;

use chrono::NaiveDate;
use decimal::{SCALE, money, rounded};
use serde::{Deserialize, Serialize};

pub use decimal::Decimal;

/// A draft is capped before deriving per-line results.
pub const MAX_LINES: usize = 1000;
/// Maximum UTF-8 byte length of any draft text field.
pub const MAX_TEXT_BYTES: usize = 1024;

/// Minimum party identity for arithmetic previews, not a complete invoice address.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Party {
    pub name: String,
    pub country_code: String,
}

/// One standard-rated line. Exemptions, reverse charge, credits and adjustments
/// need separate semantic support and are intentionally not inferred from a zero rate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Line {
    pub description: String,
    pub quantity: Decimal,
    pub unit_price: Decimal,
    /// Percentage, e.g. `"19"`, with at most two significant fractional places.
    pub vat_rate: Decimal,
}

/// Serializable input shared by future GUI, format writers and headless callers.
/// Unknown fields are refused so unsupported financial terms cannot disappear.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Draft {
    pub number: String,
    /// Strict `YYYY-MM-DD` calendar date.
    pub issue_date: String,
    pub currency: String,
    pub seller: Party,
    pub buyer: Party,
    pub lines: Vec<Line>,
}

/// A field-level diagnostic. Codes describe draft checks, not official business rules.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Issue {
    pub path: String,
    pub code: &'static str,
    pub message: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TaxTotal {
    pub vat_rate: Decimal,
    pub taxable_amount: String,
    pub tax_amount: String,
}

/// All amounts are decimal strings in EUR with exactly two fractional places.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Totals {
    pub currency: &'static str,
    pub line_net_amounts: Vec<String>,
    pub tax_breakdown: Vec<TaxTotal>,
    pub net_amount: String,
    pub tax_amount: String,
    pub payable_amount: String,
}

/// A preview never certifies an invoice. Invalid or unsupported drafts have no totals.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Report {
    pub scope: &'static str,
    pub standards_validation: &'static str,
    pub issues: Vec<Issue>,
    pub totals: Option<Totals>,
}

fn issue(issues: &mut Vec<Issue>, path: impl Into<String>, code: &'static str, message: &'static str) {
    issues.push(Issue { path: path.into(), code, message });
}

fn text(issues: &mut Vec<Issue>, path: &str, value: &str) {
    if value.len() > MAX_TEXT_BYTES {
        issue(issues, path, "text_limit", "Text exceeds 1024 UTF-8 bytes.");
    } else if value.trim().is_empty() {
        issue(issues, path, "required", "Provide a non-empty value.");
    } else if value.chars().any(char::is_control) {
        issue(issues, path, "control_character", "Remove control characters from this field.");
    }
}

/// Validate the supported draft subset and calculate an immutable arithmetic preview.
///
/// Positive quantities, non-negative prices and VAT rates above zero up to 100%
/// are supported. Each line net is rounded half-up to cents; rounded line nets
/// are grouped by rate, then each group's tax is rounded half-up to cents.
/// Zero-rated/exempt/reverse-charge lines and non-DE parties are unsupported.
/// No external resources are loaded and the draft is never modified.
pub fn calculate(draft: &Draft) -> Report {
    let mut report = Report { scope: "draft_arithmetic_only", standards_validation: "not_performed", issues: Vec::new(), totals: None };
    let issues = &mut report.issues;
    text(issues, "number", &draft.number);
    let date = &draft.issue_date;
    if date.len() != 10
        || date.starts_with("0000")
        || !NaiveDate::parse_from_str(date, "%Y-%m-%d").is_ok_and(|parsed| parsed.format("%Y-%m-%d").to_string() == *date)
    {
        issue(issues, "issue_date", "invalid_date", "Use a real calendar date in YYYY-MM-DD form.");
    }
    if draft.currency != "EUR" {
        issue(issues, "currency", "unsupported_currency", "This draft calculator supports EUR only.");
    }
    for (path, party) in [("seller", &draft.seller), ("buyer", &draft.buyer)] {
        text(issues, &format!("{path}.name"), &party.name);
        if party.country_code != "DE" {
            issue(issues, format!("{path}.country_code"), "unsupported_country", "This draft calculator supports DE parties only.");
        }
    }
    if draft.lines.is_empty() || draft.lines.len() > MAX_LINES {
        issue(issues, "lines", "line_limit", "Provide between 1 and 1000 lines.");
        return report;
    }
    for (index, line) in draft.lines.iter().enumerate() {
        let path = format!("lines[{index}]");
        text(issues, &format!("{path}.description"), &line.description);
        if line.quantity.0 == 0 {
            issue(issues, format!("{path}.quantity"), "positive_quantity", "Quantity must be greater than zero; credit notes are not supported yet.");
        }
        if line.vat_rate.0 == 0 || line.vat_rate.0 > 100 * SCALE || !line.vat_rate.0.is_multiple_of(10_000) {
            issue(
                issues,
                format!("{path}.vat_rate"),
                "unsupported_vat_rate",
                "Use a standard-rated percentage above 0 and up to 100 with at most 2 decimal places; tax exemptions are not supported yet.",
            );
        }
    }
    if issues.is_empty() {
        match totals(draft) {
            Some(totals) => report.totals = Some(totals),
            None => issue(&mut report.issues, "lines", "arithmetic_overflow", "The calculation exceeds the supported numeric range."),
        }
    }
    report
}

fn totals(draft: &Draft) -> Option<Totals> {
    let mut line_net_amounts = Vec::with_capacity(draft.lines.len());
    let mut groups: BTreeMap<Decimal, u128> = BTreeMap::new();
    let mut net = 0_u128;
    for line in &draft.lines {
        // Both operands are in millionths. Dividing by 10^10 yields cents.
        let cents = rounded(line.quantity.0.checked_mul(line.unit_price.0)?, 10_000_000_000)?;
        net = net.checked_add(cents)?;
        let base = groups.entry(line.vat_rate).or_default();
        *base = base.checked_add(cents)?;
        line_net_amounts.push(money(cents));
    }
    let mut tax = 0_u128;
    let mut tax_breakdown = Vec::with_capacity(groups.len());
    for (rate, base) in groups {
        let cents = rounded(base.checked_mul(rate.0)?, 100 * SCALE)?;
        tax = tax.checked_add(cents)?;
        tax_breakdown.push(TaxTotal { vat_rate: rate, taxable_amount: money(base), tax_amount: money(cents) });
    }
    Some(Totals {
        currency: "EUR",
        line_net_amounts,
        tax_breakdown,
        net_amount: money(net),
        tax_amount: money(tax),
        payable_amount: money(net.checked_add(tax)?),
    })
}

#[cfg(test)]
mod tests;
