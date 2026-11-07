# Read out loud

Read out loud plays extracted PDF text through local speech voices on Windows, macOS and Linux.
Windows uses safe WinRT SpeechSynthesizer and MediaPlayer calls, keeping text/audio in memory.
macOS invokes `/usr/bin/say`; Linux invokes an installed `espeak-ng` executable. Both synthesize
WAV audio in an owner-private temporary directory, then play it through Rodio/CPAL. No speech
library with a copyleft licence is linked, and no voice/model is bundled. Text is supplied through
stdin, never shell or command-line text. Embedded `[[speech commands]]` are escaped. Temporary
text/audio is removed before playback, or on cancellation/error. As with other temporary files,
a forcibly terminated process can leave data for the OS to clean up; deletion is not secure erasure.
It does not upload a document, download a voice, run a shell, or change the PDF. Opening a document
or constructing a player never starts speech or opens an audio device.

Open **View → Read out loud**, or **Prepare for accessibility → Read out loud**. Choose the
current page or a page range, an installed voice (or the system default), and a speed from 0.5×
to 2×. Press Read out loud. Pause and Resume preserve the current audio position; Stop cancels
preparation and playback. Closing the player, closing or switching the source tab, or changing
the source PDF stops it. The controls do not create an Undo step or mark the document dirty. If native cancellation fails,
the speech worker exits and releases its audio handles; status reports the error.

Text preparation runs off the desktop UI thread. A request is limited to 64 pages, 1 MiB of
extracted text and 4,096 Unicode-safe chunks of at most 2,048 bytes each. Pages without text are
listed as skipped. An entirely textless range returns an error directing the user to OCR.
Preparation checks cancellation between pages; extraction already running on one page must
return before that worker exits. Playback commands have a bounded queue; Stop invalidates queued
starts even when the queue is full.

UI translations come from the shared `storytold/craft-i18n` catalogs, configured with
`CRAFT_I18N_DIR` at the revision in `craft-i18n.rev`. No local `.tsv` catalogs are maintained here.

## Platforms and languages

- **Windows:** install the desired local text-to-speech voices in Windows settings. English and
  Italian use the same implementation; select an installed voice with `en-*` or `it-*` language.
- **macOS:** uses the system `say` tool and its listed voices. Install the desired English/Italian
  voice in system accessibility settings if absent. Multiword names, including enhanced voices,
  remain intact. An unspecified voice uses the system default.
