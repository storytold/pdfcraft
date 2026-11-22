//! Opening the Print dialog and running it each pass: the draft is the model, `HostedPrint` is the dialog over it.

use craft::print::ui::{Destination, DriverControls, Duplex as UiDuplex, HostedPrint, PrintLayout, Settings, State};
use craft_ui::patterns::form_dialogs::PagerStep;
use craft_ui::primitives::{ThumbnailOptions, Thumbnails};
use craft_ui::runtime::env::Label;
use pdfcraft_engine::print::{self, spool};

use super::draft::{Handling, PrintDraft, Which};
use super::sheet::SheetInput;
use super::{PREVIEW_MAX_SIDE, PrintStart, body, default_printer, listed_printers, offered, offered_printers, sheet, still_looking};
use crate::PdfCraftApp;
use crate::canvas::pictures::{DocPages, PrintPage};

fn duplex_to_ui(duplex: Option<spool::Duplex>) -> Option<UiDuplex> {
    duplex.map(|d| match d {
        spool::Duplex::Off => UiDuplex::Off,
        spool::Duplex::LongEdge => UiDuplex::LongEdge,
        spool::Duplex::ShortEdge => UiDuplex::ShortEdge,
    })
}

fn duplex_from_ui(duplex: Option<UiDuplex>) -> Option<spool::Duplex> {
    duplex.map(|d| match d {
        UiDuplex::LongEdge => spool::Duplex::LongEdge,
        UiDuplex::ShortEdge => spool::Duplex::ShortEdge,
        // One-sided, and any mode the spooler has no name for, prints one-sided.
        _ => spool::Duplex::Off,
    })
}

/// The job settings the draft holds, as the dialog's `State` keeps them. A draft without a printer is Save as PDF, or
/// (`looking`) a printer not yet known.
fn settings_of(d: &PrintDraft, looking: bool) -> Settings {
    let mut s = Settings::default();
    s.copies = d.copies;
    s.collate = d.collate;
    s.grayscale = d.grayscale;
    s.duplex = duplex_to_ui(d.duplex);
    s.destination = match (&d.printer, looking) {
        (Some(p), _) => Destination::Printer(p.clone()),
        (None, true) => Destination::Printer(String::new()),
        (None, false) => Destination::File,
    };
    s
}

