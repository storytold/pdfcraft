//! The question asked before a save while redaction marks aren't applied (#4741).

use craft::ui::dialog::{DialogResult, PromptChoice, StandardPrompt};

use crate::PdfCraftApp;
use crate::editing::PendingRedactSave;

/// The question's sentence for `marks` marks in document `name`: the count picks the form by the
/// language's plural rules, as every counted message does ([`crate::i18n::trn`]).
pub(crate) fn question_text(name: &str, marks: usize) -> String {
    let n = u64::try_from(marks).unwrap_or(u64::MAX);
    let said = crate::i18n::trn(
        crate::i18n::current(),
        n,
        "“{name}” has {n} redaction mark that isn't applied yet. Until it is, the text and images under it stay in the file, where anyone can read or copy them.",
        "“{name}” has {n} redaction marks that aren't applied yet. Until they are, the text and images under them stay in the file, where anyone can read or copy them.",
    );
    let said = crate::i18n::fmt(&said, &[("name", name)]);
    let rest = tl!("Redact all removes that content for good. Save with marks keeps them to apply later.");
    format!("{said}\n\n{rest}")
}

/// Asks whether to apply the marks before the save `pending` holds. The save waits in
/// `pending_redact_save` until [`finish_ask`] answers.
pub(crate) fn ask_before_save(app: &mut PdfCraftApp, pending: PendingRedactSave) {
    let id = pending.id;
    let (name, marks) = app.session.get(id).map(|d| (d.name.clone(), d.redaction_marks())).unwrap_or_default();
    app.pending_redact_save = Some(pending);
    let prompt = StandardPrompt::custom(question_text(&name, marks))
        .title(tl!("Apply the redaction marks before saving?").to_string())
        .button(PromptChoice::Proceed, tl!("Redact all and save").to_string())
        .button(PromptChoice::Alternate, tl!("Save with marks").to_string())
        .button(PromptChoice::Cancel, tl!("Cancel").to_string())
        .alternate_role(craft::ui::sections::DialogRole::Alternative)
        .tag(id)
        .instance("redact-before-save");
    app.prompts.redact_save = Some(crate::dialogs::host::open(app, prompt));
}