- **Linux:** install `espeak-ng` through the distribution's package manager (e.g. `sudo apt install
  espeak-ng` on Debian/Ubuntu). The engine must be on an absolute directory in PATH. Its normal
  voice set includes English regional voices and Italian; the voice menu exposes those actually
  reported by the installation. An unspecified voice uses eSpeak NG's default English voice.
  The build needs ALSA development files (`libasound2-dev` on Debian/Ubuntu); runtime playback
  needs a working default audio output. Missing engine or device produces an actionable error.

**Interface language and speech voice are independent.** An Italian interface can read English
with an English voice and vice versa. PdfCraft does not automatically detect a document's language:
choose a matching voice, especially when the system default differs from the text. This also
applies to every other installed language; English and Italian have explicit regression tests.
The Unix rate multiplier uses 175 words/minute as the 1× base; Windows uses the voice's normal
rate. All support 0.5×–2× without changing audio pitch.

Unix synthesis is polled and cancellable, limited to 60 seconds and 32 MiB per chunk. WAV audio
must be mono/stereo, 8–96 kHz and at most 10 minutes. Unix voice discovery is capped at 1,024 voices,
64 KiB and 5 seconds (a pending discovery can delay Stop by up to that timeout). Pause/resume
controls the audio sink, rather than suspending a synthesis process or restarting the sentence.

## CLI and MCP

The tools use 1-based pages. `read_aloud_prepare` accepts `{doc, pages?}` and returns the text
chunks and empty page numbers without initializing audio. Omitted pages mean page 1, not the
whole document. Pages must be unique and in document order. Assistive reading is allowed when
copying is prohibited, as specified by ISO 32000-2 §7.6.4.2; this does not change export/copy tools.

- `read_aloud_voices {}` requests voice discovery without speaking.
- `read_aloud_start {doc, pages?, voice?, rate?}` explicitly requests audible playback.
- `read_aloud_control {action: "pause" | "resume" | "stop"}` controls the session's player.
- `read_aloud_status {}` reports playback state, source page, chunk progress, voices and errors.
- `read_aloud_wait {seconds?}` keeps a CLI script alive until playback completes or fails,
  or a 1–60 second timeout (default 30). A persistent MCP session can wait again after a timeout.

Start and control requests are asynchronous. Check status for eventual success or failure.
The player stops when its session exits, so a CLI script must end with `read_aloud_wait` to allow
audio to finish. For example, with `sample.pdf` inside the chosen root:

```json
[
  {"tool":"doc_open","args":{"path":"sample.pdf"}},
  {"tool":"read_aloud_start","args":{"doc":1,"pages":[1],"rate":1.0}},
  {"tool":"read_aloud_wait","args":{"seconds":60}}
]
```

In a fresh CLI session the first opened document has id 1; otherwise use the returned document id.
The same tools are discoverable and callable in compact MCP mode. Speech never starts an MCP
server or adds a network transport.

## Current limits and verification

Playback requires local voices and working audio output on Windows, macOS or Linux.
FreeBSD and browser playback remain unsupported. Voice and speed choices last
for the current app session; pitch and persisted preferences are not implemented.
Reading order uses PdfCraft's extracted page text, rather than the PDF structure tree: columns,
tables, mathematical text and malformed text maps can read imperfectly. Scans require OCR first.
There is no sentence highlighting or automatic page navigation.

Synthetic tests inject audio backends, so automated tests never speak a document. The explicit
native Windows test below mutes its output and checks that pause freezes playback position,
resume advances it and stop releases the synthesis/stream:

```text
cargo test -p pdfcraft-platform windows_speech_pause_preserves_position_and_stop_releases_stream -- --ignored
```

`cargo run -p pdfcraft-platform --example speech_probe` lists local voices without initializing
audio output. Adding `-- --speak --language en` or `-- --speak --language it` explicitly runs an
audible synthetic smoke check with a matching installed voice, and fails if that language is
absent; it never substitutes an Italian voice for an English smoke check.

Unix synthesis can be checked without an audio device:

```text
cargo test -p pdfcraft-platform native_synthesizes_english_to_valid_audio -- --ignored
cargo test -p pdfcraft-platform native_synthesizes_italian_to_valid_audio -- --ignored
```

Each check generates and decodes non-silent native WAV audio at 0.5×, 1× and 2×. The explicit
`native_audio_pause_resume_and_stop_preserve_position` check requires an audio device and mutes
its sink before playback. Ordinary tests consume the real sink in memory, checking audio position
through pause/resume/Stop, and exercise English/Italian voice parsing, UTF-8 stdin, timeouts,
process failures and private-file cleanup without any voice engine or audio device.

Historical contributor-fork native checks passed on Windows/macOS/Linux at `ba1c212`
([run](https://github.com/robertobulltech/pdfcraft/actions/runs/38278639802)), including English
synthesis on all three and Italian synthesis on the Unix runners. The commands above remain
available for explicit native checks. This feature follows the shared CI policy and adds no
local CI workflow. Runner synthesis and silent sink tests do not
prove audible output or desktop integration on physical machines. Physical macOS playback remains
unverified. Linux desktop validation is recorded below; packaged and sandboxed builds still need
their own playback checks.

### Physical Linux validation (2026-10-13)

The PR source at `50cf2ce8cf6bf0a17311f40e012d8316473e10b5` was built and tested on CachyOS,
KDE Plasma/Wayland, Rust 1.99.0 and eSpeak NG 1.52.0, with PipeWire 1.6.9 routing playback to
USB Audio Speakers. Builds used `--locked`, `--profile low-disk` and `-j 2`.

- Platform tests: 22 passed, 4 opt-in tests ignored; strict all-target platform Clippy passed.
- All three Unix native speech checks passed: non-silent English/Italian WAV synthesis at
  0.5×, 1× and 2×, and muted playback on the actual audio device with position-preserving
  pause/resume and stream release on Stop.
- Both audible `speech_probe` checks completed. The listener confirmed English and Italian
  playback with correct pause/resume, but reported poor voice naturalness. This validates
  functionality, not voice quality.
- Native CLI scripts and persistent compact MCP sessions read synthetic PDFs in both languages,
  reached completion, and exercised pause/resume/Stop and document-close cancellation. Source
  PDF SHA-256 hashes stayed unchanged and closing required no discard of unsaved work.
- The native wgpu desktop window selected English under the Italian interface and Italian under
  the English interface. Pause/resume/Stop worked, switching or closing the source stopped
  playback, and neither document became dirty. All three `read_aloud` UI regressions passed.

Reproduce the device checks explicitly (the probes speak aloud):

```text
cargo test -p pdfcraft-platform --profile low-disk -j 2 --locked native_ -- --ignored --nocapture --test-threads=1
cargo run -p pdfcraft-platform --example speech_probe --profile low-disk -j 2 --locked -- --speak --language en
cargo run -p pdfcraft-platform --example speech_probe --profile low-disk -j 2 --locked -- --speak --language it
```
