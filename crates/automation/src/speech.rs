//! Read-only assistive text preparation and explicit local speech playback.
use crate::{Args, Automation, Result, ToolError, failed};
use pdfcraft_engine::speech::Plan;
use pdfcraft_platform::speech::{Chunk, Options, Player};
use serde_json::{Value, json};
use std::sync::Arc;

impl Automation {
    /// Injectable audio avoids speaking private/test PDFs during automated verification.
    pub fn with_speech_player(mut self, player: Player) -> Self {
        self.speech = player;
        self
    }

    /// Stop playback when its source document closes or its byte snapshot changes.
    pub(crate) fn invalidate_speech(&mut self) -> Result<()> {
        if self.speech_source.as_ref().is_some_and(|(id, bytes)| self.session.get(*id).is_none_or(|d| !Arc::ptr_eq(bytes, &d.bytes))) {
            self.speech.stop().map_err(failed)?;
            self.speech_source = None;
        }
        Ok(())
    }

    /// Extract a bounded reading plan from requested 1-based pages, defaulting to page 1.
    fn speech_plan(&self, a: &Args) -> Result<Plan> {
        let doc = self.doc(a)?;
        let pages = if a.opt_ints("pages")?.is_some() { self.pages(a, "pages")? } else { vec![0] };
        doc.read_aloud_source(&pages).map_err(ToolError::InvalidArgs)?.extract().map_err(failed)
    }

    /// Return extracted chunks and skipped empty pages without starting audio or editing the PDF.
    pub(crate) fn read_aloud_prepare(&self, a: &Args) -> Result<Value> {
        let plan = self.speech_plan(a)?;
        Ok(
            json!({ "chunks": plan.chunks.iter().map(|c| json!({"page":c.page,"text":c.text})).collect::<Vec<_>>(), "empty_pages":plan.empty_pages, "reading_order":"extracted_page_text", "audio_started":false }),
        )
    }

    /// Validate voice/rate options, prepare text and queue local playback tied to the source snapshot.
    pub(crate) fn read_aloud_start(&mut self, a: &Args) -> Result<Value> {
        let options = Options { voice: a.opt_str("voice")?.map(str::to_owned), rate: a.opt_num("rate")?.unwrap_or(1.0) };
        options.validate().map_err(ToolError::InvalidArgs)?;
        let doc = self.doc(a)?;
        let source = (doc.id, doc.bytes.clone());
        let plan = self.speech_plan(a)?;
        let empty_pages = plan.empty_pages;
        let count = plan.chunks.len();
        self.speech.start(plan.chunks.into_iter().map(|c| Chunk { page: c.page, text: c.text }).collect(), options).map_err(failed)?;
        self.speech_source = Some(source);
        Ok(json!({"accepted":true,"chunks":count,"empty_pages":empty_pages,"status_tool":"read_aloud_status"}))
    }

    /// Queue pause/resume/stop; reject unknown actions and detach the source on Stop.
    pub(crate) fn read_aloud_control(&mut self, a: &Args) -> Result<Value> {
        match a.str("action")? {
            "pause" => self.speech.pause(),
            "resume" => self.speech.resume(),
            "stop" => {
                self.speech_source = None;
                self.speech.stop()
            }
            _ => return Err(ToolError::InvalidArgs("action must be pause, resume or stop".into())),
        }
        .map_err(failed)?;
        Ok(json!({"accepted":true,"status_tool":"read_aloud_status"}))
    }

    /// Report platform support, source document and the latest asynchronous playback state.
    pub(crate) fn read_aloud_status(&self) -> Result<Value> {
        Ok(json!({"supported":Player::supported(),"doc":self.speech_source.as_ref().map(|s|s.0.0),"playback":self.speech.status()}))
    }

    /// Queue local voice discovery without starting speech; results arrive through status polling.
    pub(crate) fn read_aloud_voices(&mut self) -> Result<Value> {
        self.speech.refresh_voices().map_err(failed)?;
        Ok(json!({"accepted":true,"supported":Player::supported(),"status_tool":"read_aloud_status"}))
    }

    /// Wait 1–60 seconds for completion/error; a timeout reports progress without stopping playback.
    pub(crate) fn read_aloud_wait(&self, a: &Args) -> Result<Value> {
        let seconds = a.opt_int("seconds")?.unwrap_or(30);
        let seconds = u64::try_from(seconds)
            .ok()
            .filter(|s| (1..=60).contains(s))
            .ok_or_else(|| ToolError::InvalidArgs("seconds must be from 1 to 60".into()))?;
        let until = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
        // A start request is asynchronous; don't mistake the initial idle state for completion.
        loop {
            let status = self.speech.status();
            if matches!(status.state, pdfcraft_platform::speech::State::Completed | pdfcraft_platform::speech::State::Error)
                || (self.speech_source.is_none() && status.state == pdfcraft_platform::speech::State::Idle)
            {
                return Ok(json!({"timed_out":false,"playback":status}));
            }
            if std::time::Instant::now() >= until {
                return Ok(json!({"timed_out":true,"playback":status}));
            }
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
    }
}
