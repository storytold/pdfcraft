//! Local synthesis through macOS `say` or Linux `espeak-ng`, followed by safe WAV playback.
//! No shell, SSML, network, bundled voices, or linked speech-engine library. Text and WAVs
//! live only in an owner-private temporary directory, removed on completion, Stop or Drop.
#[cfg(any(target_os = "macos", target_os = "linux"))]
use super::Backend;
use super::{Options, Voice};
use rodio::{Decoder, Sink, Source};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use tempfile::TempDir;

const MAX_VOICE_BYTES: u64 = 65_536;
const MAX_WAV_BYTES: u64 = 32 * 1024 * 1024;
const MAX_VOICES: usize = 1024;
const SYNTHESIS_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug)]
enum Engine {
    #[cfg(any(target_os = "macos", test))]
    MacSay,
    #[cfg(any(target_os = "linux", test))]
    EspeakNg,
}

impl Engine {
    /// Parse bounded `say`/`espeak-ng` output into unique, valid installed voice records.
    fn voices(self, text: &str) -> Result<Vec<Voice>, String> {
        if text.len() as u64 > MAX_VOICE_BYTES {
            return Err("the speech engine returned too much voice data".into());
        }
        let mut voices: Vec<Voice> = Vec::new();
        for line in text.lines() {
            let voice = match self {
                #[cfg(any(target_os = "macos", test))]
                Self::MacSay => {
                    // Names can contain spaces (including newer '(Enhanced)' names). The
                    // locale, not the first whitespace, separates the name from the sample.
                    let Some((prefix, _)) = line.split_once('#') else { continue };
                    let mut words = prefix.split_whitespace().collect::<Vec<_>>();
                    let Some(language) = words.pop().filter(|tag| locale_tag(tag)) else { continue };
                    let name = words.join(" ");
                    Voice { id: name.clone(), name, language: language.replace('_', "-") }
                }
                #[cfg(any(target_os = "linux", test))]
                Self::EspeakNg => {
                    // Pty Language Age/Gender VoiceName File Other Languages.
                    let words = line.split_whitespace().collect::<Vec<_>>();
                    if words.first().and_then(|p| p.parse::<u16>().ok()).is_none() {
                        continue;
                    }
                    let (Some(language), Some(name), Some(id)) = (words.get(1), words.get(3), words.get(4)) else { continue };
                    if !locale_tag(language) {
                        continue;
                    }
                    Voice { id: (*id).into(), name: name.replace('_', " "), language: language.replace('_', "-") }
                }
            };
            if voice.id.is_empty() || voice.id.len() > 1024 || voice.id.chars().any(char::is_control) {
                continue;
            }
            if !voices.iter().any(|v| v.id == voice.id) {
                if voices.len() >= MAX_VOICES {
                    return Err("the speech engine reported too many voices".into());
                }
                voices.push(voice);
            }
        }
        if voices.is_empty() {
            return Err("no local speech voices were found; install a voice in macOS settings or install espeak-ng on Linux".into());
        }
        Ok(voices)
    }

    /// Build a voice-list command for the chosen engine without passing document text.
    fn query(self, program: &Path) -> Command {
        let mut command = Command::new(program);
        match self {
            #[cfg(any(target_os = "macos", test))]
            Self::MacSay => {
                command.args(["-v", "?"]);
            }
            #[cfg(any(target_os = "linux", test))]
            Self::EspeakNg => {
                command.arg("--voices");
            }
        }
        command
    }

    /// Build a WAV-synthesis command with stdin text, a single voice argument and the requested rate.
    fn synthesis(self, program: &Path, output: &Path, options: &Options) -> Result<Command, String> {
        options.validate()?;
        let mut command = Command::new(program);
        // Both commands use words/minute. Change the synthesis rate, not audio playback
        // speed, so changing rate doesn't change pitch. Linux defaults to English.
        let rate = (175.0 * options.rate).round().to_string();
        match self {
            #[cfg(any(target_os = "macos", test))]
            Self::MacSay => {
                command.args(["-f", "-", "--file-format=WAVE", "--data-format=LEI16", "-r", &rate, "-o"]).arg(output);
            }
            #[cfg(any(target_os = "linux", test))]
            Self::EspeakNg => {
                command.args(["--stdin", "-b", "1", "-s", &rate, "-w"]).arg(output);
            }
        }
        if let Some(id) = &options.voice {
            // The worker checks the id against the installed voice list. It is a single
            // argument, never command text; PDF text goes only through a private stdin file.
            command.arg("-v").arg(id);
        }
        Ok(command)
    }
}

