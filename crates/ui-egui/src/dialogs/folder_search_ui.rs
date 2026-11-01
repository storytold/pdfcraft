//! Edit ▸ Search a folder…: the dialog for `pdfcraft_engine::folder_search`. The search runs on its
//! own thread, its results show as they come, and closing the dialog stops it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};

use egui::text::LayoutJob;
use egui::{Color32, FontId, TextFormat};
use pdfcraft_engine::folder_search::{self, Event, FileMatches, Options, Skip, Skipped, Stop, Summary};

use crate::i18n::fmt;
use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, widgets};

/// Files listed under "Not searched"; the count above them says how many there are in all.
const MAX_SKIPPED_SHOWN: usize = 200;
/// The tallest the results get before they scroll.
const RESULTS_HEIGHT: f32 = 320.0;
/// The results' least height, however short the window.
const MIN_RESULTS_HEIGHT: f32 = 80.0;
/// What the rest of the dialog and its margins take from the window.
const RESULTS_MARGIN: f32 = 300.0;

enum Message {
    Event(Event),
    Done(Result<Summary, String>),
}

struct Job {
    receiver: mpsc::Receiver<Message>,
    cancel: Arc<AtomicBool>,
}

pub(crate) struct State {
    folder: String,
    query: String,
    subfolders: bool,
    case_sensitive: bool,
    whole_words: bool,
    job: Option<Job>,
    found: usize,
    reading: Option<(usize, String)>,
    files: Vec<FileMatches>,
    skipped: Vec<Skipped>,
    outcome: Option<Result<Summary, String>>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            folder: String::new(),
            query: String::new(),
            subfolders: true,
            case_sensitive: false,
            whole_words: false,
            job: None,
            found: 0,
            reading: None,
            files: Vec::new(),
            skipped: Vec::new(),
            outcome: None,
        }
    }
}

impl State {
    /// Ask a running search to end; it does at its next page, and what it found stays.
    pub(crate) fn stop(&mut self) {
        if let Some(job) = &self.job {
            job.cancel.store(true, Ordering::Relaxed);
        }
    }

    pub(crate) fn poll(&mut self) {
        let Some(job) = self.job.take() else { return };
        loop {
            match job.receiver.try_recv() {
                Ok(Message::Event(Event::Found(found))) => self.found = found,
                Ok(Message::Event(Event::Reading { index, path })) => {
                    self.reading = Some((index, path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned())));
                }
                Ok(Message::Event(Event::Matches(file))) => self.files.push(file),
                Ok(Message::Event(Event::Skipped(file))) => self.skipped.push(file),
                Ok(Message::Done(result)) => {
                    self.outcome = Some(result);
                    self.reading = None;
                    return;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.outcome = Some(Err(tl!("The search stopped unexpectedly.").to_string()));
                    self.reading = None;
                    return;
                }
            }
        }
        self.job = Some(job);
    }

    fn start(&mut self, ctx: &egui::Context, inline: bool) {
        self.found = 0;
        self.reading = None;
        self.files.clear();
        self.skipped.clear();
        self.outcome = None;
        let mut options = Options::new(self.folder.trim(), self.query.clone());
        options.subfolders = self.subfolders;
        options.case_sensitive = self.case_sensitive;
        options.whole_words = self.whole_words;
        let (sender, receiver) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        self.job = Some(Job { receiver, cancel: cancel.clone() });
        let ctx = ctx.clone();
        let work = move || {
            let result = folder_search::search(&options, &cancel, |event| {
                let _ = sender.send(Message::Event(event));
                ctx.request_repaint();
            });
            let _ = sender.send(Message::Done(result));
            ctx.request_repaint();
        };
        #[cfg(not(target_arch = "wasm32"))]
        if !inline {
            if let Err(error) = std::thread::Builder::new().name("folder-search".into()).spawn(work) {
                self.job = None;
                self.outcome = Some(Err(error.to_string()));
            }
            return;
        }
        let _ = inline;
        work();
    }

    fn status(&self) -> Vec<String> {
        if self.job.is_some() {
            return vec![match &self.reading {
                Some((index, name)) => fmt(
                    tl!("Reading file {n} of {total}: {name}"),
                    &[("n", &index.saturating_add(1).to_string()), ("total", &self.found.to_string()), ("name", name)],
                ),
                None => tl!("Listing the folder…").to_string(),
            }];
        }
        let summary = match &self.outcome {
            None => return Vec::new(),
            Some(Err(error)) => return vec![error.clone()],
            Some(Ok(summary)) => summary,
        };
        let mut lines = vec![fmt(
            tl!("Matches: {matches}. Files with matches: {files}. PDFs read to the end: {searched} of {found}."),
            &[
                ("matches", &summary.matches.to_string()),
                ("files", &summary.files_with_matches.to_string()),
                ("searched", &summary.searched.to_string()),
                ("found", &summary.found.to_string()),
            ],
        )];
        match summary.stopped {
            Some(Stop::Cancelled) => lines.push(tl!("Stopped before the end.").to_string()),
            Some(Stop::MatchLimit) => lines.push(fmt(tl!("Stopped at {limit} matches."), &[("limit", &folder_search::DEFAULT_LIMIT.to_string())])),
            Some(Stop::TooManySlowFiles) => lines.push(tl!("Stopped: too many files took too long to read.").to_string()),
            None => {}
        }
        if summary.folder_cut_short {
            lines.push(tl!("This folder is too large or too deep to search all of it.").to_string());
        }
        lines
    }
}

