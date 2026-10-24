//! Local RoboDhan workspace. Network requests run off the editor thread.
use serde_json::{Value, json};
use std::sync::mpsc::{self, Receiver};

pub struct Studio {
    pub visible: bool,
    pub prompt: String,
    pub language: String,
    pub result: String,
    pub status: String,
    pending: Option<Receiver<Result<String, String>>>,
}
impl Default for Studio {
    fn default() -> Self {
        Self { visible: false, prompt: String::new(), language: "Indonesia".into(), result: String::new(), status: String::new(), pending: None }
    }
}
pub enum Action {
    Generate,
    Apply,
    Save,
}
impl Studio {
    pub fn draw(&mut self, ui: &mut egui::Ui, title: &str, apply: &str) -> Option<Action> {
        if let Some(rx) = &self.pending {
            match rx.try_recv() {
                Ok(result) => {
                    self.pending = None;
                    match result {
                        Ok(text) => {
                            self.result = text;
                            self.status = "Hasil siap ditinjau.".into();
                        }
                        Err(error) => self.status = error,
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.pending = None;
                    self.status = "Proses AI terputus.".into();
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
                }
            }
        }
        egui::Panel::top("robodhan-toggle").show(ui, |ui| {
            ui.horizontal(|ui| {
                #[cfg(target_os = "macos")]
                ui.add_space(100.0);
                let toggle_label = if self.visible { "Sembunyikan AI Studio" } else { "Tampilkan AI Studio" };
                ui.toggle_value(&mut self.visible, toggle_label);
                if self.pending.is_some() {
                    ui.spinner();
                }
            });
        });
        if !self.visible {
            return None;
        }
        let mut action = None;
        egui::Panel::right("robodhan-studio").resizable(true).default_size(310.0).show(ui, |ui| {
            ui.heading("AI Studio · RoboDhan");
            ui.label(title);
            ui.horizontal(|ui| {
                ui.label("Bahasa hasil:");
                ui.text_edit_singleline(&mut self.language);
            });
            ui.add(
                egui::TextEdit::multiline(&mut self.prompt)
                    .desired_rows(3)
                    .desired_width(f32::INFINITY)
                    .char_limit(4000)
                    .hint_text("Instruksi tambahan…"),
            );
            if ui.add_enabled(self.pending.is_none(), egui::Button::new("Proses dengan AI lokal")).clicked() {
                action = Some(Action::Generate);
            }
            ui.label(&self.status);
            ui.separator();
            egui::ScrollArea::vertical().max_height(450.0).show(ui, |ui| {
                ui.add(egui::TextEdit::multiline(&mut self.result).desired_rows(14).desired_width(f32::INFINITY).char_limit(100_000));
            });
            ui.horizontal_wrapped(|ui| {
                let ready = self.pending.is_none() && !self.result.trim().is_empty();
                if ui.add_enabled(ready, egui::Button::new("Salin hasil")).clicked() {
                    ui.ctx().copy_text(self.result.clone());
                }
                if ui.add_enabled(ready, egui::Button::new("Simpan hasil…")).clicked() {
                    action = Some(Action::Save);
                }
                if !apply.is_empty() && ui.add_enabled(ready, egui::Button::new(apply)).clicked() {
                    action = Some(Action::Apply);
                }
            });
            ui.small("API lokal 127.0.0.1:1234. Model mengikuti RoboDhan. Hasil tidak diterapkan otomatis.");
        });
        action
    }
    pub fn start(&mut self, ctx: &egui::Context, job: impl FnOnce() -> Result<String, String> + Send + 'static) {
        if self.pending.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        match std::thread::Builder::new().name("robodhan-studio".into()).spawn(move || {
            let result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(job)).unwrap_or_else(|_| Err("Proses AI gagal; dokumen tetap aman.".into()));
            let _ = tx.send(result);
            ctx.request_repaint();
        }) {
            Ok(_) => {
                self.pending = Some(rx);
                self.status = "Menunggu RoboDhan…".into();
            }
            Err(error) => self.status = error.to_string(),
        }
    }
}

pub fn ask(task: &str, prompt: String, language: String, source: String) -> Result<String, String> {
    if source.len() > 240_000 {
        return Err("Sumber terlalu panjang (maksimum 240 KB). Pilih bagian lebih kecil.".into());
    }
    if !matches!(task, "pdf.summary" | "word.translate") || source.chars().count() <= 10_000 {
        return ask_once(task, prompt, language, source);
    }
    // Bound each local-model context; never silently omit the rest of a document.
    let mut chunks = Vec::new();
    let mut chunk = String::new();
    for (index, ch) in source.chars().enumerate() {
        chunk.push(ch);
        if (index + 1).is_multiple_of(10_000) {
            chunks.push(std::mem::take(&mut chunk));
        }
    }
    if !chunk.is_empty() {
        chunks.push(chunk);
    }
    let mut results = Vec::new();
    for (index, chunk) in chunks.into_iter().enumerate() {
        results.push(ask_once(task, format!("Bagian {}. {}", index + 1, prompt), language.clone(), chunk)?);
    }
    let joined = results.join("\n\n");
    if task == "pdf.summary" {
        if joined.len() > 240_000 {
            return Err("Ringkasan antara terlalu panjang. Ringkas dokumen yang lebih pendek.".into());
        }
        ask_once(task, format!("Gabungkan ringkasan bagian berikut tanpa menghilangkan kesimpulan dan referensi halaman. {prompt}"), language, joined)
    } else {
        Ok(joined)
    }
}

