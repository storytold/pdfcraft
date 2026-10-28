//! PdfCraft Assembly: Page organization, crash recovery transaction journal,
//! drag-and-drop page assembly, and bookmark action hierarchy.
//!
//! Subsystem covering Milestone M4:
//! - [P1] Crash recovery in web and desktop app
//! - [P1] Insert pages by dropping files & range/orientation selection
//! - [P1] Bookmark actions and navigation destinations

pub mod journal;
pub mod filter;
pub mod bookmarks;

pub use journal::{CrashRecoveryJournal, JournalTransaction, PageAction};
pub use filter::{PageMetadata, PageOrientation, PageSelectionEngine};
pub use bookmarks::{BookmarkDestination, BookmarkNode, BookmarkTree, DestinationFit};
