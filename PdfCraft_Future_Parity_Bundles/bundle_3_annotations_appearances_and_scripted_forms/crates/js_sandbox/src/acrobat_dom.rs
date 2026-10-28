//! Acrobat Host Object Models (`app`, `Doc`, `event`, `util`).

use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct AcrobatEvent {
    pub name: String, // "validate", "calculate", "format", "keystroke"
    pub target_field: String,
    pub value: String,
    pub rc: bool,
    pub will_commit: bool,
}

#[derive(Debug, Clone)]
pub struct AppObject {
    pub alerts_logged: Vec<String>,
    pub beeps_count: u32,
}

impl Default for AppObject {
    fn default() -> Self {
        Self {
            alerts_logged: Vec::new(),
            beeps_count: 0,
        }
    }
}

impl AppObject {
    pub fn alert(&mut self, msg: impl Into<String>) {
        self.alerts_logged.push(msg.into());
    }

    pub fn beep(&mut self) {
        self.beeps_count += 1;
    }
}

#[derive(Debug, Clone)]
pub struct DocObject {
    pub num_pages: u32,
    pub page_num: u32,
    pub fields: HashMap<String, String>,
}

impl DocObject {
    pub fn new(num_pages: u32, fields: HashMap<String, String>) -> Self {
        Self {
            num_pages,
            page_num: 0,
            fields,
        }
    }

    pub fn get_field_value(&self, name: &str) -> Option<String> {
        self.fields.get(name).cloned()
    }

    pub fn set_field_value(&mut self, name: &str, val: String) {
        self.fields.insert(name.to_string(), val);
    }
}
