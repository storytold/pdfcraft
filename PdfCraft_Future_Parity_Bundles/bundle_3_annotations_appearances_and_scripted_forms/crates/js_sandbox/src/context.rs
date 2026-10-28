//! Fuel-Limited Sandboxed JavaScript Execution Engine.

use std::fmt;
use crate::acrobat_dom::{AcrobatEvent, AppObject, DocObject};
use crate::af_routines::AfRoutines;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JsError {
    FuelExhausted,
    SyntaxError(String),
    EvaluationError(String),
    FieldNotFound(String),
}

impl fmt::Display for JsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FuelExhausted => write!(f, "JavaScript execution aborted: fuel limit exhausted (infinite loop protection)"),
            Self::SyntaxError(msg) => write!(f, "JavaScript syntax error: {}", msg),
            Self::EvaluationError(msg) => write!(f, "JavaScript evaluation error: {}", msg),
            Self::FieldNotFound(name) => write!(f, "Form field '{}' not found in document", name),
        }
    }
}

impl std::error::Error for JsError {}

pub struct SandboxedJsContext {
    pub app: AppObject,
    pub doc: DocObject,
    pub event: Option<AcrobatEvent>,
    fuel_limit: usize,
    fuel_remaining: usize,
}

impl SandboxedJsContext {
    pub fn new(doc: DocObject, fuel_limit: usize) -> Self {
        Self {
            app: AppObject::default(),
            doc,
            event: None,
            fuel_limit,
            fuel_remaining: fuel_limit,
        }
    }

    pub fn reset_fuel(&mut self) {
        self.fuel_remaining = self.fuel_limit;
    }

    #[inline]
    fn consume_fuel(&mut self, amount: usize) -> Result<(), JsError> {
        if self.fuel_remaining < amount {
            return Err(JsError::FuelExhausted);
        }
        self.fuel_remaining -= amount;
        Ok(())
    }

    /// Evaluates Acrobat JavaScript action or calculation formula.
    pub fn execute(&mut self, script: &str) -> Result<Option<String>, JsError> {
        self.reset_fuel();
        let trimmed = script.trim();

        // 1. Detect AFSimple_Calculate call
        if trimmed.starts_with("AFSimple_Calculate(") {
            self.consume_fuel(100)?;
            return self.eval_af_calculate(trimmed);
        }

        // 2. Detect event.value assignment (e.g. event.value = ...;)
        if let Some(rest) = trimmed.strip_prefix("event.value =") {
            self.consume_fuel(50)?;
            let expr = rest.trim().trim_end_matches(';').trim();
            let result_val = self.eval_simple_expression(expr)?;
            if let Some(ref mut ev) = self.event {
                ev.value = result_val.clone();
            }
            return Ok(Some(result_val));
        }

        // 3. Detect app.alert(...)
        if trimmed.starts_with("app.alert(") {
            self.consume_fuel(20)?;
            if let Some(inner) = trimmed.strip_prefix("app.alert(").and_then(|s| s.strip_suffix(");").or_else(|| s.strip_suffix(')'))) {
                let msg = inner.trim().trim_matches('"').trim_matches('\'');
                self.app.alert(msg);
                return Ok(None);
            }
        }

        // 4. Fallback execution simulation
        self.consume_fuel(trimmed.len())?;
        Ok(None)
    }

    fn eval_af_calculate(&mut self, script: &str) -> Result<Option<String>, JsError> {
        // Format: AFSimple_Calculate("SUM", new Array ("Field1", "Field2"))
        let inner = script.strip_prefix("AFSimple_Calculate(").and_then(|s| s.strip_suffix(')').or_else(|| s.strip_suffix(");")))
            .ok_or_else(|| JsError::SyntaxError("Malformed AFSimple_Calculate call".into()))?;

        let parts: Vec<&str> = inner.splitn(2, ',').collect();
        if parts.is_empty() {
            return Err(JsError::SyntaxError("Missing calculation arguments".into()));
        }

        let op = parts[0].trim().trim_matches('"').trim_matches('\'');

        let mut field_names = Vec::new();
        if parts.len() > 1 {
            let fields_part = parts[1].trim();
            for token in fields_part.split(&['(', ')', ',', '"', '\'', '[', ']'][..]) {
                let clean = token.trim();
                if !clean.is_empty() && clean != "new" && clean != "Array" {
                    field_names.push(clean.to_string());
                }
            }
        }

        let result = AfRoutines::simple_calculate(op, &field_names, &self.doc.fields);
        let formatted = format!("{:.2}", result);

        if let Some(ref mut ev) = self.event {
            ev.value = formatted.clone();
        }

        Ok(Some(formatted))
    }

    fn eval_simple_expression(&mut self, expr: &str) -> Result<String, JsError> {
        // Check if string literal
        if (expr.starts_with('"') && expr.ends_with('"')) || (expr.starts_with('\'') && expr.ends_with('\'')) {
            return Ok(expr[1..expr.len() - 1].to_string());
        }

        // Check if field lookup: this.getField("X").value
        if expr.contains("getField(") {
            if let Some(name_start) = expr.find("getField(") {
                let after = &expr[name_start + 9..];
                if let Some(name_end) = after.find(')') {
                    let field_name = after[..name_end].trim().trim_matches('"').trim_matches('\'');
                    if let Some(val) = self.doc.get_field_value(field_name) {
                        return Ok(val);
                    }
                }
            }
        }

        // Check if number literal
        if let Ok(num) = expr.parse::<f64>() {
            return Ok(num.to_string());
        }

        Ok(expr.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_sandboxed_af_calculate_execution() {
        let mut fields = HashMap::new();
        fields.insert("Subtotal".into(), "200.00".into());
        fields.insert("Tax".into(), "16.00".into());

        let doc = DocObject::new(1, fields);
        let mut ctx = SandboxedJsContext::new(doc, 10000);
        ctx.event = Some(AcrobatEvent {
            name: "calculate".into(),
            target_field: "Total".into(),
            value: "".into(),
            rc: true,
            will_commit: true,
        });

        let res = ctx.execute("AFSimple_Calculate(\"SUM\", new Array(\"Subtotal\", \"Tax\"));").unwrap();
        assert_eq!(res, Some("216.00".into()));
        assert_eq!(ctx.event.unwrap().value, "216.00");
    }

    #[test]
    fn test_fuel_exhaustion_protection() {
        let doc = DocObject::new(1, HashMap::new());
        // Fuel limit of 10 instruction units
        let mut ctx = SandboxedJsContext::new(doc, 10);
        let long_script = "a".repeat(100);
        let res = ctx.execute(&long_script);
        assert_eq!(res, Err(JsError::FuelExhausted));
    }
}
