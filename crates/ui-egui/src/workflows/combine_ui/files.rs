use super::*;

/// A column of the list: what it shows and what it sorts by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortKey {
    Name,
    Pages,
    Size,
    Modified,
    Warnings,
}

impl SortKey {
    pub const ALL: [SortKey; 5] = [SortKey::Name, SortKey::Pages, SortKey::Size, SortKey::Modified, SortKey::Warnings];

    pub(super) fn index(self) -> usize {
        self as usize
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            SortKey::Name => tl!("File name"),
            SortKey::Pages => tl!("Pages"),
            SortKey::Size => tl!("Size"),
            SortKey::Modified => tl!("Modified"),
            SortKey::Warnings => tl!("Warnings"),
        }
    }

    /// The narrowest the column may be dragged.
    pub(super) fn min_width(self) -> f32 {
        match self {
            SortKey::Name => 120.0,
            SortKey::Pages => 110.0,
            SortKey::Size => 60.0,
            SortKey::Modified => 80.0,
            SortKey::Warnings => 100.0,
        }
    }
}

/// The columns' order and widths, as the user arranged them (kept in the settings).
#[derive(Clone, Debug, PartialEq)]
pub struct Columns {
    pub order: [SortKey; 5],
    /// By [`SortKey::index`]; `None` until a column is resized: fitted to the window until then.
    pub widths: Option<[f32; 5]>,
}

impl Default for Columns {
    fn default() -> Self {
        Columns { order: SortKey::ALL, widths: None }
    }
}

impl Columns {
    pub(crate) fn to_json(&self) -> serde_json::Value {
        serde_json::json!({ "order": self.order, "widths": self.widths })
    }

    /// Settings written by [`Self::to_json`]; anything malformed gives the default layout.
    pub(crate) fn from_json(v: &serde_json::Value) -> Self {
        let order = serde_json::from_value::<Vec<SortKey>>(v["order"].clone())
            .ok()
            .filter(|o| SortKey::ALL.iter().all(|k| o.contains(k)))
            .and_then(|o| <[SortKey; 5]>::try_from(o).ok())
            .unwrap_or(SortKey::ALL);
        let widths = serde_json::from_value::<[f32; 5]>(v["widths"].clone())
            .ok()
            .filter(|w| w.iter().all(|x| x.is_finite()))
            .map(|w| std::array::from_fn(|i| w[i].clamp(SortKey::ALL[i].min_width(), 4000.0)));
        Columns { order, widths }
    }

    /// The widths to show: the user's, or the window's width shared out.
    pub(super) fn resolved(&self, available: f32) -> [f32; 5] {
        if let Some(w) = self.widths {
            return w;
        }
        let fixed = 170.0 + 84.0 + 110.0;
        let flexible = (available - fixed).max(260.0);
        [(flexible * 0.55).max(140.0), 170.0, 84.0, 110.0, (flexible * 0.45).max(120.0)]
    }
}

/// How the files are shown: a thumbnail of each (the default, to see what goes where) or the
/// table (kept in the settings).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CombineView {
    #[default]
    Grid,
    List,
}

impl CombineView {
    pub fn as_str(self) -> &'static str {
        match self {
            CombineView::Grid => "grid",
            CombineView::List => "list",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "grid" => Some(CombineView::Grid),
            "list" => Some(CombineView::List),
            _ => None,
        }
    }
}

/// The pages a range takes, and the range and page count they were worked out for.
pub(crate) type PagesTaken = ((String, usize), Arc<[usize]>);

/// A file shown large: which, the place among the pages it adds, and those pages (worked out
/// again only when its page range or page count changes).
#[derive(Clone, Debug)]
pub(crate) struct Preview {
    pub(crate) file: u64,
    pub(crate) at: usize,
    /// The page (0-based, in the file) on screen, for the thumbnails to render.
    pub(crate) page: Option<usize>,
    pub(crate) pages: Option<PagesTaken>,
}

/// A password: kept in memory only, never printed (debug output ends up in logs).
#[derive(Clone, Default)]
pub struct Secret(pub(super) String);

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_empty() { "\"\"" } else { "…" })
    }
}

/// Which password a protected file still needs before it can be combined.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lock {
    /// The password that opens it.
    Open,
    /// It opens, but only its permissions (owner) password allows copying its pages.
    Permissions,
}

/// The combine list's field-name conflicts, computed once per list (it opens the files):
/// for each file, the renames combining would make of its form fields. Keyed by the file ids
/// and their lock state, so unlocking a file refreshes it.
#[derive(Default, Debug)]
pub struct ConflictPreview {
    key: (Vec<u64>, Vec<bool>),
    pub(super) per_file: Vec<Vec<(String, String)>>,
}