fn reason(skip: &Skip) -> String {
    match skip {
        Skip::LinkOrSpecialFile => tl!("A link or special file, which isn't followed").to_string(),
        Skip::TooLarge => tl!("Larger than 512 MiB").to_string(),
        Skip::Unreadable(why) | Skip::Damaged(why) => fmt(tl!("Couldn't be read: {reason}"), &[("reason", why)]),
        Skip::Secured => tl!("Secured").to_string(),
        Skip::TooSlow => tl!("Took too long to read").to_string(),
        Skip::PagesUnread(pages) => fmt(tl!("Pages that couldn't be read: {n}"), &[("n", &pages.to_string())]),
    }
}

fn excerpt(found: &folder_search::Match, width: f32, t: &Tokens) -> LayoutJob {
    let mut job = LayoutJob::default();
    let plain = TextFormat { font_id: FontId::new(12.5, egui::FontFamily::Proportional), color: t.text_muted, ..Default::default() };
    job.append(&format!("…{} ", found.before), 0.0, plain.clone());
    job.append(
        &found.text,
        0.0,
        TextFormat {
            font_id: theme::semibold(12.5),
            color: t.text,
            background: Color32::from_rgba_unmultiplied(255, 214, 0, 90),
            ..Default::default()
        },
    );
    job.append(&format!(" {}…", found.after), 0.0, plain);
    job.wrap.max_width = width;
    job
}