fn ask_once(task: &str, prompt: String, language: String, source: String) -> Result<String, String> {
    use std::io::Read;
    if source.len() > 240_000 {
        return Err("Sumber terlalu panjang (maksimum 240 KB). Pilih bagian lebih kecil.".into());
    }
    let agent: ureq::Agent =
        ureq::Agent::config_builder().proxy(None).max_redirects(0).timeout_global(Some(std::time::Duration::from_secs(600))).build().into();
    let body = json!({"task":task,"prompt":prompt,"language":language,"source":source});
    let mut response = agent
        .post("http://127.0.0.1:1234/v1/craft/tasks")
        .header("Content-Type", "application/json")
        .send(body.to_string())
        .map_err(|e| format!("RoboDhan: {e}. Aktifkan API lokal dan gunakan RoboDhan versi integrasi Craft."))?;
    let mut bytes = Vec::new();
    response.body_mut().as_reader().take(512 * 1024 + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() > 512 * 1024 {
        return Err("Respons AI terlalu besar.".into());
    }
    let value: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    value["choices"][0]["message"]["content"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().to_owned())
        .ok_or_else(|| "RoboDhan tidak mengembalikan hasil.".into())
}

pub fn save(text: &str, extension: &str) -> Result<Option<std::path::PathBuf>, String> {
    let Some(path) = rfd::FileDialog::new().set_file_name(format!("hasil-ai.{extension}")).add_filter("Hasil AI", &[extension]).save_file() else {
        return Ok(None);
    };
    use std::io::Write;
    let parent = path.parent().ok_or("Folder tujuan tidak valid")?;
    static NEXT_TEMP: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let stamp = NEXT_TEMP.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let temporary = parent.join(format!(".craft-ai-{}-{stamp}.tmp", std::process::id()));
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&temporary).map_err(|e| e.to_string())?;
    let written = (|| -> std::io::Result<()> {
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, &path)
    })();
    if let Err(error) = written {
        let _ = std::fs::remove_file(&temporary);
        return Err(error.to_string());
    }
    Ok(Some(path))
}

pub fn show(app: &mut crate::PdfCraftApp, ui: &mut egui::Ui) {
    let mut state = std::mem::take(&mut app.ai_studio);
    match state.draw(ui, "Ringkas PDF aktif dengan referensi halaman. PDF scan perlu OCR terlebih dahulu.", "Simpan & buka di WordCraft…") {
        Some(Action::Generate) => {
            let source = app.active.and_then(|i| app.views.get(i)).and_then(|view| app.session.get(view.id));
            match source {
                None => state.status = "Buka PDF terlebih dahulu.".into(),
                Some(doc) if !doc.allows_copying() => state.status = "Dokumen ini membatasi penyalinan teks.".into(),
                Some(doc) => {
                    let source = doc.export_source();
                    let (prompt, language) = (state.prompt.clone(), state.language.clone());
                    state.start(ui.ctx(), move || {
                        let pages = source.pages;
                        if pages > 500 {
                            return Err("Maksimum 500 halaman per ringkasan. Pecah dokumen terlebih dahulu.".into());
                        }
                        let mut exporter = pdfcraft_engine::export::Exporter::from_source(source);
                        let mut text = String::new();
                        let mut letters = 0usize;
                        for page in 0..pages {
                            let page_text = exporter.text(page)?;
                            letters = letters.saturating_add(page_text.chars().filter(|c| c.is_alphabetic()).count());
                            text.push_str(&format!("\n[Halaman {}]\n{}", page + 1, page_text));
                            if text.len() > 240_000 {
                                return Err("Teks PDF melebihi 240 KB. Pecah dokumen agar seluruh isi dapat diringkas.".into());
                            }
                        }
                        if letters < 20 {
                            return Err("PDF tidak memiliki cukup teks. Jalankan OCR terlebih dahulu.".into());
                        }
                        ask("pdf.summary", prompt, language, text)
                    });
                }
            }
        }
        Some(Action::Save) => {
            if let Err(e) = save(&state.result, "txt") {
                state.status = e;
            }
        }
        Some(Action::Apply) => match save(&state.result, "txt") {
            Ok(Some(path)) => {
                #[cfg(target_os = "macos")]
                let launched = {
                    let sibling = std::env::current_exe()
                        .ok()
                        .and_then(|exe| exe.ancestors().nth(4).map(|folder| folder.join("WordCraft.app")))
                        .filter(|app| app.is_dir());
                    let target = sibling.unwrap_or_else(|| std::path::PathBuf::from("WordCraft"));
                    std::process::Command::new("open").arg("-a").arg(target).arg(&path).status().map(|s| s.success())
                };
                #[cfg(target_os = "windows")]
                let launched = std::process::Command::new("WordCraft.exe").arg(&path).spawn().map(|_| true);
                #[cfg(not(any(target_os = "macos", target_os = "windows")))]
                let launched = std::process::Command::new("wordcraft").arg(&path).spawn().map(|_| true);
                state.status = if launched.unwrap_or(false) {
                    "Resume dikirim ke WordCraft.".into()
                } else {
                    format!("Resume tersimpan di {}. Pasang WordCraft di Applications / PATH lalu buka berkas ini.", path.display())
                };
            }
            Err(e) => state.status = e,
            _ => {}
        },
        None => {}
    }
    app.ai_studio = state;
}

#[cfg(test)]
mod studio_tests {
    use super::*;
    #[test]
    fn hidden_panel_keeps_prompt_and_pending_result() {
        let mut state = Studio { prompt: "keep this".into(), ..Default::default() };
        let (tx, rx) = mpsc::channel();
        state.pending = Some(rx);
        tx.send(Ok("completed".into())).unwrap();
        let mut frame = egui::Context::default().run_ui(Default::default(), |ui| {
            state.draw(ui, "test", "");
        });
        frame.textures_delta.clear();
        assert!(!state.visible);
        assert_eq!(state.prompt, "keep this");
        assert_eq!(state.result, "completed");
        assert!(state.pending.is_none());
    }
}