/// The question's answer. Redact all and save applies the marks, then saves; Save with marks saves
/// them as they are; anything else drops the save, which counts as one that did not happen.
pub(crate) fn finish_ask(ctx: &egui::Context, app: &mut PdfCraftApp, result: DialogResult<craft::ui::dialog::PromptAnswer>) {
    match result {
        DialogResult::Ok(answer) if answer.choice == PromptChoice::Proceed => app.redact_then_save(ctx),
        DialogResult::Ok(answer) if answer.choice == PromptChoice::Alternate => app.save_keeping_redaction_marks(ctx),
        _ => {
            if let Some(p) = app.pending_redact_save.take() {
                p.dismissed(app, ctx);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use craft::host::{AppInfo, Host, ManualJobs, NoPickFiles, Wake};
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    use super::*;
    use crate::SaveTarget;

    /// A one-page PDF with a proper xref table.
    fn fixture() -> Vec<u8> {
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 300] >>",
            "<< /Type /Page /Parent 2 0 R >>",
        ];
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for o in offsets {
            out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes());
        out
    }

    /// The app with a one-page file open, whose first page carries one redaction mark.
    struct Rig {
        h: Harness<'static, crate::PdfCraftApp>,
        jobs: Arc<ManualJobs>,
        dir: std::path::PathBuf,
        path: String,
    }

    impl Rig {
        fn new(test: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("pdfcraft-redact-save-{test}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("marked.pdf").to_string_lossy().into_owned();
            std::fs::write(&path, fixture()).unwrap();
            let jobs = Arc::new(ManualJobs::new());
            let info = AppInfo { id: "pdfcraft", name: "PdfCraft", version: env!("CARGO_PKG_VERSION") };
            let host = Arc::new(Host::builder(info).jobs(jobs.clone()).files(Arc::new(NoPickFiles::default())).build());
            let (intake, _) = craft::platform::open::FileIntake::new(Wake::none());
            let clipboard = craft::platform::clipboard::Clipboard::new(craft::platform::clipboard::EventClipboard::new(|| 0, |_| Ok(0)));
            let services =
                crate::AppServices::new(Wake::none(), clipboard, None, None, intake, craft::platform::watch::FileWatch::new(Wake::none()).inline());
            let open = path.clone();
            let h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |cc| {
                crate::PdfCraftApp::install_runtime(&cc.egui_ctx);
                let mut app = crate::PdfCraftApp::with_host(host, services);
                app.set_option(&cc.egui_ctx, "language", "en").unwrap();
                app.open_path(&open);
                app
            });
            let mut rig = Self { h, jobs, dir, path };
            rig.steps(4);
            rig.h.state_mut().redact_pages(&egui::Context::default(), &[0]);
            rig.steps(2);
            assert_eq!(rig.marks(), 1, "the page is marked");
            rig
        }

        /// `n` frames, running the jobs the app queued before each one.
        fn steps(&mut self, n: usize) {
            for _ in 0..n {
                self.jobs.run_all();
                self.h.run_steps(1);
            }
        }

        fn marks(&self) -> usize {
            let app = self.h.state();
            app.session.get(app.views[0].id).map_or(0, |d| d.redaction_marks())
        }

        /// Save in place and settle; whether the question about the marks is on screen.
        fn save_and_ask(&mut self) -> bool {
            self.h.state_mut().save_active(&egui::Context::default(), SaveTarget::InPlace);
            self.steps(3);
            self.h.state().prompts.redact_save.is_some()
        }
    }

    impl Drop for Rig {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// The count picks the sentence's form: one mark and several, through the catalog's plural
    /// rules rather than a hand-written `if`.
    #[test]
    fn the_question_counts_one_mark_and_several() {
        let one = question_text("a.pdf", 1);
        assert!(one.contains("“a.pdf” has 1 redaction mark that isn't applied yet."), "{one}");
        let two = question_text("a.pdf", 2);
        assert!(two.contains("“a.pdf” has 2 redaction marks that aren't applied yet."), "{two}");
        let many = question_text("a.pdf", 12);
        assert!(many.contains("has 12 redaction marks"), "{many}");
    }

    /// Save anyway, as the question words it: Save with marks writes the marks as they are, and
    /// the next Save of the unchanged document does not ask again.
    #[test]
    fn save_with_marks_writes_the_marks_and_does_not_ask_again() {
        let mut r = Rig::new("keep");
        assert!(r.save_and_ask(), "marks that aren't applied are asked about");
        r.h.get_by_label("Save with marks").click();
        r.steps(4);
        assert_eq!(r.marks(), 1, "the marks are kept as marks");
        assert!(!r.h.state().session.get(r.h.state().views[0].id).unwrap().dirty, "the document was written");
        assert!(!r.save_and_ask(), "unchanged since, the next Save does not ask again");
    }

    /// Redact all and save applies the marks first, then writes.
    #[test]
    fn redact_all_and_save_applies_the_marks_then_writes() {
        let mut r = Rig::new("apply");
        assert!(r.save_and_ask());
        r.h.get_by_label("Redact all and save").click();
        r.steps(6);
        assert_eq!(r.marks(), 0, "the marks were applied");
    }

    /// Cancel writes nothing, and the next Save asks again.
    #[test]
    fn cancel_writes_nothing_and_the_next_save_asks_again() {
        let mut r = Rig::new("cancel");
        let before = std::fs::read(&r.path).unwrap();
        assert!(r.save_and_ask());
        r.h.get_by_label("Cancel").click();
        r.steps(4);
        assert_eq!(std::fs::read(&r.path).unwrap(), before, "nothing was written");
        assert!(r.save_and_ask(), "asked again");
    }
}