/// Accept bounded ASCII language/region identifiers from native voice-list output.
fn locale_tag(tag: &str) -> bool {
    let primary = tag.split(['-', '_']).next().unwrap_or_default();
    (2..=3).contains(&primary.len())
        && primary.bytes().all(|b| b.is_ascii_alphabetic())
        && tag.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Create a new private speech file without replacing an existing path; use mode 0600 on Unix.
fn private_file(path: &Path) -> Result<File, String> {
    let mut options = OpenOptions::new();
    options.create_new(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path).map_err(|e| format!("could not create private speech data: {e}"))
}

struct Job {
    child: Option<Child>,
    directory: TempDir,
    output: PathBuf,
    started: Instant,
    timeout: Duration,
    limit: u64,
}

impl Job {
    /// Create the speech staging directory with owner-only Unix permissions from the outset.
    fn directory() -> Result<TempDir, String> {
        let mut builder = tempfile::Builder::new();
        builder.prefix("pdfcraft-speech-");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // Set 0700 at creation, rather than relying on tempfile's default or the
            // user's umask. A later chmod would expose a window for reading WAV data.
            builder.permissions(std::fs::Permissions::from_mode(0o700));
        }
        builder.tempdir().map_err(|e| format!("could not create private speech folder: {e}"))
    }

    /// Start a speech subprocess with suppressed diagnostics and retain its staging files until reaped.
    fn spawn(mut command: Command, directory: TempDir, output: PathBuf, limit: u64, timeout: Duration) -> Result<Self, String> {
        command.stderr(Stdio::null()); // Never echo PDF text in subprocess diagnostics.
        let child = command.spawn().map_err(|e| format!("could not start the local speech engine: {e}"))?;
        Ok(Self { child: Some(child), directory, output, started: Instant::now(), timeout, limit })
    }

    /// Capture bounded UTF-8 voice output in a private file, enforcing a five-second timeout.
    fn query(command: Command) -> Result<String, String> {
        let directory = Self::directory()?;
        let output = directory.path().join("voices.txt");
        let file = private_file(&output)?;
        let mut command = command;
        command.stdin(Stdio::null()).stdout(Stdio::from(file));
        let mut job = Self::spawn(command, directory, output, MAX_VOICE_BYTES, Duration::from_secs(5))?;
        while !job.poll()? {
            std::thread::sleep(Duration::from_millis(20));
        }
        String::from_utf8(job.bytes()?).map_err(|_| "local voice names are not valid UTF-8".into())
    }

    /// Stage 1–4096 bytes of UTF-8 text with escaped command delimiters and start WAV synthesis.
    /// Keep input/output private and attach the synthesis timeout and WAV-size budget to the job.
    fn synthesize(engine: Engine, program: &Path, text: &str, options: &Options) -> Result<Self, String> {
        if text.trim().is_empty() || text.len() > 4096 {
            return Err("local synthesis needs 1–4096 bytes of text".into());
        }
        let directory = Self::directory()?;
        let input = directory.path().join("text.txt");
        let output = directory.path().join("speech.wav");
        let mut file = private_file(&input)?;
        // Both engines have embedded [[speech commands]] even without SSML. Escape their
        // delimiters so document text cannot alter voice, rate, or insert long silences.
        let escaped = text.replace('[', "[ ").replace(']', " ]");
        file.write_all(escaped.as_bytes()).map_err(|e| format!("could not prepare local speech text: {e}"))?;
        drop(file);
        let stdin = File::open(&input).map_err(|e| format!("could not read local speech text: {e}"))?;
        let mut command = engine.synthesis(program, &output, options)?;
        command.stdin(Stdio::from(stdin)).stdout(Stdio::null());
        Self::spawn(command, directory, output, MAX_WAV_BYTES, SYNTHESIS_TIMEOUT)
    }

    /// Check job output size, child exit status and deadline; return true only after successful exit.
    fn poll(&mut self) -> Result<bool, String> {
        if std::fs::metadata(&self.output).is_ok_and(|m| m.len() > self.limit) {
            return Err("local speech output exceeded its size limit".into());
        }
        let Some(child) = self.child.as_mut() else { return Ok(true) };
        if let Some(status) = child.try_wait().map_err(|e| format!("could not check local speech synthesis: {e}"))? {
            self.child = None;
            if !status.success() {
                return Err("the local speech engine failed; check the selected voice and engine installation".into());
            }
            return Ok(true);
        }
        if self.started.elapsed() > self.timeout {
            return Err("the local speech engine timed out".into());
        }
        Ok(false)
    }

    /// Read the job output with its size budget, rejecting missing files and oversized output.
    fn bytes(&self) -> Result<Vec<u8>, String> {
        let mut bytes = Vec::new();
        File::open(&self.output)
            .map_err(|e| format!("local speech output is unavailable: {e}"))?
            .take(self.limit.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|e| format!("could not read local speech output: {e}"))?;
        if bytes.len() as u64 > self.limit {
            return Err("local speech output exceeded its size limit".into());
        }
        Ok(bytes)
    }
}