impl ConflictPreview {
    /// Bring the preview in step with `files`, opening them through the engine when the list
    /// changed. A file split into parts is combined as one file, once — each group's first
    /// entry stands for it, as the combine does — and every part shows the file's preview.
    /// A file that cannot be opened previews as nothing (its row says why).
    pub(super) fn refresh(&mut self, session: &pdfcraft_engine::Session, files: &[CombineFile]) {
        let key = (files.iter().map(|f| f.id).collect(), files.iter().map(|f| f.password.is_some()).collect());
        if self.key == key && self.per_file.len() == files.len() {
            return;
        }
        let mut groups: Vec<u64> = Vec::new();
        let mut sources: Vec<pdfcraft_engine::CombineSource> = Vec::new();
        let mut passwords: Vec<Option<&str>> = Vec::new();
        for f in files {
            if groups.contains(&f.group) {
                continue;
            }
            groups.push(f.group);
            sources.push((f.name.clone(), f.bytes.clone(), None));
            passwords.push(f.password.as_ref().map(|p| p.0.as_str()));
        }
        let per_group = session.combine_field_conflicts(&sources, &passwords);
        self.per_file =
            files.iter().map(|f| groups.iter().position(|g| *g == f.group).and_then(|at| per_group.get(at)).cloned().unwrap_or_default()).collect();
        self.key = key;
    }
}

/// Unlock…'s password box: the rows it unlocks and what has been typed.
#[derive(Clone, Debug)]
pub(super) struct UnlockPrompt {
    pub(super) rows: Vec<u64>,
    pub(super) input: Secret,
    pub(super) show: bool,
    pub(super) error: Option<String>,
    /// Where the box opens (below the link or button that asked for it).
    pub(super) at: egui::Pos2,
    pub(super) focused: bool,
}

/// What can be known about a file up front, opened with a password or not.
#[derive(Clone)]
pub(super) struct Assessed {
    pub(super) pages: usize,
    /// Each page's displayed size in points (empty when the inspector couldn't read it).
    pub(super) sizes: Arc<[(f32, f32)]>,
    pub(super) problem: Option<String>,
    pub(super) lock: Option<Lock>,
    pub(super) notes: Vec<String>,
}

pub(super) enum AssessError {
    WrongPassword,
    Unreadable(String),
}

/// Read `bytes` (with `password`, if given) for the list: pages, what stops it being combined
/// and which password would help, and warnings.
pub(super) fn assess(bytes: &Arc<Vec<u8>>, password: Option<&str>) -> Result<Assessed, AssessError> {
    // The engine that combines decides whether the password works; the inspector (which may
    // not accept every password the engine does, e.g. an RC4 owner password) adds the details.
    let check = pdfcraft_engine::combine_source_check(bytes, password);
    if check == Err(SourceProblem::WrongPassword) {
        return Err(AssessError::WrongPassword);
    }
    let info = pdfcraft_render::inspect(bytes.clone(), password);
    if let (Err(SourceProblem::Unreadable(e)), Err(_)) = (&check, &info) {
        return Err(AssessError::Unreadable(e.clone()));
    }
    let info = info.ok();
    let pages = match &info {
        Some(i) => i.pages.len(),
        None => pdfcraft_engine::source_page_count(bytes, password).unwrap_or(0),
    };
    let sizes: Arc<[(f32, f32)]> = match &info {
        Some(i) => i.pages.iter().map(|p| (p.width, p.height)).collect(),
        None => Arc::from(Vec::new()),
    };
    let encrypted = password.is_some() || info.as_ref().is_some_and(|i| i.encrypted);
    let warnings = info.map(|i| i.warnings).unwrap_or_default();
    let (problem, lock) = match check {
        Ok(()) => (None, None),
        Err(SourceProblem::Password) => (Some(tl!("Password-protected").to_string()), Some(Lock::Open)),
        Err(SourceProblem::WrongPassword) => return Err(AssessError::WrongPassword),
        Err(SourceProblem::NotPermitted) if password.is_some() => (
            Some(tl!("Opened, but its security settings don't allow combining. Enter the permissions password.").to_string()),
            Some(Lock::Permissions),
        ),
        Err(SourceProblem::NotPermitted) => (Some(tl!("Its security settings don't allow copying pages").to_string()), Some(Lock::Permissions)),
        Err(SourceProblem::Unreadable(e)) => (Some(crate::i18n::fmt(tl!("Can't be read: {e}"), &[("e", &e)])), None),
    };
    let mut notes = Vec::new();
    // The combined file is a new, unprotected document: protection doesn't carry over.
    if encrypted && problem.is_none() {
        notes.push(if password.is_some() {
            tl!("Unlocked: the combined file won't be password-protected").to_string()
        } else {
            tl!("Protected: the combined file won't keep its security settings").to_string()
        });
    }
    notes.extend(warnings.into_iter().take(5));
    Ok(Assessed { pages, sizes, problem, lock, notes })
}

