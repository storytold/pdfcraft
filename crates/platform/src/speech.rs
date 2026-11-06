//! Explicit, local-only speech playback. Construction never initializes audio or starts speech.
//! Native synthesis and playback live on one cancellable worker, not the UI thread.

use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

#[cfg(any(target_os = "macos", target_os = "linux", test))]
mod unix;
#[cfg(windows)]
mod windows;

pub const MAX_TEXT_BYTES: usize = 1_048_576;
pub const MAX_CHUNKS: usize = 4096;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Voice {
    pub id: String,
    pub name: String,
    pub language: String,
}

#[derive(Clone, Debug)]
pub struct Chunk {
    /// The 1-based document page, preserved across skipped empty pages.
    pub page: usize,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct Options {
    pub voice: Option<String>,
    /// Speech rate multiplier. 1 is the voice's normal rate.
    pub rate: f64,
}

impl Default for Options {
    /// Use the platform default voice at its normal speaking rate.
    fn default() -> Self {
        Self { voice: None, rate: 1.0 }
    }
}

impl Options {
    /// Reject non-finite/out-of-range rates and empty or oversized voice identifiers.
    /// Voice installation is checked by the worker when playback starts.
    pub fn validate(&self) -> Result<(), String> {
        if !self.rate.is_finite() || !(0.5..=2.0).contains(&self.rate) {
            return Err("speech rate must be from 0.5 to 2.0".into());
        }
        if self.voice.as_ref().is_some_and(|v| v.is_empty() || v.len() > 1024) {
            return Err("voice must be a nonempty installed voice id (at most 1024 bytes)".into());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum State {
    #[default]
    Idle,
    Preparing,
    Speaking,
    Paused,
    Completed,
    Error,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Status {
    pub state: State,
    pub page: Option<usize>,
    pub chunk: usize,
    pub total_chunks: usize,
    pub voices: Vec<Voice>,
    pub error: Option<String>,
}

/// Backends are constructed and used on the speech worker. Native audio handles may be
/// thread-bound (CoreAudio); only injected backends need Send to reach that worker.
/// Implementations must return promptly so Stop/Drop can cancel preparation and playback.
pub trait Backend {
    /// List installed voice identifiers, display names and language tags without speaking.
    fn voices(&mut self) -> Result<Vec<Voice>, String>;
    /// Begin the supplied text chunk with the selected voice and synthesis rate.
    /// Return promptly so the worker can poll completion and process cancellation.
    fn start(&mut self, text: &str, options: &Options) -> Result<(), String>;
    /// Suspend the current chunk while preserving its playback position.
    fn pause(&mut self) -> Result<(), String>;
    /// Continue the paused chunk from its current playback position.
    fn resume(&mut self) -> Result<(), String>;
    /// Cancel pending synthesis/playback and release resources held by the current chunk.
    fn stop(&mut self) -> Result<(), String>;
    /// True when the current chunk has completely finished playing.
    fn finished(&mut self) -> Result<bool, String>;
}

enum Request {
    Voices,
    Start(Vec<Chunk>, Options, u64),
    Pause,
    Resume,
    Stop,
}

/// One player per frontend session. The bounded command queue prevents repeated callers from
/// accumulating unbounded PDF text. Dropping the player disconnects and stops its worker.
pub struct Player {
    sender: Option<mpsc::SyncSender<Request>>,
    status: Arc<Mutex<Status>>,
    backend: Option<Box<dyn Backend + Send>>,
    available: bool,
    generation: Arc<AtomicU64>,
}

impl Default for Player {
    /// Construct an idle player without initializing its worker or an audio device.
    fn default() -> Self {
        Self::new()
    }
}

impl Player {
    /// Construct an idle session player without starting a worker, discovering voices or opening audio.
    pub fn new() -> Self {
        Self {
            sender: None,
            status: Arc::new(Mutex::new(Status::default())),
            backend: None,
            available: Self::supported(),
            generation: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Inject a backend for deterministic tests and hosts with their own speech service.
    pub fn with_backend(backend: Box<dyn Backend + Send>) -> Self {
        Self { backend: Some(backend), available: true, ..Self::new() }
    }

    /// Report whether this build targets Windows, macOS or Linux; no device/voice probe is performed.
    pub fn supported() -> bool {
        cfg!(any(windows, target_os = "macos", target_os = "linux"))
    }

    /// Report whether native playback is supported or an injected backend was supplied.
    pub fn available(&self) -> bool {
        self.available
    }

    /// Clone the latest worker snapshot; report an error state if its mutex is poisoned.
    pub fn status(&self) -> Status {
        self.status.lock().map_or_else(
            |_| Status { state: State::Error, error: Some("speech worker state is unavailable".into()), ..Status::default() },
            |s| s.clone(),
        )
    }

    /// Lazily start the guarded worker and enqueue a request without blocking on a full queue.
    fn send(&mut self, request: Request) -> Result<(), String> {
        if !self.available {
            return Err("Read out loud requires Windows, macOS or Linux with local speech voices".into());
        }
        if self.sender.is_none() {
            let (tx, rx) = mpsc::sync_channel(4);
            let status = self.status.clone();
            let backend = self.backend.take();
            let generation = self.generation.clone();
            std::thread::Builder::new()
                .name("pdfcraft-speech".into())
                .spawn(move || {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(rx, &status, backend, &generation)));
                    if result.is_err() {
                        fail(&status, "the speech worker stopped unexpectedly".into());
                    }
                })
                .map_err(|e| format!("could not start the speech worker: {e}"))?;
            self.sender = Some(tx);
        }
        self.sender.as_ref().ok_or("speech worker is unavailable")?.try_send(request).map_err(|e| match e {
            mpsc::TrySendError::Full(_) => "speech is busy; retry after checking read_aloud_status".into(),
            mpsc::TrySendError::Disconnected(_) => "speech worker has stopped".into(),
        })
    }

    /// Queue installed-voice discovery; results and errors are published through `status`.
    pub fn refresh_voices(&mut self) -> Result<(), String> {
        self.send(Request::Voices)
    }

    /// Queue 1–4096 nonempty chunks, each at most 4096 bytes and at most 1 MiB in total.
    /// Reject invalid options/text before creating a worker; accepted playback is asynchronous.
    pub fn start(&mut self, chunks: Vec<Chunk>, options: Options) -> Result<(), String> {
        options.validate()?;
        let bytes = chunks.iter().try_fold(0usize, |n, c| n.checked_add(c.text.len())).ok_or("speech text is too large")?;
        if chunks.is_empty()
            || chunks.len() > MAX_CHUNKS
            || bytes > MAX_TEXT_BYTES
            || chunks.iter().any(|c| c.page == 0 || c.text.trim().is_empty() || c.text.len() > 4096)
        {
            return Err("speech needs 1–4096 nonempty chunks, at most 4096 bytes each and 1 MiB total".into());
        }
        let count = chunks.len();
        let shared = self.status.clone();
        let mut status = shared.lock().map_err(|_| "speech worker state is unavailable")?;
        self.send(Request::Start(chunks, options, self.generation.load(Ordering::Relaxed)))?;
        status.state = State::Preparing;
        status.total_chunks = count;
        status.error = None;
        Ok(())
    }

    /// Queue a pause request; poll status to observe the resulting playback state.
    pub fn pause(&mut self) -> Result<(), String> {
        self.send(Request::Pause)
    }
    /// Queue a resume request without restarting the current chunk.
    pub fn resume(&mut self) -> Result<(), String> {
        self.send(Request::Resume)
    }
    /// Invalidate queued starts and request cancellation, even when the command queue is full.
    /// Stopping an uninitialized or already disconnected player is a harmless no-op.
    pub fn stop(&mut self) -> Result<(), String> {
        // No audio has ever been initialized: stopping is a harmless no-op.
        if self.sender.is_none() {
            return Ok(());
        }
        // Cancellation bypasses a full queue and invalidates every already queued start.
        self.generation.fetch_add(1, Ordering::Relaxed);
        if let Some(sender) = &self.sender {
            match sender.try_send(Request::Stop) {
                Ok(()) | Err(mpsc::TrySendError::Full(_)) => Ok(()),
                Err(mpsc::TrySendError::Disconnected(_)) => {
                    // The worker has already dropped its backend and stopped audio. Never
                    // report a successful document edit as failed just because speech ended.
                    self.sender = None;
                    if let Ok(mut s) = self.status.lock() {
                        s.state = State::Idle;
                        s.page = None;
                        s.chunk = 0;
                        s.total_chunks = 0;
                    }
                    Ok(())
                }
            }
        } else {
            Ok(())
        }
    }
}

/// Publish a worker error when the shared status lock remains usable.
fn fail(status: &Mutex<Status>, message: String) {
    if let Ok(mut s) = status.lock() {
        s.state = State::Error;
        s.error = Some(message);
    }
}

/// Own the backend on one worker, sequence chunks and honor generation-based cancellation.
/// Publish asynchronous state/errors and stop the backend when the command channel disconnects.
fn run(rx: mpsc::Receiver<Request>, status: &Mutex<Status>, injected: Option<Box<dyn Backend + Send>>, generation: &AtomicU64) {
    let mut backend = match injected.map(|b| Ok(b as Box<dyn Backend>)).unwrap_or_else(native_backend) {
        Ok(b) => b,
        Err(e) => {
            fail(status, e);
            return;
        }
    };
    let mut chunks: Vec<Chunk> = Vec::new();
    let mut options = Options::default();
    let mut index: usize = 0;
    let mut state = State::Idle;
    let mut current_generation = generation.load(Ordering::Relaxed);
    loop {
        let request = if matches!(state, State::Preparing | State::Speaking) {
            match rx.recv_timeout(Duration::from_millis(30)) {
                Ok(r) => Some(r),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match rx.recv() {
                Ok(r) => Some(r),
                Err(_) => break,
            }
        };
        let result = (|| -> Result<(), String> {
            if current_generation != generation.load(Ordering::Relaxed) {
                backend.stop()?;
                chunks.clear();
                index = 0;
                state = State::Idle;
                current_generation = generation.load(Ordering::Relaxed);
            }
            match request {
                Some(Request::Voices) => {
                    let voices = backend.voices()?;
                    if let Ok(mut s) = status.lock() {
                        s.voices = voices;
                        s.error = None;
                    }
                }
                Some(Request::Start(new, opts, request_generation)) if request_generation == generation.load(Ordering::Relaxed) => {
                    // Validate the voice before replacing the current playback.
                    let voices = backend.voices()?;
                    if opts.voice.as_ref().is_some_and(|id| !voices.iter().any(|v| &v.id == id)) {
                        return Err("the selected voice is not installed".into());
                    }
                    backend.stop()?;
                    chunks = new;
                    options = opts;
                    index = 0;
                    backend.start(&chunks.first().ok_or("no speech text")?.text, &options)?;
                    state = State::Speaking;
                    if let Ok(mut s) = status.lock() {
                        s.voices = voices;
                        s.error = None;
                    }
                }
                Some(Request::Pause) if state == State::Speaking => {
                    backend.pause()?;
                    state = State::Paused;
                }
                Some(Request::Resume) if state == State::Paused => {
                    backend.resume()?;
                    state = State::Speaking;
                }
                Some(Request::Stop) => {
                    backend.stop()?;
                    chunks.clear();
                    index = 0;
                    state = State::Idle;
                }
                _ => {}
            }
            if state == State::Speaking && backend.finished()? {
                index = index.saturating_add(1);
                if let Some(chunk) = chunks.get(index) {
                    backend.start(&chunk.text, &options)?;
                } else {
                    backend.stop()?;
                    state = State::Completed;
                }
            }
            Ok(())
        })();
        let mut stop_failed = false;
        if let Err(e) = result {
            stop_failed = backend.stop().is_err();
            chunks.clear();
            index = 0;
            state = State::Error;
            fail(status, e);
        }
        if let Ok(mut s) = status.lock() {
            s.state = state;
            s.page = chunks.get(index).map(|c| c.page);
            s.chunk = index;
            s.total_chunks = chunks.len();
        }
        if stop_failed {
            // Native Drop closes the audio handles even if Stop could not establish silence.
            // Disconnect the request queue as well: no queued text may restart this backend.
            return;
        }
    }
    let _ = backend.stop();
}

/// Create the WinRT backend on the speech worker thread.
#[cfg(windows)]
fn native_backend() -> Result<Box<dyn Backend>, String> {
    Ok(Box::new(windows::WindowsSpeech::new()?))
}

/// Create the local macOS/Linux backend on the speech worker thread.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn native_backend() -> Result<Box<dyn Backend>, String> {
    Ok(Box::new(unix::UnixSpeech::new()?))
}

/// Return an explicit unsupported-platform error without initializing audio.
#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn native_backend() -> Result<Box<dyn Backend>, String> {
    Err("Read out loud requires Windows, macOS or Linux with local speech voices; FreeBSD and web playback are not supported yet".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::time::Instant;

    struct Fake {
        log: Arc<Mutex<Vec<String>>>,
        done: Arc<AtomicBool>,
        fail_start: bool,
    }
    impl Backend for Fake {
        /// Return the single synthetic voice used by worker-state tests.
        fn voices(&mut self) -> Result<Vec<Voice>, String> {
            Ok(vec![Voice { id: "fake".into(), name: "Test".into(), language: "en".into() }])
        }
        /// Record a synthetic chunk, or return the injected start failure without producing audio.
        fn start(&mut self, text: &str, _: &Options) -> Result<(), String> {
            if self.fail_start {
                return Err("synthetic audio failure".into());
            }
            self.log.lock().unwrap().push(text.into());
            Ok(())
        }
        /// Record a pause request in the fake backend log.
        fn pause(&mut self) -> Result<(), String> {
            self.log.lock().unwrap().push("pause".into());
            Ok(())
        }
        /// Record a resume request in the fake backend log.
        fn resume(&mut self) -> Result<(), String> {
            self.log.lock().unwrap().push("resume".into());
            Ok(())
        }
        /// Record a stop request in the fake backend log.
        fn stop(&mut self) -> Result<(), String> {
            self.log.lock().unwrap().push("stop".into());
            Ok(())
        }
        /// Consume the fake completion flag so each signal advances at most one chunk.
        fn finished(&mut self) -> Result<bool, String> {
            Ok(self.done.swap(false, Ordering::Relaxed))
        }
    }
    /// Wait at most three seconds for a worker-state transition in deterministic tests.
    fn wait(player: &Player, state: State) {
        let until = Instant::now() + Duration::from_secs(3);
        while player.status().state != state {
            assert!(Instant::now() < until, "expected {state:?}, got {:?}", player.status());
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    /// Build a player with shared fake playback logs, completion flags and optional start failure.
    fn setup(fail_start: bool) -> (Player, Arc<Mutex<Vec<String>>>, Arc<AtomicBool>) {
        let log = Arc::new(Mutex::new(Vec::new()));
        let done = Arc::new(AtomicBool::new(false));
        (Player::with_backend(Box::new(Fake { log: log.clone(), done: done.clone(), fail_start })), log, done)
    }
    /// Create synthetic text associated with a 1-based source page.
    fn chunk(page: usize, text: &str) -> Chunk {
        Chunk { page, text: text.into() }
    }

    /// Verify lazy initialization, voice discovery, chunk sequencing and position-preserving controls.
    #[test]
    fn speech_is_opt_in_and_pause_resume_preserves_the_current_chunk() {
        let (mut p, log, done) = setup(false);
        assert!(log.lock().unwrap().is_empty());
        p.stop().unwrap();
        assert!(log.lock().unwrap().is_empty());
        p.start(vec![chunk(2, "first"), chunk(4, "second")], Options::default()).unwrap();
        wait(&p, State::Speaking);
        assert_eq!(p.status().page, Some(2));
        p.pause().unwrap();
        wait(&p, State::Paused);
        done.store(true, Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(80));
        assert_eq!(p.status().page, Some(2));
        p.resume().unwrap();
        let until = Instant::now() + Duration::from_secs(3);
        while p.status().page != Some(4) {
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(log.lock().unwrap().iter().filter(|s| s.as_str() == "first").count(), 1, "resume must not repeat the current text");
        done.store(true, Ordering::Relaxed);
        wait(&p, State::Completed);
        assert_eq!(p.status().chunk, 2);
        p.start(vec![chunk(1, "again")], Options::default()).unwrap();
        wait(&p, State::Speaking);
        p.stop().unwrap();
        wait(&p, State::Idle);
    }

    /// Verify invalid rates, voices and text budgets fail before any backend call.
    #[test]
    fn bad_input_never_initializes_the_backend() {
        let (mut p, log, _) = setup(false);
        for options in
            [Options { rate: f64::NAN, voice: None }, Options { rate: 3.0, voice: None }, Options { rate: 1.0, voice: Some(String::new()) }]
        {
            assert!(p.start(vec![chunk(1, "text")], options).is_err());
        }
        for chunks in [
            vec![],
            vec![chunk(0, "text")],
            vec![chunk(1, " ")],
            vec![chunk(1, &"x".repeat(4097))],
            vec![chunk(1, "text"); MAX_CHUNKS + 1],
            vec![chunk(1, &"x".repeat(4096)); 257],
        ] {
            assert!(p.start(chunks, Options::default()).is_err());
        }
        assert!(log.lock().unwrap().is_empty());
    }

    /// Verify unknown voices and backend start failures reach status as errors without playback.
    #[test]
    fn missing_voice_and_audio_failure_are_reported_without_speech() {
        let (mut p, log, _) = setup(false);
        p.start(vec![chunk(1, "text")], Options { voice: Some("missing".into()), rate: 1.0 }).unwrap();
        wait(&p, State::Error);
        assert!(p.status().error.unwrap().contains("not installed"));
        assert!(!log.lock().unwrap().iter().any(|s| s == "text"));
        let (mut p, _, _) = setup(true);
        p.start(vec![chunk(1, "text")], Options::default()).unwrap();
        wait(&p, State::Error);
        assert_eq!(p.status().error.as_deref(), Some("synthetic audio failure"));
    }

    /// Verify dropping a player disconnects its worker and stops active or paused playback.
    #[test]
    fn drop_stops_active_or_paused_audio() {
        for paused in [false, true] {
            let (mut p, log, _) = setup(false);
            p.start(vec![chunk(1, "text")], Options::default()).unwrap();
            wait(&p, State::Speaking);
            if paused {
                p.pause().unwrap();
                wait(&p, State::Paused);
            }
            let before = log.lock().unwrap().iter().filter(|s| s.as_str() == "stop").count();
            drop(p);
            let until = Instant::now() + Duration::from_secs(3);
            while log.lock().unwrap().iter().filter(|s| s.as_str() == "stop").count() == before {
                assert!(Instant::now() < until);
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }

    /// Verify generation-based Stop cancels queued starts even when the request queue is saturated.
    #[test]
    fn stop_cancels_queued_starts_even_when_the_queue_is_full() {
        let (mut p, log, _) = setup(false);
        for i in 0..50 {
            let _ = p.start(vec![chunk(1, &format!("queued {i}"))], Options::default());
        }
        p.stop().unwrap();
        wait(&p, State::Idle);
        let before = log.lock().unwrap().len();
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(log.lock().unwrap().len(), before, "stale queued starts cannot restart stopped audio");
    }

    /// A failed native Stop must release the backend instead of stranding active audio.
    #[test]
    fn failed_stop_drops_the_backend_and_discards_queued_audio() {
        struct FailingStop {
            active: bool,
            dropped: Arc<AtomicBool>,
        }
        impl Backend for FailingStop {
            /// Return an empty synthetic voice list.
            fn voices(&mut self) -> Result<Vec<Voice>, String> {
                Ok(Vec::new())
            }
            /// Mark the synthetic device active without emitting audio.
            fn start(&mut self, _: &str, _: &Options) -> Result<(), String> {
                self.active = true;
                Ok(())
            }
            /// Accept an unused pause request.
            fn pause(&mut self) -> Result<(), String> {
                Ok(())
            }
            /// Accept an unused resume request.
            fn resume(&mut self) -> Result<(), String> {
                Ok(())
            }
            /// Fail every Stop while the synthetic device remains active.
            fn stop(&mut self) -> Result<(), String> {
                if self.active { Err("synthetic native Stop failure".into()) } else { Ok(()) }
            }
            /// Keep the current chunk active until the worker releases it.
            fn finished(&mut self) -> Result<bool, String> {
                Ok(false)
            }
        }
        impl Drop for FailingStop {
            /// Record destruction of the worker-owned audio handles.
            fn drop(&mut self) {
                self.dropped.store(true, Ordering::Relaxed);
            }
        }
        let dropped = Arc::new(AtomicBool::new(false));
        let mut player = Player::with_backend(Box::new(FailingStop { active: false, dropped: dropped.clone() }));
        player.start(vec![chunk(1, "current"), chunk(2, "must not start")], Options::default()).unwrap();
        wait(&player, State::Speaking);
        player.stop().unwrap();
        wait(&player, State::Error);
        let until = Instant::now() + Duration::from_secs(3);
        while !dropped.load(Ordering::Relaxed) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(dropped.load(Ordering::Relaxed), "failed cancellation must destroy the native audio handles");
        assert!(player.status().error.unwrap().contains("Stop failure"));
    }
}
