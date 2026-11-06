//! Real-shell controls with injected audio: tests must never speak a document automatically.
use egui_kittest::{Harness, kittest::Queryable};
use pdfcraft_platform::speech::{Backend, Options, Player, State, Voice};
use pdfcraft_ui_egui::PdfCraftApp;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const PDF:&[u8]=b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 100] >> endobj\n3 0 obj << /Type /Page /Parent 2 0 R /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >> endobj\n4 0 obj << /Length 33 >> stream\nBT /F1 12 Tf 20 50 Td (Hello) Tj ET\nendstream endobj\n5 0 obj << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> endobj\ntrailer << /Root 1 0 R >>\n%%EOF";

struct Fake(Arc<Mutex<Vec<String>>>);
impl Backend for Fake {
    /// Expose deterministic English/Italian voices without querying native services.
    fn voices(&mut self) -> Result<Vec<Voice>, String> {
        Ok(vec![
            Voice { id: "fake".into(), name: "Test voice".into(), language: "en-US".into() },
            Voice { id: "fake-it".into(), name: "Test Italian voice".into(), language: "it-IT".into() },
        ])
    }
    /// Record synthetic text and the selected voice instead of producing audio.
    fn start(&mut self, text: &str, options: &Options) -> Result<(), String> {
        self.0.lock().unwrap().push(text.into());
        if let Some(voice) = &options.voice {
            self.0.lock().unwrap().push(format!("voice={voice}"));
        }
        Ok(())
    }
    /// Record the UI pause request for assertions.
    fn pause(&mut self) -> Result<(), String> {
        self.0.lock().unwrap().push("pause".into());
        Ok(())
    }
    /// Record the UI resume request without replaying text.
    fn resume(&mut self) -> Result<(), String> {
        self.0.lock().unwrap().push("resume".into());
        Ok(())
    }
    /// Record playback cancellation without touching an audio device.
    fn stop(&mut self) -> Result<(), String> {
        self.0.lock().unwrap().push("stop".into());
        Ok(())
    }
    /// Keep the fake chunk active until the test explicitly stops it.
    fn finished(&mut self) -> Result<bool, String> {
        Ok(false)
    }
}

/// Verify interface language does not override the explicitly selected speech voice.
#[test]
fn read_aloud_voice_language_is_independent_of_english_or_italian_interface() {
    for (language, voice, button) in [("it", "fake", "Leggi ad alta voce"), ("en", "fake-it", "Read out loud")] {
        let log = Arc::new(Mutex::new(Vec::new()));
        let backend = log.clone();
        let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
            let mut app = PdfCraftApp::new();
            app.set_option("language", language).unwrap();
            app.open_bytes("voice.pdf", None, PDF.to_vec()).unwrap();
            app.read_aloud.player = Player::with_backend(Box::new(Fake(backend.clone())));
            app.read_aloud.options.voice = Some(voice.into());
            app
        });
        assert!(h.state_mut().execute("a11y.read_aloud"));
        h.run_steps(4);
        h.get_by_role_and_label(egui::accesskit::Role::Button, button).click();
        wait(&mut h, State::Speaking);
        assert!(log.lock().unwrap().contains(&format!("voice={voice}")));
        assert!(log.lock().unwrap().contains(&"Hello".into()));
        h.state_mut().stop_read_aloud();
        wait(&mut h, State::Idle);
        assert!(!h.state().session.get(h.state().active_ids().unwrap().1).unwrap().dirty);
    }
}

/// Drive UI frames until the expected speech state appears, with a five-second deadline.
fn wait(h: &mut Harness<'static, PdfCraftApp>, state: State) {
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        h.run_steps(2);
        if h.state().read_aloud.player.status().state == state {
            break;
        }
        assert!(Instant::now() < until, "{:?}", h.state().read_aloud.player.status());
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Verify dialog playback controls preserve document bytes and closing the source stops speech.
#[test]
fn read_aloud_shell_controls_preserve_the_pdf_and_close_stops_audio() {
    let shot = std::env::var_os("PDFCRAFT_SPEECH_SHOT");
    let _gpu = shot.as_ref().map(|_| super::gpu());
    let log = Arc::new(Mutex::new(Vec::new()));
    let backend = log.clone();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("voice.pdf", None, PDF.to_vec()).unwrap();
        app.read_aloud.player = Player::with_backend(Box::new(Fake(backend.clone())));
        app
    });
    h.run_steps(3);
    let id = h.state().active_ids().unwrap().1;
    let bytes = h.state().session.get(id).unwrap().bytes.clone();
    assert!(log.lock().unwrap().is_empty());
    assert!(h.state_mut().execute("a11y.read_aloud"));
    h.run_steps(4);
    h.get_by_label("Voice");
    h.get_by_label("Speed");
    if let Some(path) = shot {
        h.render().unwrap().save(path).unwrap();
    }
    h.get_by_role_and_label(egui::accesskit::Role::Button, "Read out loud").click();
    wait(&mut h, State::Speaking);
    assert!(log.lock().unwrap().contains(&"Hello".into()));
    h.get_by_label("Pause").click();
    wait(&mut h, State::Paused);
    h.get_by_label("Resume").click();
    wait(&mut h, State::Speaking);
    assert_eq!(log.lock().unwrap().iter().filter(|s| s.as_str() == "Hello").count(), 1);
    assert!(Arc::ptr_eq(&bytes, &h.state().session.get(id).unwrap().bytes));
    assert!(!h.state().session.get(id).unwrap().dirty);
    h.get_by_label("Stop").click();
    wait(&mut h, State::Idle);
    h.state_mut().start_read_aloud(1, 1).unwrap();
    wait(&mut h, State::Speaking);
    h.state_mut().close_tab(0);
    wait(&mut h, State::Idle);
}

/// Reject invalid 1-based page ranges before the injected backend is called.
#[test]
fn read_aloud_bad_ranges_do_not_start_audio() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut app = PdfCraftApp::new();
    app.open_bytes("voice.pdf", None, PDF.to_vec()).unwrap();
    app.read_aloud.player = Player::with_backend(Box::new(Fake(log.clone())));
    for (first, last) in [(0, 1), (2, 1), (1, 2), (1, usize::MAX)] {
        assert!(app.start_read_aloud(first, last).is_err());
    }
    assert!(log.lock().unwrap().is_empty());
}

/// Source switches and edits cancel speech through app logic, including while its dialog is hidden.
#[test]
fn read_aloud_source_switch_and_edit_stop_hidden_playback() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let backend = log.clone();
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.open_bytes("first.pdf", None, PDF.to_vec()).unwrap();
        app.read_aloud.player = Player::with_backend(Box::new(Fake(backend.clone())));
        app
    });
    h.state_mut().start_read_aloud(1, 1).unwrap();
    wait(&mut h, State::Speaking);
    assert!(!h.state().read_aloud.open);
    h.state_mut().open_bytes("second.pdf", None, PDF.to_vec()).unwrap();
    wait(&mut h, State::Idle);
    h.state_mut().start_read_aloud(1, 1).unwrap();
    wait(&mut h, State::Speaking);
    let id = h.state().active_ids().unwrap().1;
    h.state_mut().session.apply(id, pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }).unwrap();
    wait(&mut h, State::Idle);
    assert_eq!(log.lock().unwrap().iter().filter(|s| s.as_str() == "Hello").count(), 2);
}