#[derive(Clone, Debug)]
pub(super) struct Snapshot {
    pub(super) files: Vec<CombineFile>,
    pub(super) selected: BTreeSet<u64>,
    pub(super) sort: Option<(SortKey, bool)>,
}

/// A file arriving in the list (picked, dropped or already open).
pub(crate) struct Incoming {
    pub name: String,
    pub bytes: Arc<Vec<u8>>,
    pub modified: Option<SystemTime>,
    /// Something to tell the user about where it came from.
    pub note: Option<String>,
    /// The open document it is, when it came from a tab.
    pub open_doc: Option<pdfcraft_engine::DocId>,
}

/// A new value for [`CombineFile::auth`]: counted for the whole run, outside the undo history.
fn next_auth() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// A page range checked: the pages it takes and the first of them (0-based), or why it's wrong.
type RangeCheck = Result<(usize, usize), String>;

#[derive(Clone, Debug)]
pub struct CombineFile {
    /// Stable while the file is in the list (selection, undo).
    pub id: u64,
    /// The file it is (part of): the entries a file is split into (a page moved between another
    /// file's pages) share it. Combined as one file, unlocked together, counted once.
    pub(crate) group: u64,
    pub name: String,
    pub bytes: Arc<Vec<u8>>,
    /// The file as it was added: `bytes` itself for a PDF, the image or text it was made from
    /// otherwise (converted anew, with its own date, each time it is added).
    pub(super) source: Arc<Vec<u8>>,
    pub pages: usize,
    /// Each page's displayed size in points; shared, so undo snapshots don't copy it.
    pub(crate) sizes: Arc<[(f32, f32)]>,
    /// How the file was read (with which password): a new value every time it is read, never
    /// reused, not even after undo, so a thumbnail made another way is never taken for this one.
    pub(crate) auth: u64,
    /// The pages to take ("" = all).
    pub range: String,
    /// When the file was last modified (desktop picks and drops only).
    pub modified: Option<SystemTime>,
    /// Why the file can't be combined (password, security settings), if so.
    pub problem: Option<String>,
    /// The password that would let it be combined, while one is needed.
    pub lock: Option<Lock>,
    /// The password it was unlocked with.
    pub(super) password: Option<Secret>,
    /// Warnings that don't stop it being combined.
    pub notes: Vec<String>,
    /// Where it came from, shown among the warnings (e.g. an open document's unsaved changes).
    pub(super) origin_note: Option<String>,
    /// The open document it is, when it came from a tab: Add open documents leaves it out.
    pub(super) open_doc: Option<pdfcraft_engine::DocId>,
    /// The range last checked and the result: the number of pages it takes and the first of
    /// them (0-based), or why it is wrong.
    pub(super) checked: Option<(String, RangeCheck)>,
}

impl CombineFile {
    pub(super) fn take(&mut self, a: Assessed, password: Option<Secret>) {
        self.pages = a.pages;
        self.sizes = a.sizes;
        self.problem = a.problem;
        self.lock = a.lock;
        self.password = password;
        self.auth = next_auth();
        self.notes = self.origin_note.iter().cloned().chain(a.notes).collect();
        self.checked = None;
    }

    /// Whether the file can't be read at all (damaged, or a type that can't be made into a PDF):
    /// it stays in the list, with why, and Combine leaves it out once the user agrees. A
    /// password-protected file can be unlocked instead, so it isn't one.
    pub(crate) fn unreadable(&self) -> bool {
        self.problem.is_some() && self.lock.is_none()
    }

    /// The password the file was unlocked with, if any.
    pub(crate) fn password(&self) -> Option<&str> {
        self.password.as_ref().map(|p| p.0.as_str())
    }

    /// The number of pages `range` takes, or why it can't be used. Checked again only when the
    /// range changes.
    pub fn selection(&mut self) -> Result<usize, String> {
        self.checked_range().map(|(count, _)| count)
    }

