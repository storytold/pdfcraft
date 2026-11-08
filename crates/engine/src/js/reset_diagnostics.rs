//! Bootstrap diagnostics are repeated by the ephemeral runtime, not by the user's action.
//! Remember a bounded multiset for one reset: keep new messages and extra occurrences, and
//! always retain the actual field script's messages, even when their text is identical.

use std::collections::HashMap;

use super::{Message, Outcome};

const MAX_ENTRIES: usize = 1024;
const MAX_BYTES: usize = 1 << 20;

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
enum Diagnostic {
    Console(String),
    Alert { text: String, title: Option<String>, icon: &'static str, buttons: &'static str },
}

impl Diagnostic {
    fn alert(message: &Message) -> Self {
        Self::Alert { text: message.text.clone(), title: message.title.clone(), icon: message.icon.name(), buttons: message.buttons.name() }
    }

    fn bytes(&self) -> usize {
        let payload = match self {
            Self::Console(text) => text.len(),
            Self::Alert { text, title, .. } => text.len().saturating_add(title.as_ref().map_or(0, String::len)),
        };
        payload.saturating_add(std::mem::size_of::<Self>())
    }
}

#[derive(Default)]
pub(super) struct ResetDiagnostics {
    seen: HashMap<Diagnostic, usize>,
    bytes: usize,
}

impl ResetDiagnostics {
    pub(super) fn clear(&mut self) {
        self.seen.clear();
        self.bytes = 0;
    }

    pub(super) fn append(&mut self, outcome: &Outcome, alerts: &mut Vec<Message>, console: &mut Vec<String>) {
        self.append_channel(&outcome.alerts, outcome.bootstrap.alerts, alerts, Diagnostic::alert);
        self.append_channel(&outcome.console, outcome.bootstrap.console, console, |text| Diagnostic::Console(text.clone()));
    }

    fn append_channel<T: Clone>(&mut self, messages: &[T], bootstrap: usize, output: &mut Vec<T>, key: impl Fn(&T) -> Diagnostic) {
        // The runtime already bounds each vector and its payload. This temporary multiset
        // preserves repetitions within one bootstrap, including two scripts saying the same.
        let mut occurrences = HashMap::new();
        for (index, message) in messages.iter().enumerate() {
            if index < bootstrap {
                let key = key(message);
                let count = occurrences.entry(key.clone()).or_insert(0usize);
                *count = count.saturating_add(1);
                if !self.keep(key, *count) {
                    continue;
                }
            }
            output.push(message.clone());
        }
    }

    fn keep(&mut self, key: Diagnostic, occurrence: usize) -> bool {
        if let Some(previous) = self.seen.get_mut(&key) {
            if occurrence <= *previous {
                return false;
            }
            *previous = occurrence;
            return true;
        }
        // Bound the extra memory used by deduplication. At capacity retain diagnostics
        // instead of hiding uncached messages; the existing runtime limits still apply.
        let Some(bytes) = self.bytes.checked_add(key.bytes()).filter(|bytes| *bytes <= MAX_BYTES) else { return true };
        if self.seen.len() >= MAX_ENTRIES {
            return true;
        }
        self.bytes = bytes;
        self.seen.insert(key, occurrence);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdfcraft_js::BootstrapDiagnostics;

    #[test]
    fn reset_diagnostics_cache_is_bounded_and_keeps_uncached_messages() {
        let mut cache = ResetDiagnostics::default();
        for i in 0..MAX_ENTRIES {
            assert!(cache.keep(Diagnostic::Console(i.to_string()), 1));
        }
        let uncached = Diagnostic::Console("new message".into());
        assert!(cache.keep(uncached.clone(), 1));
        assert!(cache.keep(uncached, 1));
        assert_eq!(cache.seen.len(), MAX_ENTRIES);
        assert!(cache.bytes <= MAX_BYTES);
        cache.clear();
        let oversized = Diagnostic::Console("x".repeat(MAX_BYTES));
        assert!(cache.keep(oversized.clone(), 1));
        assert!(cache.keep(oversized, 1));
        assert!(cache.seen.is_empty());
        assert_eq!(cache.bytes, 0);
    }

    #[test]
    fn reset_diagnostics_compare_full_alerts_and_preserve_occurrence_counts() {
        let mut cache = ResetDiagnostics::default();
        let message = Message::plain("same");
        let titled = Message { title: Some("different".into()), ..message.clone() };
        let first = Outcome {
            alerts: vec![message.clone(), message.clone()],
            bootstrap: BootstrapDiagnostics { alerts: 2, console: 0 },
            ..Default::default()
        };
        let mut alerts = Vec::new();
        let mut console = Vec::new();
        cache.append(&first, &mut alerts, &mut console);
        assert_eq!(alerts.len(), 2);
        cache.append(&first, &mut alerts, &mut console);
        assert_eq!(alerts.len(), 2);
        let next = Outcome {
            alerts: vec![message.clone(), message.clone(), message.clone(), titled.clone(), message.clone()],
            bootstrap: BootstrapDiagnostics { alerts: 4, console: 0 },
            ..Default::default()
        };
        cache.append(&next, &mut alerts, &mut console);
        assert_eq!(alerts, [message.clone(), message.clone(), message.clone(), titled, message]);
        cache.clear();
        cache.append(&first, &mut alerts, &mut console);
        assert_eq!(alerts.len(), 7);
    }
}
