//! Schema-Driven Preferences Model for Desktop and Web Applications.

use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrefValue {
    Bool(bool),
    Int(i64),
    String(String),
}

#[derive(Debug, Clone)]
pub struct PreferenceItem {
    pub key: String,
    pub category: String, // "General", "PageDisplay", "Security", "Accessibility"
    pub default_value: PrefValue,
    pub current_value: PrefValue,
}

pub struct PreferencesManager {
    items: HashMap<String, PreferenceItem>,
}

impl Default for PreferencesManager {
    fn default() -> Self {
        let mut mgr = Self { items: HashMap::new() };
        mgr.register_defaults();
        mgr
    }
}

impl PreferencesManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get_bool(&self, key: &str) -> Option<bool> {
        match self.items.get(key)?.current_value {
            PrefValue::Bool(b) => Some(b),
            _ => None,
        }
    }

    pub fn set_bool(&mut self, key: &str, val: bool) {
        if let Some(item) = self.items.get_mut(key) {
            item.current_value = PrefValue::Bool(val);
        }
    }

    fn register_defaults(&mut self) {
        self.items.insert("display.smooth_text".into(), PreferenceItem {
            key: "display.smooth_text".into(),
            category: "PageDisplay".into(),
            default_value: PrefValue::Bool(true),
            current_value: PrefValue::Bool(true),
        });
        self.items.insert("accessibility.replace_document_colors".into(), PreferenceItem {
            key: "accessibility.replace_document_colors".into(),
            category: "Accessibility".into(),
            default_value: PrefValue::Bool(false),
            current_value: PrefValue::Bool(false),
        });
        self.items.insert("security.verify_signatures_on_open".into(), PreferenceItem {
            key: "security.verify_signatures_on_open".into(),
            category: "Security".into(),
            default_value: PrefValue::Bool(true),
            current_value: PrefValue::Bool(true),
        });
    }
}