impl Drop for Job {
    /// Kill/reap any remaining child before the private staging directory is deleted.
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait(); // Reap before TempDir removes text and audio.
        }
        // Hold the directory until the child has exited, including cancellation/error paths.
        let _ = &self.directory;
    }
}

/// Validate bounded WAV data, channels, sample rate and duration before creating a playback source.
fn decode(bytes: Vec<u8>) -> Result<Decoder<std::io::Cursor<Vec<u8>>>, String> {
    if bytes.len() as u64 > MAX_WAV_BYTES {
        return Err("local speech output exceeded its size limit".into());
    }
    let decoder = Decoder::try_from(std::io::Cursor::new(bytes)).map_err(|e| format!("the speech engine returned invalid WAV audio: {e}"))?;
    if decoder.channels() == 0
        || decoder.channels() > 2
        || !(8_000..=96_000).contains(&decoder.sample_rate())
        || decoder.total_duration().is_none_or(|d| d.is_zero() || d > Duration::from_secs(600))
    {
        return Err("the speech engine returned unsupported audio (maximum 10 minutes, mono/stereo, 8–96 kHz)".into());
    }
    Ok(decoder)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub(super) struct UnixSpeech {
    engine: Engine,
    program: PathBuf,
    voices: Option<Vec<Voice>>,
    synthesis: Option<Job>,
    sink: Option<Sink>,
    stream: Option<rodio::OutputStream>,
    failure: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    paused: bool,
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
impl UnixSpeech {
    /// Locate the local synthesis executable without opening an audio device or starting a subprocess.
    pub fn new() -> Result<Self, String> {
        #[cfg(target_os = "macos")]
        let (engine, program) = (Engine::MacSay, PathBuf::from("/usr/bin/say"));
        #[cfg(target_os = "linux")]
        let (engine, program) = {
            let path = std::env::var_os("PATH").unwrap_or_default();
            let program = std::env::split_paths(&path)
                .filter(|p| p.is_absolute())
                .map(|p| p.join("espeak-ng"))
                .find(|p| p.is_file())
                .and_then(|p| p.canonicalize().ok())
                .ok_or("Read out loud on Linux requires espeak-ng; install it with your distribution's package manager")?;
            (Engine::EspeakNg, program)
        };
        Ok(Self { engine, program, voices: None, synthesis: None, sink: None, stream: None, failure: Default::default(), paused: false })
    }

    /// Lazily open the default output device and attach decoded audio, respecting a pending pause.
    fn play(&mut self, audio: Decoder<std::io::Cursor<Vec<u8>>>) -> Result<(), String> {
        if self.stream.is_none() {
            let failure = self.failure.clone();
            let mut stream = rodio::OutputStreamBuilder::from_default_device()
                .map_err(|e| format!("no default audio output is available: {e}"))?
                .with_error_callback(move |e| {
                    if let Ok(mut error) = failure.lock() {
                        *error = Some(format!("local audio playback failed: {e}"));
                    }
                })
                .open_stream()
                .map_err(|e| format!("could not open the default audio output: {e}"))?;
            stream.log_on_drop(false);
            self.stream = Some(stream);
        }
        let sink = Sink::connect_new(self.stream.as_ref().ok_or("local audio output is unavailable")?.mixer());
        if self.paused {
            sink.pause();
        }
        sink.append(audio);
        self.sink = Some(sink);
        Ok(())
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
impl Backend for UnixSpeech {
    /// Refresh installed local voices and cache the exact identifiers used to validate subsequent starts.
    fn voices(&mut self) -> Result<Vec<Voice>, String> {
        // Reopening the dialog or starting a new request observes voices installed since
        // the last request. Keep that exact list for start's id validation between chunks.
        self.voices = Some(self.engine.voices(&Job::query(self.engine.query(&self.program))?)?);
        Ok(self.voices.as_ref().ok_or("local voices are unavailable")?.clone())
    }
    /// Validate the cached voice selection, stop the previous chunk and begin cancellable local synthesis.
    fn start(&mut self, text: &str, options: &Options) -> Result<(), String> {
        options.validate()?;
        if options.voice.as_ref().is_some_and(|id| self.voices.as_ref().is_none_or(|v| !v.iter().any(|v| &v.id == id))) {
            return Err("the selected voice is not installed".into());
        }
        self.stop()?;
        self.synthesis = Some(Job::synthesize(self.engine, &self.program, text, options)?);
        Ok(())
    }
    /// Remember the paused state during synthesis and pause any existing playback sink.
    fn pause(&mut self) -> Result<(), String> {
        self.paused = true;
        if let Some(sink) = &self.sink {
            sink.pause();
        }
        Ok(())
    }
    /// Clear a pending pause and continue an existing sink from its current position.
    fn resume(&mut self) -> Result<(), String> {
        self.paused = false;
        if let Some(sink) = &self.sink {
            sink.play();
        }
        Ok(())
    }
    /// Cancel synthesis, stop the sink, release the audio device and clear playback failures.
    fn stop(&mut self) -> Result<(), String> {
        self.synthesis = None;
        if let Some(sink) = self.sink.take() {
            sink.stop();
        }
        self.stream = None; // Release the audio device when finished, stopped or errored.
        self.paused = false;
        if let Ok(mut error) = self.failure.lock() {
            *error = None;
        }
        Ok(())
    }
    /// Poll synthesis and audio failures, delete staged data before playback and report sink completion.
    fn finished(&mut self) -> Result<bool, String> {
        if let Some(error) = self.failure.lock().map_err(|_| "local audio state is unavailable")?.take() {
            return Err(error);
        }
        if let Some(job) = &mut self.synthesis {
            if !job.poll()? {
                return Ok(false);
            }
            let bytes = job.bytes()?;
            self.synthesis = None; // Delete all temporary text/audio before playback.
            self.play(decode(bytes)?)?;
        }
        Ok(self.sink.as_ref().is_some_and(Sink::empty))
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
impl Drop for UnixSpeech {
    /// Stop synthesis/playback and release the local audio device when the backend is dropped.
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify voice parsing preserves names with spaces and English/Italian regional language tags.
    #[test]
    fn parses_english_and_italian_voices_with_spaces_and_regions() {
        let mac = Engine::MacSay.voices("Samantha           en_US    # Hello, my name is Samantha.\nDaniel (Enhanced) en_GB # Hello\nAlice it_IT # Ciao\nVoice Without Locale\n").unwrap();
        assert_eq!(
            mac.iter().map(|v| (v.id.as_str(), v.language.as_str())).collect::<Vec<_>>(),
            [("Samantha", "en-US"), ("Daniel (Enhanced)", "en-GB"), ("Alice", "it-IT")]
        );
        let linux = Engine::EspeakNg.voices("Pty Language Age/Gender VoiceName File Other Languages\n 5 en-gb M English_(Great_Britain) gmw/en (en 2)\n 2 en-us M English_(America) gmw/en-US (en 3)\n 5 it M Italian roa/it\n").unwrap();
        assert_eq!(
            linux.iter().map(|v| (v.id.as_str(), v.language.as_str())).collect::<Vec<_>>(),
            [("gmw/en", "en-gb"), ("gmw/en-US", "en-us"), ("roa/it", "it")]
        );
        assert_eq!(linux[1].name, "English (America)");
        assert!(Engine::MacSay.voices("invalid").is_err());
        assert!(Engine::EspeakNg.voices(&"x".repeat(MAX_VOICE_BYTES as usize + 1)).is_err());
    }

    /// Verify command construction keeps text off argv and forwards each voice/rate as intended.
    #[test]
    fn synthesis_uses_stdin_and_single_voice_arguments_for_both_languages() {
        for (engine, voice) in
            [(Engine::MacSay, "Daniel (Enhanced)"), (Engine::MacSay, "Alice"), (Engine::EspeakNg, "gmw/en-US"), (Engine::EspeakNg, "roa/it")]
        {
            let command =
                engine.synthesis(Path::new("engine"), Path::new("private/speech.wav"), &Options { voice: Some(voice.into()), rate: 1.5 }).unwrap();
            let args = command.get_args().map(|a| a.to_string_lossy().into_owned()).collect::<Vec<_>>();
            assert_eq!(args.last().unwrap(), voice);
            assert!(args.iter().any(|a| a == "263"));
            assert!(args.iter().any(|a| a == "private/speech.wav"));
            assert!(!args.iter().any(|a| a == "-m" || a == "-n"));
            assert!(!engine.query(Path::new("engine")).get_args().any(|a| a == "--stdin"));
            assert!(engine.synthesis(Path::new("engine"), Path::new("audio"), &Options { voice: None, rate: f64::NAN }).is_err());
            assert!(Job::synthesize(engine, Path::new("no-such-pdfcraft-test-engine"), "text", &Options::default()).is_err());
        }
    }

    /// Act as a child-process fixture for voice output, stdin capture, failures and deliberate hangs.
    // A subprocess of this test executable is the fixture: no shell, audio device or engine.
    #[test]
    fn process_fixture() {
        let Ok(action) = std::env::var("PDFCRAFT_TEST_SPEECH_PROCESS") else { return };
        match action.as_str() {
            "voices" => println!("Samantha en_US # Hello\nAlice it_IT # Ciao"),
            "fail" => std::process::exit(7),
            "hang" => std::thread::sleep(Duration::from_secs(30)),
            "stdin" => {
                let mut text = String::new();
                std::io::stdin().read_to_string(&mut text).unwrap();
                assert_eq!(text, "Hello, world. Ciao, città. --output=/stranger $(command)");
                std::fs::write(std::env::var_os("PDFCRAFT_TEST_SPEECH_OUTPUT").unwrap(), wave()).unwrap();
            }
            _ => panic!("unknown fixture"),
        }
    }

    /// Build a command that re-enters only the process fixture with the requested test action.
    fn fixture(action: &str) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command.args(["--exact", "speech::unix::tests::process_fixture", "--nocapture"]).env("PDFCRAFT_TEST_SPEECH_PROCESS", action);
        command
    }

    /// Verify actual staged stdin retains Unicode text and bracket counts without speech-command delimiters.
    #[test]
    fn staged_speech_text_escapes_odd_and_even_bracket_runs() {
        // Inspect the actual synthesis stdin file. The test executable stands in for the
        // engine; it need not produce audio, and Drop reaps it and removes the private files.
        let program = std::env::current_exe().unwrap();
        for engine in [Engine::MacSay, Engine::EspeakNg] {
            for run in [3, 4, 1, 2, 5, 6, 31, 32] {
                let text = format!("Ciao, città. {}rate 50{} Hello.", "[".repeat(run), "]".repeat(run));
                let job = Job::synthesize(engine, &program, &text, &Options::default()).unwrap();
                let root = job.directory.path().to_owned();
                let staged = std::fs::read_to_string(root.join("text.txt")).unwrap();
                assert!(!staged.contains("[["), "opening delimiter survives a run of {run}: {staged:?}");
                assert!(!staged.contains("]]"), "closing delimiter survives a run of {run}: {staged:?}");
                assert_eq!(staged.chars().filter(|&ch| ch == '[').count(), run);
                assert_eq!(staged.chars().filter(|&ch| ch == ']').count(), run);
                assert!(staged.starts_with("Ciao, città. "));
                assert!(staged.contains("rate 50"));
                assert!(staged.ends_with(" Hello."));
                drop(job);
                assert!(!root.exists());
            }
        }
    }

    /// Verify child failures, hangs and oversized output return errors and clean up private files.
    #[test]
    fn subprocess_query_errors_timeout_and_drop_clean_private_data() {
        let voices = Engine::MacSay.voices(&Job::query(fixture("voices")).unwrap()).unwrap();
        assert_eq!(voices.len(), 2);
        assert!(Job::query(fixture("fail")).is_err());
        for (action, limit, timeout, oversized) in [("hang", 1024, Duration::from_millis(150), false), ("hang", 1, Duration::from_secs(3), true)] {
            let directory = Job::directory().unwrap();
            let root = directory.path().to_owned();
            let output = root.join("out");
            if oversized {
                std::fs::write(&output, "too large").unwrap();
            }
            let mut command = fixture(action);
            command.stdin(Stdio::null()).stdout(Stdio::null());
            let mut job = Job::spawn(command, directory, output, limit, timeout).unwrap();
            let until = Instant::now() + Duration::from_secs(3);
            loop {
                if job.poll().is_err() {
                    break;
                }
                assert!(Instant::now() < until);
                std::thread::sleep(Duration::from_millis(20));
            }
            drop(job);
            assert!(!root.exists(), "Stop/error must delete private text and audio");
        }
    }

    /// Generate contributor-original one-second mono PCM audio entirely in memory.
    fn wave() -> Vec<u8> {
        // Contributor-original synthetic 1 second mono PCM, generated only in memory.
        let data = 16_000u32;
        let mut bytes = Vec::new();
        bytes.extend(b"RIFF");
        bytes.extend((36 + data).to_le_bytes());
        bytes.extend(b"WAVEfmt ");
        bytes.extend(16u32.to_le_bytes());
        bytes.extend(1u16.to_le_bytes());
        bytes.extend(1u16.to_le_bytes());
        bytes.extend(8_000u32.to_le_bytes());
        bytes.extend(16_000u32.to_le_bytes());
        bytes.extend(2u16.to_le_bytes());
        bytes.extend(16u16.to_le_bytes());
        bytes.extend(b"data");
        bytes.extend(data.to_le_bytes());
        bytes.extend((0..8_000).flat_map(|n| if n % 2 == 0 { 1024i16 } else { -1024i16 }.to_le_bytes()));
        bytes
    }

    /// Verify private-file permissions, exact UTF-8 stdin, WAV decoding and staging cleanup.
    #[test]
    fn utf8_text_is_piped_privately_and_decoded_before_temp_files_are_removed() {
        let directory = Job::directory().unwrap();
        let root = directory.path().to_owned();
        let input = root.join("text.txt");
        let output = root.join("audio.wav");
        private_file(&input).unwrap().write_all("Hello, world. Ciao, città. --output=/stranger $(command)".as_bytes()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&root).unwrap().permissions().mode() & 0o777, 0o700);
            assert_eq!(std::fs::metadata(&input).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let mut command = fixture("stdin");
        command.env("PDFCRAFT_TEST_SPEECH_OUTPUT", &output).stdin(File::open(&input).unwrap()).stdout(Stdio::null());
        let mut job = Job::spawn(command, directory, output, MAX_WAV_BYTES, SYNTHESIS_TIMEOUT).unwrap();
        while !job.poll().unwrap() {
            std::thread::sleep(Duration::from_millis(20));
        }
        let audio = decode(job.bytes().unwrap()).unwrap();
        drop(job);
        assert!(!root.exists());
        assert_eq!(audio.total_duration(), Some(Duration::from_secs(1)));
        assert_eq!(audio.count(), 8_000);
        assert!(decode(b"invalid WAV".to_vec()).is_err());
    }

    /// Exercise Rodio position-preserving controls through an in-memory output iterator.
    #[test]
    fn real_sink_pauses_at_same_position_resumes_and_stops_without_audio_device() {
        let (sink, mut output) = Sink::new();
        sink.append(decode(wave()).unwrap());
        for _ in 0..2_000 {
            output.next();
        }
        sink.pause();
        // Rodio applies controls at periodic source boundaries. Drain only that transition.
        for _ in 0..500 {
            output.next();
        }
        let paused = sink.get_pos();
        for _ in 0..2_000 {
            assert_eq!(output.next(), Some(0.0));
        }
        assert_eq!(sink.get_pos(), paused);
        sink.play();
        for _ in 0..2_000 {
            output.next();
        }
        assert!(sink.get_pos() > paused);
        sink.stop();
        for _ in 0..500 {
            output.next();
        }
        assert!(sink.empty());
    }

    /// Explicitly verify installed English voices generate decodable audio at supported rates.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    #[ignore = "explicit native synthesis check; needs a local English voice, no audio device"]
    fn native_synthesizes_english_to_valid_audio() {
        native_synthesis("en", "This is an English PdfCraft speech test.");
    }

    /// Explicitly verify installed Italian voices generate decodable audio at supported rates.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    #[ignore = "explicit native synthesis check; needs a local Italian voice, no audio device"]
    fn native_synthesizes_italian_to_valid_audio() {
        native_synthesis("it", "Questa è una prova italiana di PdfCraft.");
    }

    /// Select an installed voice and validate non-silent WAV synthesis at 0.5×, 1× and 2× rates.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn native_synthesis(language: &str, text: &str) {
        let mut backend = UnixSpeech::new().unwrap();
        let voices = backend.voices().unwrap();
        let voice = voices
            .iter()
            .find(|v| v.language.split('-').next().is_some_and(|l| l.eq_ignore_ascii_case(language)))
            .unwrap_or_else(|| panic!("install a {language} voice before running this explicit test"));
        for rate in [0.5, 1.0, 2.0] {
            let mut job = Job::synthesize(backend.engine, &backend.program, text, &Options { voice: Some(voice.id.clone()), rate }).unwrap();
            while !job.poll().unwrap() {
                std::thread::sleep(Duration::from_millis(30));
            }
            let root = job.directory.path().to_owned();
            let mut decoder = decode(job.bytes().unwrap()).unwrap();
            assert!(decoder.total_duration().unwrap() > Duration::from_millis(100));
            assert!(decoder.any(|s| s.abs() > 0.001), "synthesis must produce non-silent audio");
            drop(job);
            assert!(!root.exists());
        }
    }

    /// Explicitly test muted device playback, pause/resume position and resource release on Stop.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    #[ignore = "explicit muted native playback check; requires a local voice and working audio device"]
    fn native_audio_pause_resume_and_stop_preserve_position() {
        let mut backend = UnixSpeech::new().unwrap();
        let voice = backend.voices().unwrap().into_iter().next().unwrap();
        backend.start(&"This is a synthetic speech playback test. ".repeat(15), &Options { voice: Some(voice.id), rate: 1.0 }).unwrap();
        backend.pause().unwrap(); // Pause before synthesis finishes: no audible startup.
        let until = Instant::now() + Duration::from_secs(20);
        while backend.sink.is_none() {
            assert!(!backend.finished().unwrap());
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(30));
        }
        backend.sink.as_ref().unwrap().set_volume(0.0);
        backend.resume().unwrap();
        std::thread::sleep(Duration::from_millis(300));
        assert!(backend.sink.as_ref().unwrap().get_pos() > Duration::ZERO);
        backend.pause().unwrap();
        std::thread::sleep(Duration::from_millis(100));
        let paused = backend.sink.as_ref().unwrap().get_pos();
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(backend.sink.as_ref().unwrap().get_pos(), paused);
        backend.resume().unwrap();
        std::thread::sleep(Duration::from_millis(300));
        assert!(backend.sink.as_ref().unwrap().get_pos() > paused);
        backend.stop().unwrap();
        assert!(backend.sink.is_none() && backend.stream.is_none() && backend.synthesis.is_none());
    }
}
