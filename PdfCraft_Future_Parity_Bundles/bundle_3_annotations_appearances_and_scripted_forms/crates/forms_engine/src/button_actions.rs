//! Interactive Push Button Event Dispatcher and Action Execution.

use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonMouseEvent {
    MouseDown,
    MouseUp,
    MouseEnter,
    MouseExit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormAction {
    ResetForm { fields: Option<Vec<String>> },
    SubmitForm { url: String, export_format: String },
    JavaScript { script: String },
}

pub struct ButtonActionDispatcher;

impl ButtonActionDispatcher {
    /// Executes button action, mutating form field values in-place where applicable.
    pub fn dispatch(
        action: &FormAction,
        fields: &mut HashMap<String, String>,
        defaults: &HashMap<String, String>,
    ) -> Option<String> {
        match action {
            FormAction::ResetForm { fields: target_fields } => {
                match target_fields {
                    Some(list) => {
                        for name in list {
                            if let Some(def) = defaults.get(name) {
                                fields.insert(name.clone(), def.clone());
                            } else {
                                fields.remove(name);
                            }
                        }
                    }
                    None => {
                        // Reset all fields
                        for (name, def) in defaults {
                            fields.insert(name.clone(), def.clone());
                        }
                    }
                }
                None
            }
            FormAction::SubmitForm { url, export_format } => {
                Some(format!("SUBMIT_REQUEST: URL={} FORMAT={}", url, export_format))
            }
            FormAction::JavaScript { script } => {
                Some(format!("EXECUTE_JS: {}", script))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reset_form_action() {
        let mut fields = HashMap::new();
        fields.insert("Name".into(), "Modified Name".into());
        fields.insert("Age".into(), "42".into());

        let mut defaults = HashMap::new();
        defaults.insert("Name".into(), "Default Name".into());
        defaults.insert("Age".into(), "0".into());

        ButtonActionDispatcher::dispatch(&FormAction::ResetForm { fields: None }, &mut fields, &defaults);
        assert_eq!(fields.get("Name").unwrap(), "Default Name");
        assert_eq!(fields.get("Age").unwrap(), "0");
    }
}
