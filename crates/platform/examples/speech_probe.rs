//! Explicit native smoke check. Does not speak unless invoked with --speak.
//! `--language en` (default for speech) or `--language it` chooses a matching installed voice.
use pdfcraft_platform::speech::{Chunk, Options, Player, State};
use std::time::{Duration, Instant};

/// List local voices; only `--speak` plays a synthetic English/Italian control probe.
/// Bound discovery and playback waits, and reject unsupported command-line options.
fn main() -> Result<(), String> {
    let mut speak = false;
    let mut language = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--speak" => speak = true,
            "--language" => {
                let code = args.next().ok_or("--language needs en or it")?;
                if !matches!(code.as_str(), "en" | "it") {
                    return Err("the smoke check supports --language en or it".into());
                }
                language = Some(code);
            }
            _ => return Err("usage: speech_probe [--language en|it] [--speak]".into()),
        }
    }
    let mut player = Player::new();
    player.refresh_voices()?;
    let deadline = Instant::now() + Duration::from_secs(10);
    let voices = loop {
        let status = player.status();
        if let Some(e) = status.error {
            return Err(e);
        }
        if !status.voices.is_empty() {
            for voice in &status.voices {
                println!("{}: {} ({})", voice.id, voice.name, voice.language);
            }
            break status.voices;
        }
        if Instant::now() >= deadline {
            return Err("no installed voice was returned within 10 seconds".into());
        }
        std::thread::sleep(Duration::from_millis(30));
    };
    if !speak {
        return Ok(());
    }
    let language = language.as_deref().unwrap_or("en");
    let voice = voices
        .iter()
        .find(|v| v.language.split(['-', '_']).next().is_some_and(|l| l.eq_ignore_ascii_case(language)))
        .ok_or_else(|| format!("no installed {language} voice: install one before running this smoke check"))?;
    let text = if language == "it" {
        "Questa è una prova sintetica di lettura con PdfCraft. Il lettore può mettere in pausa, riprendere e interrompere senza modificare il documento."
    } else {
        "This is a synthetic PdfCraft read aloud test. The player can pause, resume, and stop without changing the document."
    };
    player.start(vec![Chunk { page: 1, text: text.into() }], Options { voice: Some(voice.id.clone()), ..Options::default() })?;
    let started = Instant::now();
    let mut paused = false;
    let mut resumed = false;
    loop {
        let status = player.status();
        if let Some(e) = status.error {
            return Err(e);
        }
        if !paused && started.elapsed() > Duration::from_secs(2) {
            player.pause()?;
            paused = true;
            println!("pause requested");
        }
        if paused && !resumed && started.elapsed() > Duration::from_secs(4) {
            player.resume()?;
            resumed = true;
            println!("resume requested");
        }
        if status.state == State::Completed {
            println!("completed");
            break;
        }
        if started.elapsed() > Duration::from_secs(30) {
            player.stop()?;
            return Err("speech did not finish within 30 seconds".into());
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    Ok(())
}
