//! FieldMDP and Signature Document Locking per ISO 32000-1 §12.8.2.4.

use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockAction {
    All,
    Include(HashSet<String>),
    Exclude(HashSet<String>),
}

#[derive(Debug, Clone)]
pub struct FieldLockDefinition {
    pub action: LockAction,
}

impl FieldLockDefinition {
    pub fn is_field_locked(&self, field_name: &str) -> bool {
        match &self.action {
            LockAction::All => true,
            LockAction::Include(set) => set.contains(field_name),
            LockAction::Exclude(set) => !set.contains(field_name),
        }
    }
}
