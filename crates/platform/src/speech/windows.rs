//! Safe WinRT synthesis into an in-memory stream; no SSML, shell or network input.
use super::{Backend, Options, Voice};
use std::sync::{Arc, Mutex};
use windows::{
    Foundation::TypedEventHandler,
    Media::{
        Core::MediaSource,
        Playback::{IMediaPlaybackSource, MediaPlayer},
        SpeechSynthesis::{SpeechSynthesisStream, SpeechSynthesizer},
    },
    core::HSTRING,
};
use windows_future::{AsyncStatus, IAsyncOperation};

/// Format a WinRT failure for the speech worker without including document text.
fn error(e: windows::core::Error) -> String {
    format!("Windows speech: {e}")
}

pub(super) struct WindowsSpeech {
    synthesizer: SpeechSynthesizer,
    player: MediaPlayer,
    synthesis: Option<IAsyncOperation<SpeechSynthesisStream>>,
    stream: Option<SpeechSynthesisStream>,
    ended: Arc<Mutex<bool>>,
    failure: Arc<Mutex<Option<String>>>,
    paused: bool,
}

impl WindowsSpeech {
    /// Create worker-local WinRT synthesis/playback handles and isolated completion/error callbacks.
    /// Disable autoplay and global media-key handling; no speech is started by construction.
    pub fn new() -> Result<Self, String> {
        let synthesizer = SpeechSynthesizer::new().map_err(error)?;
        let player = MediaPlayer::new().map_err(error)?;
        player.SetAutoPlay(false).map_err(error)?;
        // Disable global media-key interception; the user operates PdfCraft's controls.
        player.CommandManager().and_then(|m| m.SetIsEnabled(false)).map_err(error)?;
        let ended = Arc::new(Mutex::new(false));
        let end = ended.clone();
        player
            .MediaEnded(&TypedEventHandler::new(move |_, _| {
                if let Ok(mut done) = end.lock() {
                    *done = true;
                }
                Ok(())
            }))
            .map_err(error)?;
        let failure = Arc::new(Mutex::new(None));
        let fail = failure.clone();
        player
            .MediaFailed(&TypedEventHandler::<MediaPlayer, windows::Media::Playback::MediaPlayerFailedEventArgs>::new(move |_, args| {
                if let Ok(mut message) = fail.lock() {
                    *message = Some(
                        args.as_ref()
                            .and_then(|a| a.ErrorMessage().ok())
                            .map_or_else(|| "Windows audio playback failed".into(), |s| s.to_string_lossy()),
                    );
                }
                Ok(())
            }))
            .map_err(error)?;
        Ok(Self { synthesizer, player, synthesis: None, stream: None, ended, failure, paused: false })
    }
}

impl Backend for WindowsSpeech {
    /// Return at most 256 installed Windows voices with their identifiers, names and language tags.
    fn voices(&mut self) -> Result<Vec<Voice>, String> {
        let voices = SpeechSynthesizer::AllVoices().map_err(error)?;
        let n = voices.Size().map_err(error)?;
        if n > 256 {
            return Err("Windows reported too many speech voices".into());
        }
        (0..n)
            .map(|i| {
                let v = voices.GetAt(i).map_err(error)?;
                Ok(Voice {
                    id: v.Id().map_err(error)?.to_string_lossy(),
                    name: v.DisplayName().map_err(error)?.to_string_lossy(),
                    language: v.Language().map_err(error)?.to_string_lossy(),
                })
            })
            .collect()
    }

    /// Replace the current chunk, select an installed voice/rate and begin plain-text stream synthesis.
    /// Use a fresh player so callbacks from an earlier chunk cannot complete the new one.
    fn start(&mut self, text: &str, options: &Options) -> Result<(), String> {
        self.stop()?;
        // A fresh player isolates delayed Ended/Failed callbacks from the previous chunk.
        // The old player's callbacks retain their own flags and cannot finish this chunk.
        *self = Self::new()?;
        let voice = if let Some(id) = &options.voice {
            let voices = SpeechSynthesizer::AllVoices().map_err(error)?;
            let n = voices.Size().map_err(error)?.min(256);
            let mut found = None;
            for i in 0..n {
                let v = voices.GetAt(i).map_err(error)?;
                if v.Id().map_err(error)?.to_string_lossy() == *id {
                    found = Some(v);
                    break;
                }
            }
            found.ok_or("the selected voice is not installed")?
        } else {
            SpeechSynthesizer::DefaultVoice().map_err(error)?
        };
        self.synthesizer.SetVoice(&voice).map_err(error)?;
        self.synthesizer.Options().and_then(|o| o.SetSpeakingRate(options.rate)).map_err(error)?;
        self.synthesis = Some(self.synthesizer.SynthesizeTextToStreamAsync(&HSTRING::from(text)).map_err(error)?);
        Ok(())
    }

