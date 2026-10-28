//! Write-Ahead Crash Recovery Transaction Journal for Web and Desktop Sessions.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageAction {
    InsertPage { index: usize, width: u32, height: u32 },
    DeletePage { index: usize },
    RotatePage { index: usize, degrees: i32 }, // 90, 180, 270
    ReorderPage { from_index: usize, to_index: usize },
    CropPage { index: usize, rect: [u32; 4] },
}

#[derive(Debug, Clone)]
pub struct JournalTransaction {
    pub timestamp_epoch_ms: u64,
    pub action: PageAction,
}

pub struct CrashRecoveryJournal {
    transactions: Vec<JournalTransaction>,
    is_dirty: bool,
}

impl Default for CrashRecoveryJournal {
    fn default() -> Self {
        Self {
            transactions: Vec::new(),
            is_dirty: false,
        }
    }
}

impl CrashRecoveryJournal {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a page manipulation action to the journal.
    pub fn record(&mut self, action: PageAction, timestamp_ms: u64) {
        self.transactions.push(JournalTransaction {
            timestamp_epoch_ms: timestamp_ms,
            action,
        });
        self.is_dirty = true;
    }

    /// Replay transactions to restore document state after crash.
    pub fn replay_onto_pages(&self, initial_pages: &mut Vec<usize>) {
        for tx in &self.transactions {
            match tx.action {
                PageAction::InsertPage { index, .. } => {
                    let insert_pos = index.min(initial_pages.len());
                    initial_pages.insert(insert_pos, 9999); // Placeholder ID
                }
                PageAction::DeletePage { index } => {
                    if index < initial_pages.len() {
                        initial_pages.remove(index);
                    }
                }
                PageAction::ReorderPage { from_index, to_index } => {
                    if from_index < initial_pages.len() {
                        let item = initial_pages.remove(from_index);
                        let target = to_index.min(initial_pages.len());
                        initial_pages.insert(target, item);
                    }
                }
                PageAction::RotatePage { .. } | PageAction::CropPage { .. } => {
                    // Page attributes updated in-place
                }
            }
        }
    }

    pub fn commit(&mut self) {
        self.is_dirty = false;
    }

    pub fn is_dirty(&self) -> bool {
        self.is_dirty
    }

    pub fn clear(&mut self) {
        self.transactions.clear();
        self.is_dirty = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_journal_replay_recovers_state() {
        let mut journal = CrashRecoveryJournal::new();
        journal.record(PageAction::InsertPage { index: 1, width: 612, height: 792 }, 100);
        journal.record(PageAction::DeletePage { index: 0 }, 200);

        let mut pages = vec![1, 2, 3];
        journal.replay_onto_pages(&mut pages);

        // Initially [1, 2, 3], insert at 1 -> [1, 9999, 2, 3], delete at 0 -> [9999, 2, 3]
        assert_eq!(pages, vec![9999, 2, 3]);
    }
}