/// Draw the dialog. Returns `true` to close it.
pub(crate) fn body(ui: &mut egui::Ui, app: &mut PdfCraftApp) -> bool {
    let t = Tokens::get(ui.ctx());
    app.folder_search.poll();
    ui.label(egui::RichText::new(tl!("Search a folder")).font(theme::semibold(18.0)));
    ui.add_space(8.0);
    let inline = app.run_inline;
    let mut choose = false;
    let state = &mut app.folder_search;
    let running = state.job.is_some();
    let mut enter = false;
    // A grid cell has no width of its own for `desired_width` to ask for.
    let field_size = egui::vec2(420.0, 30.0);
    egui::Grid::new("folder-search-fields").num_columns(3).show(ui, |ui| {
        let label = ui.label(tl!("Folder"));
        ui.add_sized(field_size, widgets::line(&mut state.folder)).labelled_by(label.id);
        // The web build has no folders to choose from.
        #[cfg(not(target_arch = "wasm32"))]
        {
            choose = widgets::pill_button(ui, tl!("Browse…"), false).clicked();
        }
        ui.end_row();
        let label = ui.label(tl!("Search"));
        let field = ui.add_sized(field_size, widgets::line(&mut state.query).char_limit(folder_search::MAX_QUERY_CHARS)).labelled_by(label.id);
        enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        ui.end_row();
    });
    ui.horizontal(|ui| {
        ui.add(widgets::checkbox(&mut state.subfolders, tl!("Include subfolders")));
        ui.add(widgets::checkbox(&mut state.whole_words, tl!("Whole words only")));
        ui.add(widgets::checkbox(&mut state.case_sensitive, tl!("Case-sensitive")));
    });
    ui.add_space(6.0);
    let ready = !state.folder.trim().is_empty() && !state.query.trim().is_empty();
    let mut close = false;
    ui.horizontal(|ui| {
        if running {
            if widgets::pill_button(ui, tl!("Stop"), false).clicked() {
                state.stop();
            }
        } else if ui.add_enabled_ui(ready, |ui| widgets::pill_button(ui, tl!("Search"), true)).inner.clicked() || (ready && enter) {
            state.start(ui.ctx(), inline);
            state.poll();
        }
        close = widgets::pill_button(ui, tl!("Close"), false).clicked();
    });
    ui.add_space(6.0);
    for line in state.status() {
        ui.label(line);
    }
    let mut open = None;
    // A scroll area starts from the room a modal offered last frame: ask for all of it at once.
    let height = (ui.ctx().content_rect().height() - RESULTS_MARGIN).clamp(MIN_RESULTS_HEIGHT, RESULTS_HEIGHT);
    egui::ScrollArea::vertical().max_height(height).min_scrolled_height(height).auto_shrink([false, true]).show(ui, |ui| {
        for file in &state.files {
            ui.add_space(6.0);
            let name = file.path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            ui.label(egui::RichText::new(name).font(theme::semibold(13.0))).on_hover_text(file.path.to_string_lossy());
            let mut last_page = None;
            for found in &file.matches {
                if last_page != Some(found.page) {
                    ui.label(
                        egui::RichText::new(fmt(tl!("Page {label}"), &[("label", &found.page.saturating_add(1).to_string())]))
                            .small()
                            .color(t.text_faint),
                    );
                    last_page = Some(found.page);
                }
                let job = excerpt(found, ui.available_width(), &t);
                if ui.add(egui::Button::selectable(false, job).wrap()).on_hover_text(tl!("Go to this result")).clicked() {
                    open = Some((file.path.to_string_lossy().into_owned(), found.page));
                }
            }
        }
        if !state.skipped.is_empty() {
            ui.add_space(8.0);
            egui::CollapsingHeader::new(fmt(tl!("Not searched: {n}"), &[("n", &state.skipped.len().to_string())])).show(ui, |ui| {
                for file in state.skipped.iter().take(MAX_SKIPPED_SHOWN) {
                    ui.label(file.path.to_string_lossy());
                    ui.label(egui::RichText::new(reason(&file.reason)).small().color(t.text_muted));
                }
                if state.skipped.len() > MAX_SKIPPED_SHOWN {
                    ui.label(tl!("The rest are left out of this list."));
                }
            });
        }
    });
    if choose {
        choose_folder(app);
    }
    if let Some((path, page)) = open {
        app.open_at_page(&path, page);
        return true;
    }
    close
}

#[cfg(not(target_arch = "wasm32"))]
fn choose_folder(app: &mut PdfCraftApp) {
    let dialog = rfd::AsyncFileDialog::new().set_title(tl!("Search a folder").to_string());
    app.ask_one(crate::pickers::Ask::Folder(dialog), None, |app, folder| {
        app.folder_search.folder = folder.to_string_lossy().into_owned();
    });
}