    /// The first page (0-based) the range takes, in its order ("3, 1" starts with page 3): the
    /// page the grid shows for the file.
    pub fn first_page(&mut self) -> Result<usize, String> {
        self.checked_range().map(|(_, first)| first)
    }

    /// The first page the range takes, without updating the cache (read-only callers).
    pub(crate) fn first_taken(&self) -> Option<usize> {
        match &self.checked {
            Some((range, result)) if *range == self.range => result.as_ref().ok().map(|(_, first)| *first),
            _ => self.pages_taken().ok()?.first().copied(),
        }
    }

    /// Every page (0-based) the range takes, in its order: what the preview flips through.
    pub(crate) fn pages_taken(&self) -> Result<Vec<usize>, String> {
        if self.range.trim().is_empty() {
            return Ok((0..self.pages).collect());
        }
        match pdfcraft_engine::print::select_pages(self.pages, Some(&self.range), &[], pdfcraft_engine::print::Subset::All, false) {
            Ok(p) if p.is_empty() => Err(tl!("No pages selected").to_string()),
            Ok(p) => Ok(p),
            Err(e) => Err(e.to_string()),
        }
    }

    fn checked_range(&mut self) -> RangeCheck {
        if let Some((range, result)) = &self.checked
            && *range == self.range
        {
            return result.clone();
        }
        let result = if self.range.trim().is_empty() {
            // (A file whose pages couldn't be counted is left to the engine, as before; the grid
            // checks the first page against the page count.)
            Ok((self.pages, 0))
        } else {
            match pdfcraft_engine::print::select_pages(self.pages, Some(&self.range), &[], pdfcraft_engine::print::Subset::All, false) {
                Ok(p) => match p.first() {
                    Some(first) => Ok((p.len(), *first)),
                    None => Err(tl!("No pages selected").to_string()),
                },
                Err(e) => Err(e.to_string()),
            }
        };
        self.checked = Some((self.range.clone(), result.clone()));
        result
    }

    /// 0 = can't be combined, 1 = has warnings, 2 = fine: the Warnings column's sort order.
    pub(super) fn severity(&mut self) -> u8 {
        if self.problem.is_some() || self.selection().is_err() {
            0
        } else if !self.notes.is_empty() {
            1
        } else {
            2
        }
    }
}

#[derive(Clone)]
pub(crate) enum RowAction {
    /// A click on row `i` (Ctrl/⌘ toggles it, Shift selects up to it).
    Click(usize, Modifiers),
    SelectAll,
    /// An arrow key: `by` files on (1 in the list, or along a grid row; a grid row's length for
    /// Up/Down in the grid). With Shift the selection grows.
    Step {
        down: bool,
        by: usize,
        extend: bool,
    },
    MoveUp,
    MoveDown,
    Remove,
    /// One file's card trash: that file only, whatever else is selected.
    RemoveFile(u64),
    /// One file's magnifier: show it large.
    Preview(u64),
    /// Space: show the file the keyboard is on (or the first selected) large.
    PreviewSelected,
    /// A page card's magnifier: show its file large, from that page (its place among the pages
    /// the file adds).
    PreviewAt(u64, usize),
    /// Show one file as its pages in the grid (`true`), or as its card again.
    Expand(u64, bool),
    /// Every file that adds more than one page, as its pages (`true`), or all as cards again.
    ExpandAll(bool),
    /// A click on a page card: that page (by entry id and place among its pages) selected,
    /// Ctrl/⌘ toggling it, Shift selecting up to it.
    ClickPage(u64, usize, Modifiers),
    /// A page card (and the rest of the selected pages, if it is selected) dropped before page
    /// `gap` of the list (see [`moved_pages`]), if the list hasn't changed since the drag began.
    MovePages {
        page: (u64, usize),
        revision: u64,
        gap: usize,
    },
    /// One page's trash (`Some`), or the selected pages: out of the list.
    RemovePages(Option<(u64, usize)>),
    /// Row `from` (and the rest of the selection, if it is selected) dropped on row `to`.
    Drop {
        from: usize,
        to: usize,
    },
    /// A grid card (and the rest of the selection, if it is selected) dropped at `gap`
    /// (0 = before the first file, n = after the last), if the list hasn't changed since the
    /// drag began.
    DropAt {
        file: u64,
        revision: u64,
        gap: usize,
    },
    Sort(SortKey),
    /// Unlock… on row `i` (and the rest of the selection, if it is selected), or on the
    /// selection (`None`); the password box opens at the position.
    Unlock(Option<usize>, egui::Pos2),
}