impl PdfCraftApp {
    /// Open the Print dialog, preselecting the default printer and reading papers in the background.
    pub fn open_print(&mut self) {
        let Some((i, id)) = self.active_ids() else { return };
        #[cfg(not(target_arch = "wasm32"))]
        let (printers, listening) = if self.printer_source.is_some() {
            // The list from the last time the dialog opened, until the fresh one arrives.
            let receiver = self.list_printers();
            (self.print_tasks.listed.clone(), Some(receiver))
        } else {
            (self.sync_listed(), None)
        };
        #[cfg(target_arch = "wasm32")]
        let printers = self.sync_listed();

        let current = self.views[i].current();
        let selected: Vec<usize> = self.views[i].selected.items.iter().copied().collect();
        let mut keep = std::mem::take(&mut self.print_draft);
        // Leaving Label gives back the paper it replaced (#3564), before the layout resets below.
        if keep.handling == Handling::Label {
            keep.set_handling(Handling::Size);
        }
        let default = default_printer(&keep, &printers);
        // The printer picked last time, Save as PDF included, while it is still listed (#3537).
        let picked = keep.printer_picked && offered(keep.printer.as_deref(), &printers);
        let printer = if picked { keep.printer.clone() } else { default };
        // A printer other than last time's brings back what it was last printed with (#2001);
        // the same one keeps this session's choices.
        let changed = printer != keep.printer;
        // Picked pages are what Print is for (Acrobat's "Selected pages"); without any, the whole document.
        let which = if selected.is_empty() { Which::All } else { Which::Selected };
        // Copies, pages, the layout with its own options and orientation are this job's, and start from the defaults
        // each time the dialog opens: ISO 32000-2 Table 147 initialises them per document when the print dialog opens
        // (NumCopies, "typically 1"; PrintPageRange). The choices that describe how this computer prints (two-sided,
        // grayscale, collate, sizing, Comments & forms, a booklet's binding) carry over, and are remembered across
        // restarts (#2001).
        let fresh = PrintDraft::default();
        self.print_draft = PrintDraft {
            copies: fresh.copies,
            range: fresh.range,
            subset: fresh.subset,
            reverse: fresh.reverse,
            handling: fresh.handling,
            per_sheet: fresh.per_sheet,
            custom_grid: fresh.custom_grid,
            cols: fresh.cols,
            rows: fresh.rows,
            order: fresh.order,
            border: fresh.border,
            auto_rotate: fresh.auto_rotate,
            booklet_subset: fresh.booklet_subset,
            poster_scale: fresh.poster_scale,
            overlap: fresh.overlap,
            cut_marks: fresh.cut_marks,
            label_flip: fresh.label_flip,
            orientation: fresh.orientation,
            crops: Vec::new(),
            crop_pages: None,
            printer_picked: picked,
            units: self.units,
            printer: printer.clone(),
            current_page: current,
            sheet: 0,
            selected,
            which,
            area: None,
            custom_new: None,
            ..keep
        };
        if changed {
            self.print_draft.apply_setup();
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(p) = printer {
            self.read_papers_in_background(p);
        }

        let mut state = State::default();
        state.printers = printers;
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(receiver) = listening {
            state.listen(receiver);
        }
        let looking = still_looking(&self.print_draft, state.discovering());
        state.settings = settings_of(&self.print_draft, looking);

        let Some((sizes, _)) = self.page_geometry(id) else { return };
        let fields = body::fields_from_draft(&self.print_draft);
        let no_printers = state.printers.is_empty() && !state.discovering();
        let sections = body::build(&self.print_draft, &sizes, self.units, self.print_draft.area.is_some(), no_printers);
        let mut dialog = HostedPrint::new(state, fields, sections).layout(PrintLayout::Document).held_ok();
        dialog.set_driver(DriverControls::Properties);
        // PLAN: assumed `dialogs::host::open<D: StackDialog>(app: &mut PdfCraftApp, dialog: D)` (pdf-dialogs-w4).
        crate::dialogs::host::open(self, dialog);
    }

    /// The printers the spooler lists right now, recorded as the last list.
    fn sync_listed(&mut self) -> Vec<craft::print::ui::Printer> {
        let list = listed_printers();
        self.record_listed(&list);
        offered_printers(&list)
    }

    /// The page sizes and labels of the document.
    fn page_geometry(&self, id: pdfcraft_engine::DocId) -> Option<(Vec<(f64, f64)>, Vec<String>)> {
        let doc = self.session.get(id)?;
        let sizes = doc.info.pages.iter().map(|p| (f64::from(p.width), f64::from(p.height))).collect();
        let labels = doc.info.pages.iter().map(|p| p.label.clone()).collect();
        Some((sizes, labels))
    }

    /// Asks for a fresh printer list: the app's source reads it in the background, without one the spooler is asked now.
    /// A list already being read is the fresh one (the dialog's first pass asks too, as `State::refresh` refuses to
    /// while it discovers): reading again would replace the job that owns the dialog's receiver.
    fn refresh_printers(&mut self) {
        if self.dialogs.get::<HostedPrint>().is_some_and(|dialog| dialog.state().discovering()) {
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        if self.printer_source.is_some() {
            let receiver = self.list_printers();
            if let Some(dialog) = self.dialogs.get_mut::<HostedPrint>() {
                dialog.listen(receiver);
            }
            return;
        }
        let printers = self.sync_listed();
        if let Some(dialog) = self.dialogs.get_mut::<HostedPrint>() {
            dialog.state_mut().printers = printers;
        }
    }

    /// Runs the open Print dialog for this pass, in this order: the printer list asked for, the dialog's job settings
    /// and edited rows read into the draft, the printer change, the actions, the pager step, the label crops, the layout
    /// and the sheet, and last the held Print answered.
    ///
    /// Each request the dialog holds (`take_edited`, `take_action`, `take_preview_step`, `take_ok`) is returned once,
    /// so a second pass of the same frame applies none of them again.
    ///
    /// The actions (one namespace): `custom`, `custom.add`, `custom.cancel`, `paper.remove`, `paper.setup`,
    /// `paper.match`, `driver.done`, `driver.reset`; the panel's Properties… is `take_settings_clicked`. The row keys, as
    /// `set_json_field` and `fields_mut`
    /// take them: `which`, `range`, `subset`, `reverse`, `handling`, `per_sheet`, `as_image` and the others of
    /// `body::fields_from_draft`; the printer is `settings.destination`.
    pub(crate) fn print_pass(&mut self, ctx: &egui::Context) {
        let Some((_, id)) = self.active_ids() else {
            self.dialogs.cancel::<HostedPrint>(ctx);
            return;
        };
        let Some(dialog) = self.dialogs.get_mut::<HostedPrint>() else { return };
        let refresh = dialog.take_refresh();
        let settings = dialog.state().settings.clone();
        let chosen = dialog.take_printer_chosen();
        let edited = dialog.requests_mut().take_edited();
        let fields = dialog.fields_mut().clone();
        let action = dialog.take_action();
        // PLAN: assumed HostedPrint::take_settings_clicked(&mut self) -> bool (F-1)
        let settings_clicked = dialog.take_settings_clicked();
        let step = dialog.take_preview_step();
        let paper_px = dialog.take_paper_px();
        let ok = dialog.take_ok();
        let no_printers = dialog.state().printers.is_empty() && !dialog.state().discovering();
        if refresh {
            self.refresh_printers();
        }
        let Some((sizes, labels)) = self.page_geometry(id) else { return };

        // The dialog's settings and edited rows into the draft.
        let d = &mut self.print_draft;
        let before = d.printer.clone();
        d.copies = settings.copies;
        d.collate = settings.collate;
        d.grayscale = settings.grayscale;
        d.duplex = duplex_from_ui(settings.duplex);
        d.printer = match &settings.destination {
            Destination::Printer(name) if !name.is_empty() => Some(name.clone()),
            _ => None,
        };
        if chosen.is_some() {
            d.printer_picked = true;
        }
        body::draft_from_fields(d, &fields, &edited);
        let printer_changed = chosen.is_some() && d.printer != before;
        if printer_changed {
            d.show_driver_options = false;
            d.driver_error = None;
            // What this printer was last printed with (#2001).
            d.apply_setup();
            d.reset_unoverridden_custom_paper();
            d.apply_printer_papers(Vec::new());
        }
        #[cfg(not(target_arch = "wasm32"))]
        if printer_changed && let Some(p) = self.print_draft.printer.clone() {
            self.read_papers_in_background(p);
        }

        if let Some(action) = action {
            self.print_action(&action, &sizes);
        }
        if settings_clicked {
            self.toggle_driver_options();
        }
        let d = &mut self.print_draft;
        match step {
            Some(PagerStep::Back) => d.sheet = d.sheet.saturating_sub(1),
            Some(PagerStep::Next) => d.sheet = d.sheet.saturating_add(1),
            None => {}
        }
        if d.handling == Handling::Label {
            self.read_label_crops(id);
        }

        // The layout, once per pass; the sheet index is clamped here.
        let d = &mut self.print_draft;
        let settings_result = d.settings(sizes.len(), &labels);
        let laid = settings_result.as_ref().ok().map(|s| print::layout(&sizes, s));
        // Creep within its field's range can still be too much for a thick booklet.
        let too_much_creep = d.handling == Handling::Booklet && d.creep > 0.0 && matches!(laid, Some(Err(_)));
        let sheets = laid.and_then(Result::ok).unwrap_or_default();
        d.sheet = d.sheet.min(sheets.len().saturating_sub(1));

        let thumbs = Thumbnails::<PrintPage>::in_context(ctx, "print-preview", ThumbnailOptions::new().max_side(PREVIEW_MAX_SIDE as u32));
        // PLAN: assumed DocPages::new(&Session, DocId) -> DocPages (pdf-canvas-w10)
        let pages = DocPages::new(&self.session, id);
        let input = SheetInput {
            d: &self.print_draft,
            sizes: &sizes,
            labels: &labels,
            settings: &settings_result,
            sheets: &sheets,
            too_much_creep,
            paper_px,
        };
        let (sheet, pager) = sheet::build(input, &thumbs, &pages, ctx);
        // Print waits while the page range or another setting names no job.
        let settings_valid = settings_result.is_ok();
        let d = &self.print_draft;
        let new_fields = body::fields_from_draft(d);
        let sections = body::build(d, &sizes, self.units, d.area.is_some(), no_printers);
        let (duplex, booklet) = (duplex_to_ui(d.duplex), d.handling == Handling::Booklet);

        let Some(dialog) = self.dialogs.get_mut::<HostedPrint>() else { return };
        let rows = dialog.fields_mut();
        for (key, value) in new_fields.iter() {
            if rows.get(key) != Some(value) {
                rows.set(key, value.clone());
            }
        }
        // `apply_setup` can change two-sided; the dialog's other settings were just read from it.
        dialog.state_mut().settings.duplex = duplex;
        dialog.set_sections(sections);
        dialog.set_sheet(sheet);
        dialog.set_preview_pager(pager);
        dialog.set_duplex_by_layout(booklet);
        dialog.set_settings_valid(settings_valid);

        if ok.is_some() {
            match self.print_now(ctx) {
                Ok(PrintStart::Sent) => self.answer_print(Ok(())),
                // The Save as PDF continuation answers the held OK.
                Ok(PrintStart::Picking) => {}
                Err(reason) => self.answer_print(Err(Label::from(reason))),
            }
        }
    }

    /// Shows the printer the app chose outside the dialog (a fresh list's default, the one printed to last) and the
    /// two-sided choice its setup brought back in the open dialog, whose settings the next pass reads into the draft:
    /// the dialog's own fallback for a printer the list doesn't have would otherwise win.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn show_chosen_printer(&mut self) {
        let Some(printer) = self.print_draft.printer.clone() else { return };
        let duplex = duplex_to_ui(self.print_draft.duplex);
        let Some(dialog) = self.dialogs.get_mut::<HostedPrint>() else { return };
        let settings = &mut dialog.state_mut().settings;
        settings.destination = Destination::Printer(printer);
        settings.duplex = duplex;
    }

    fn answer_print(&mut self, answer: Result<(), Label>) {
        if let Some(dialog) = self.dialogs.get_mut::<HostedPrint>() {
            dialog.answer_ok(answer);
        }
    }

    /// One action a row or a title-row button of the dialog filed.
    fn print_action(&mut self, action: &str, sizes: &[(f64, f64)]) {
        let units = self.units;
        if action == "paper.setup" {
            self.page_setup();
            return;
        }

        let d = &mut self.print_draft;
        match action {
            "custom" => {
                d.custom_new = match d.custom_new {
                    Some(_) => None,
                    None => Some((String::new(), String::new(), String::new())),
                };
            }
            "custom.add" => {
                if let Some(paper) = body::typed_custom_paper(d, units) {
                    d.add_custom_paper(paper);
                    d.custom_new = None;
                }
            }
            "custom.cancel" => d.custom_new = None,
            "paper.remove" => {
                d.remove_custom_paper();
                d.paper_overridden = true;
            }
            "paper.match" => {
                if let Some(paper) = sizes.get(d.current_page).and_then(|&source| d.matching_paper(source)) {
                    d.paper = paper;
                    d.paper_overridden = true;
                    // A paper picked outright: nothing for leaving Label to give back.
                    d.paper_before_label = None;
                    d.handling = Handling::Size;
                    d.size = print::SizeMode::Actual;
                    d.orientation = print::Orientation::Auto;
                    d.sheet = 0;
                }
            }
            "driver.done" => d.show_driver_options = false,
            "driver.reset" => d.driver_choices.clear(),
            _ => {}
        }
    }

    /// Page Setup…: the driver's paper and orientation dialog, owned by the main window.
    fn page_setup(&mut self) {
        // PLAN: assumed `main_window_handle(&self) -> Option<isize>`, the window service's accessor for the main window.
        #[cfg(windows)]
        let parent = self.main_window_handle();
        #[cfg(not(windows))]
        let parent = None;
        let d = &mut self.print_draft;
        let Some(p) = d.printer.clone() else { return };
        d.driver_error = None;
        match spool::show_printer_properties(&p, parent) {
            Ok(Some((name, size))) => d.apply_driver_paper(name, size),
            Ok(None) => {}
            Err(e) => d.driver_error = Some(e.to_string()),
        }
    }

    /// Properties…: the driver's preferences on Windows, otherwise the driver's options for this print, read off the
    /// GUI thread (#1818).
    fn toggle_driver_options(&mut self) {
        let d = &mut self.print_draft;
        let Some(p) = d.printer.clone() else { return };
        if spool::HAS_PRINTER_PREFERENCES {
            d.driver_error = spool::open_printer_preferences(&p).err().map(|e| e.to_string());
            return;
        }
        let read = d.driver_options.as_ref().is_none_or(|(read_for, _)| *read_for != p) && d.driver_options_loading.as_deref() != Some(p.as_str());
        if read {
            d.driver_options = None;
            d.driver_choices.clear();
            d.driver_options_loading = Some(p.clone());
        }
        d.show_driver_options = !d.show_driver_options;
        #[cfg(not(target_arch = "wasm32"))]
        if read {
            self.read_options_in_background(p);
        }
    }
}