#[cfg(target_arch = "wasm32")]
fn choose_folder(_app: &mut PdfCraftApp) {}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn running(state: &mut State) -> (mpsc::Sender<Message>, Arc<AtomicBool>) {
        let (sender, receiver) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        state.job = Some(Job { receiver, cancel: cancel.clone() });
        (sender, cancel)
    }

    fn finished(summary: Summary) -> State {
        State { outcome: Some(Ok(summary)), ..State::default() }
    }

    #[test]
    fn stop_asks_a_running_search_to_end_and_it_stays_running_until_it_does() {
        let mut state = State::default();
        let (sender, cancel) = running(&mut state);
        state.stop();
        assert!(cancel.load(Ordering::Relaxed));
        state.poll();
        assert!(state.job.is_some(), "it answers at its next page");
        sender.send(Message::Done(Ok(Summary { stopped: Some(Stop::Cancelled), ..Summary::default() }))).unwrap();
        state.poll();
        assert!(state.job.is_none());
        assert!(state.status().iter().any(|line| line == "Stopped before the end."));
    }

    #[test]
    fn closing_the_dialog_stops_the_search() {
        let mut app = PdfCraftApp::new();
        let (_sender, cancel) = running(&mut app.folder_search);
        let mut h = egui_kittest::Harness::builder().build_eframe(|_cc| app);
        h.run_steps(2);
        assert!(cancel.load(Ordering::Relaxed), "no dialog, so nobody is waiting for the results");
    }

    #[test]
    fn the_search_stops_the_moment_the_dialog_closes() {
        use egui::accesskit::Role;
        use egui_kittest::kittest::Queryable;

        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.dialog = Some(crate::Dialog::FolderSearch);
        let (_sender, cancel) = running(&mut app.folder_search);
        let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| app);
        h.run_steps(3);
        assert!(!cancel.load(Ordering::Relaxed));
        h.query_all_by_role_and_label(Role::Button, "Close").last().unwrap().click();
        for _ in 0..8 {
            h.step();
            if h.state().dialog.is_none() {
                break;
            }
        }
        assert_eq!(h.state().dialog, None);
        assert!(cancel.load(Ordering::Relaxed), "not on a later frame, which a reopened dialog could beat");
    }

    #[test]
    fn results_arrive_while_the_search_runs() {
        let mut state = State::default();
        let (sender, _cancel) = running(&mut state);
        assert_eq!(state.status(), ["Listing the folder…"]);
        sender.send(Message::Event(Event::Found(4))).unwrap();
        sender.send(Message::Event(Event::Reading { index: 1, path: PathBuf::from("/folder/b.pdf") })).unwrap();
        sender.send(Message::Event(Event::Matches(FileMatches { path: "/folder/a.pdf".into(), matches: Vec::new() }))).unwrap();
        sender.send(Message::Event(Event::Skipped(Skipped { path: "/folder/c.pdf".into(), reason: Skip::Secured }))).unwrap();
        state.poll();
        assert_eq!(state.status(), ["Reading file 2 of 4: b.pdf"]);
        assert_eq!(state.files.len(), 1);
        assert_eq!(state.skipped.len(), 1);
    }

    #[test]
    fn a_search_that_dies_says_so() {
        let mut state = State::default();
        drop(running(&mut state));
        state.poll();
        assert!(state.job.is_none());
        assert_eq!(state.status(), ["The search stopped unexpectedly."]);
    }

    #[test]
    fn every_way_a_search_can_end_is_said() {
        let summary = Summary { found: 5, searched: 3, files_with_matches: 2, matches: 7, ..Summary::default() };
        let line = "Matches: 7. Files with matches: 2. PDFs read to the end: 3 of 5.";
        assert_eq!(finished(summary).status(), [line]);
        assert_eq!(finished(Summary { stopped: Some(Stop::MatchLimit), ..summary }).status(), [line, "Stopped at 1000 matches."]);
        assert_eq!(
            finished(Summary { stopped: Some(Stop::TooManySlowFiles), ..summary }).status(),
            [line, "Stopped: too many files took too long to read."]
        );
        assert_eq!(
            finished(Summary { folder_cut_short: true, ..summary }).status(),
            [line, "This folder is too large or too deep to search all of it."]
        );
        assert_eq!(State { outcome: Some(Err("no such folder".into())), ..State::default() }.status(), ["no such folder"]);
        assert!(State::default().status().is_empty(), "nothing to say before a search");
    }

    #[test]
    fn every_reason_a_file_was_left_out_is_said() {
        for (skip, said) in [
            (Skip::LinkOrSpecialFile, "A link or special file, which isn't followed"),
            (Skip::TooLarge, "Larger than 512 MiB"),
            (Skip::Unreadable("denied".into()), "Couldn't be read: denied"),
            (Skip::Damaged("no pages".into()), "Couldn't be read: no pages"),
            (Skip::Secured, "Secured"),
            (Skip::TooSlow, "Took too long to read"),
            (Skip::PagesUnread(2), "Pages that couldn't be read: 2"),
        ] {
            assert_eq!(reason(&skip), said);
        }
    }

    #[test]
    fn a_long_list_of_skipped_files_says_it_was_cut() {
        use egui_kittest::kittest::Queryable;
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.dialog = Some(crate::Dialog::FolderSearch);
        app.folder_search.skipped =
            (0..=MAX_SKIPPED_SHOWN).map(|n| Skipped { path: format!("/folder/{n}.pdf").into(), reason: Skip::Secured }).collect();
        let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| app);
        h.run_steps(4);
        h.get_by_label("Not searched: 201").click();
        h.run_steps(8);
        h.get_by_label("/folder/199.pdf");
        assert!(h.query_by_label("/folder/200.pdf").is_none());
        h.get_by_label("The rest are left out of this list.");
    }
}