    /// Pause MediaPlayer and retain the pause flag while asynchronous synthesis completes.
    fn pause(&mut self) -> Result<(), String> {
        self.player.Pause().map_err(error)?;
        self.paused = true;
        Ok(())
    }
    /// Clear the pause flag and play the current stream if synthesis has already completed.
    fn resume(&mut self) -> Result<(), String> {
        self.paused = false;
        if self.stream.is_some() {
            self.player.Play().map_err(error)?;
        }
        Ok(())
    }
    /// Cancel synthesis, detach/close the audio stream and reset per-chunk playback flags.
    fn stop(&mut self) -> Result<(), String> {
        if let Some(op) = self.synthesis.take() {
            let _ = op.Cancel();
        }
        // Attempt every cleanup step even if Pause fails. The worker destroys this backend
        // when cleanup reports an error, and Drop also closes the player and synthesizer.
        let paused = self.player.Pause().map_err(error);
        let detached = self.player.SetSource(None::<&IMediaPlaybackSource>).map_err(error);
        let closed = self.stream.take().map_or(Ok(()), |stream| stream.Close().map_err(error));
        if let Ok(mut done) = self.ended.lock() {
            *done = false;
        }
        if let Ok(mut failure) = self.failure.lock() {
            *failure = None;
        }
        self.paused = false;
        paused.and(detached).and(closed)
    }

    /// Advance completed synthesis into playback and surface WinRT/audio failures to the worker.
    fn finished(&mut self) -> Result<bool, String> {
        if let Some(message) = self.failure.lock().map_err(|_| "Windows audio state is unavailable")?.take() {
            return Err(message);
        }
        if let Some(op) = &self.synthesis {
            match op.Status().map_err(error)? {
                AsyncStatus::Completed => {
                    let stream = op.GetResults().map_err(error)?;
                    let source = MediaSource::CreateFromStream(&stream, &stream.ContentType().map_err(error)?).map_err(error)?;
                    self.player.SetSource(&source).map_err(error)?;
                    self.stream = Some(stream);
                    self.synthesis = None;
                    if !self.paused {
                        self.player.Play().map_err(error)?;
                    }
                }
                AsyncStatus::Error => return Err(op.ErrorCode().map_err(error)?.message()),
                AsyncStatus::Canceled => return Err("speech synthesis was cancelled".into()),
                _ => {}
            }
        }
        self.ended.lock().map(|done| *done).map_err(|_| "Windows audio state is unavailable".into())
    }
}

impl Drop for WindowsSpeech {
    /// Stop playback and close the WinRT player and synthesizer handles.
    fn drop(&mut self) {
        let _ = self.stop();
        let _ = self.player.Close();
        let _ = self.synthesizer.Close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// Explicitly verify an installed English voice produces audio at all supported rate extremes.
    #[test]
    #[ignore = "explicit native English synthesis check; requires an installed English Windows voice, no audio device"]
    fn windows_speech_synthesizes_english_to_audio() {
        let synthesizer = SpeechSynthesizer::new().unwrap();
        let voices = SpeechSynthesizer::AllVoices().unwrap();
        let voice = (0..voices.Size().unwrap().min(256))
            .map(|i| voices.GetAt(i).unwrap())
            .find(|v| v.Language().unwrap().to_string_lossy().to_ascii_lowercase().starts_with("en-"))
            .expect("install an English Windows speech voice before running this explicit test");
        synthesizer.SetVoice(&voice).unwrap();
        for rate in [0.5, 1.0, 2.0] {
            synthesizer.Options().unwrap().SetSpeakingRate(rate).unwrap();
            let op = synthesizer.SynthesizeTextToStreamAsync(&HSTRING::from("This is an English PdfCraft speech test.")).unwrap();
            let deadline = Instant::now() + Duration::from_secs(15);
            while op.Status().unwrap() == AsyncStatus::Started {
                assert!(Instant::now() < deadline, "English synthesis timed out");
                std::thread::sleep(Duration::from_millis(30));
            }
            assert_eq!(op.Status().unwrap(), AsyncStatus::Completed);
            let stream = op.GetResults().unwrap();
            assert!(stream.Size().unwrap() > 44, "English synthesis must return audio data");
            stream.Close().unwrap();
        }
        synthesizer.Close().unwrap();
    }

    /// Explicitly verify muted Windows playback resumes at the same position and Stop releases its stream.
    #[test]
    #[ignore = "explicit native Windows audio check; requires installed voices and MediaPlayer"]
    fn windows_speech_pause_preserves_position_and_stop_releases_stream() {
        let mut b = WindowsSpeech::new().unwrap();
        let voices = b.voices().unwrap();
        assert!(!voices.is_empty());
        b.start(&"Questa è una prova sintetica di lettura. ".repeat(20), &Options { voice: Some(voices[0].id.clone()), rate: 0.9 }).unwrap();
        b.player.SetIsMuted(true).unwrap();
        let until = Instant::now() + Duration::from_secs(15);
        while b.player.PlaybackSession().unwrap().Position().unwrap().Duration < 2_000_000 {
            assert!(!b.finished().unwrap());
            assert!(Instant::now() < until, "audio did not start");
            std::thread::sleep(Duration::from_millis(30));
        }
        b.pause().unwrap();
        std::thread::sleep(Duration::from_millis(100));
        let paused = b.player.PlaybackSession().unwrap().Position().unwrap().Duration;
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(b.player.PlaybackSession().unwrap().Position().unwrap().Duration, paused, "pause must preserve the audio position");
        b.resume().unwrap();
        std::thread::sleep(Duration::from_millis(300));
        assert!(b.player.PlaybackSession().unwrap().Position().unwrap().Duration > paused);
        b.stop().unwrap();
        assert!(b.stream.is_none() && b.synthesis.is_none());
    }
}
