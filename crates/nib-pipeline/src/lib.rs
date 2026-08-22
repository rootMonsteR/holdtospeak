//! The dictation orchestrator. Owns the capture ring, the ASR/cleanup sidecar, and the current
//! mode; drives the loop: PttDown → begin capture; PttUp → take audio, transcribe (mode + target
//! aware), inject at the cursor. Platform capabilities arrive as trait objects (hook / injector /
//! target-probe) so the Mac port is a swap, not a rewrite.
#![forbid(unsafe_code)]

use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering::SeqCst};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use nib_asr::Sidecar;
use nib_audio::{write_wav_16k, CaptureControl};
use nib_cleanup::{effective_mode, Mode};
use nib_inject::inject_with_fallback;
use nib_platform::{
    AudioCapture, ChordAction, HotkeyEvent, InjectOutcome, InputWitness, TargetProbe,
    TargetProfile, TextInjector, Utterance,
};

pub mod stats;
pub use stats::{DictationRecord, Stats};

/// Messages the pipeline's run loop consumes: a completed (already-frozen) utterance plus control
/// commands from the hotkey / tray / console. Everything funnels through one channel so ordering
/// is deterministic.
pub enum PipeMsg {
    /// A released utterance whose sample range was frozen at key-up time (see `hotkey_forwarder`).
    Utterance(Utterance),
    /// Advance to the next cleanup mode (hotkey, tray click, or console `m`).
    CycleMode,
    SetMode(u8),
    SetStyle(u8),
    Learn(String),
    Quit,
}

/// Spawn the hotkey forwarder and return the `Sender<HotkeyEvent>` to hand to
/// `HotkeySource::start`.
///
/// The forwarder freezes the capture window **at key-event time** (`begin_utterance` on PttDown,
/// `end_utterance` on PttUp) using the `Send + Sync` [`CaptureControl`], and only forwards the
/// resulting [`Utterance`] to the pipeline for the (blocking) transcribe+inject. This is what keeps
/// back-to-back dictation from losing audio: the freeze can't be delayed behind a prior
/// transcription running on the pipeline thread. It also drives the `listening` atom (overlay
/// visibility) here, at event time.
pub fn hotkey_forwarder(
    pipe_tx: Sender<PipeMsg>,
    control: CaptureControl,
    listening: Arc<AtomicBool>,
    probe: Arc<dyn TargetProbe + Send + Sync>,
    chords: Vec<ChordAction>,
    style: Arc<AtomicU8>,
    style_count: u8,
) -> Sender<HotkeyEvent> {
    let (htx, hrx) = channel::<HotkeyEvent>();
    std::thread::spawn(move || {
        for ev in hrx {
            let msg = match ev {
                HotkeyEvent::PttDown { .. } => {
                    control.begin_utterance();
                    // Prime the accessibility tree while the user speaks, so the password check at
                    // inject time is truthful (a lazy provider's first answer is stale).
                    probe.warm();
                    listening.store(true, SeqCst); // overlay shows while held
                    print!("\r● listening...      ");
                    let _ = std::io::stdout().flush();
                    continue;
                }
                HotkeyEvent::PttUp { .. } => {
                    listening.store(false, SeqCst);
                    PipeMsg::Utterance(control.end_utterance())
                }
                HotkeyEvent::Cancel => {
                    // Abandon the hold: unfreeze the ring (no pending range) or the callback
                    // would never trim again. No message to the pipeline — nothing to transcribe.
                    control.cancel_utterance();
                    listening.store(false, SeqCst);
                    continue;
                }
                HotkeyEvent::Chord(i) => match chords.get(i as usize) {
                    Some(ChordAction::CycleMode) => PipeMsg::CycleMode,
                    // The overlay and the tray menu both poll this atom every frame, so writing it
                    // here IS the whole action — and the theme changing on screen is the feedback.
                    Some(ChordAction::CycleStyle) => {
                        let next = (style.load(SeqCst) + 1) % style_count.max(1);
                        style.store(next, SeqCst);
                        continue;
                    }
                    Some(ChordAction::Quit) => PipeMsg::Quit,
                    None => continue,
                },
                _ => continue,
            };
            if pipe_tx.send(msg).is_err() {
                break;
            }
        }
    });
    htx
}

/// Default RMS level below which an utterance is treated as silence.
///
/// Calibrated live against a quiet headset (Arctis Nova Pro): room tone + key-tap clatter measured
/// 0.0007–0.0024; the quietest real speech measured 0.0044 (typical 0.005–0.023). 0.003 sits in
/// the dead zone with margin both ways. Set LOW rather than "typical-mic" values (an earlier 0.006
/// gate ate all of this mic's speech) — a too-high gate is far worse than an occasional
/// hallucinated filler. Override with `NIB_SILENCE_RMS`; the console prints each utterance's
/// measured level so re-calibration is observable. Silero VAD replaces this heuristic later.
const SILENCE_RMS: f32 = 0.003;

/// Root-mean-square level of the captured samples.
fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

/// The silence threshold: `NIB_SILENCE_RMS` (debugging) wins, else `default` — the live gate the
/// settings window tunes, seeded from `settings.toml`.
fn silence_threshold_or(default: f32) -> f32 {
    std::env::var("NIB_SILENCE_RMS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

/// Whether a sidecar reply is a transcript worth injecting.
///
/// Two non-transcripts must never reach the user's document: an **empty** line (the sidecar's
/// no-speech reply) and an **`__error__ …`** line (a per-request failure — bad/short wav, decode
/// error). Injecting the latter would paste a Python exception into whatever is focused.
fn usable_transcript(reply: &str) -> Option<&str> {
    (!reply.is_empty() && !reply.starts_with("__error__")).then_some(reply)
}

/// Smart spacing between consecutive utterances (a session heuristic; the product does this
/// properly via UIA caret context).
///
/// `prev_ended_ws` is `None` before anything has been injected (no leading space on the first
/// utterance) and otherwise reports whether the previous injection ended in whitespace — keying
/// off the *new* text's leading char instead would double-space after "… ".
fn apply_spacing(text: &str, prev_ended_ws: Option<bool>) -> String {
    if prev_ended_ws == Some(false) && !text.starts_with(|c: char| c.is_whitespace()) {
        format!(" {text}")
    } else {
        text.to_string()
    }
}

/// Identity of an injection target, for deciding whether the smart-spacing memory still applies.
///
/// Deliberately app + control shaped rather than a window handle: re-focusing the same edit box is
/// the case we want to treat as continuous, and handles churn for reasons users don't perceive.
fn target_key(t: &TargetProfile) -> String {
    format!(
        "{}|{}|{}",
        t.exe,
        t.control_type.as_deref().unwrap_or(""),
        t.class_name.as_deref().unwrap_or("")
    )
}

/// Whether the previous injection's "did it end in whitespace" memory may still be trusted.
///
/// The memory records what *we* last typed, not what is actually in front of the caret. The moment
/// dictation moves to a different app or control, we no longer know what precedes the caret, and
/// guessing produced a stray leading space. Returning `None` means "don't add a separator", which
/// is the safe direction: a missing space is one keystroke to fix, a spurious one is invisible
/// until it isn't.
fn carry_spacing(
    prev_target: Option<&str>,
    cur_target: &str,
    prev_ended_ws: Option<bool>,
    prev_input: u64,
    cur_input: u64,
) -> Option<bool> {
    let same_place = prev_target == Some(cur_target);
    // The user typing, spacing or arrowing between utterances moves the caret somewhere we cannot
    // see, so the memory of what WE last typed no longer describes what precedes it.
    let untouched = prev_input == cur_input;
    if same_place && untouched {
        prev_ended_ws
    } else {
        None
    }
}

/// The dictation pipeline. The concrete capture is `!Send`, so this must be created and `run` on
/// one thread.
pub struct Pipeline {
    injector: Box<dyn TextInjector>,
    probe: Arc<dyn TargetProbe + Send + Sync>,
    capture: Box<dyn AudioCapture>,
    sidecar: Sidecar,
    mode: Mode,
    /// Shared with the tray + overlay so their menu check / HUD label track the mode live.
    shared_mode: Arc<AtomicU8>,
    /// Whether the previously injected text ended in whitespace. `None` = nothing injected yet
    /// (so the first utterance never gets a leading space). Drives the smart-spacing heuristic.
    prev_ended_ws: Option<bool>,
    /// Which target the previous injection went into. When focus moves elsewhere the spacing
    /// memory above describes a caret that is no longer there, so it is discarded rather than
    /// applied blind.
    prev_target: Option<String>,
    /// User keystrokes observed by the platform hook. Compared against `prev_input` to notice
    /// that the caret was moved by hand between utterances.
    user_input: InputWitness,
    /// The witness reading taken when we last injected.
    prev_input: u64,
    /// Recent dictations + counters, read by the settings window's Diagnostics page.
    stats: Arc<Stats>,
    /// The silence gate (f32 bits), shared so the settings window can tune it live. `NIB_SILENCE_RMS`
    /// still overrides it for debugging.
    silence_gate: Arc<std::sync::atomic::AtomicU32>,
}

impl Pipeline {
    /// Composition-root constructor: every collaborator is handed in once, by the only caller
    /// (`nib-core`'s `main`), which is why the argument count is tolerated over a builder.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        injector: Box<dyn TextInjector>,
        probe: Arc<dyn TargetProbe + Send + Sync>,
        capture: Box<dyn AudioCapture>,
        sidecar: Sidecar,
        mode: Mode,
        shared_mode: Arc<AtomicU8>,
        user_input: InputWitness,
        stats: Arc<Stats>,
        silence_gate: Arc<std::sync::atomic::AtomicU32>,
    ) -> Pipeline {
        shared_mode.store(mode.index(), SeqCst);
        Pipeline {
            injector,
            probe,
            capture,
            sidecar,
            mode,
            shared_mode,
            prev_ended_ws: None,
            prev_target: None,
            prev_input: user_input.count(),
            user_input,
            stats,
            silence_gate,
        }
    }

    /// Bits for a `silence_gate` atom: the pipeline's default threshold.
    pub fn default_silence_gate() -> u32 {
        SILENCE_RMS.to_bits()
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Change the active mode and publish it to the shared atom (tray/overlay), then announce.
    fn set_mode(&mut self, m: Mode) {
        self.mode = m;
        self.shared_mode.store(m.index(), SeqCst);
        self.announce_mode();
    }

    /// Consume messages until `Quit` (or the channel closes).
    pub fn run(&mut self, rx: Receiver<PipeMsg>) {
        for msg in rx {
            match msg {
                PipeMsg::Utterance(u) => self.on_utterance(u),
                PipeMsg::CycleMode => self.cycle_mode(),
                PipeMsg::SetMode(i) => {
                    let llm = self.sidecar.has_llm();
                    self.set_mode(Mode::from_index(i).clamp_available(llm));
                }
                // The tray writes the style atom directly (the overlay polls it every frame); this
                // path only exists so a future stateful overlay can react.
                PipeMsg::SetStyle(_) => {}
                PipeMsg::Learn(s) => self.on_learn(&s),
                PipeMsg::Quit => break,
            }
        }
    }

    /// Transcribe + inject a released utterance. The sample range was already frozen at key-up time
    /// by the forwarder; here we only read the ring (on this thread) and do the blocking work.
    fn on_utterance(&mut self, u: Utterance) {
        let samples = self.capture.take_utterance(&u);
        if samples.len() < 1600 {
            // < 0.1 s of audio — a stray tap, not speech.
            println!("\r(too short)              ");
            self.record("", self.mode, 0, 0, "too-short");
            return;
        }
        // Every capture includes the 400 ms pre-roll, so even a key-tap with no speech hands the
        // model ~400 ms of room tone — and ASR models reliably hallucinate filler ("Mm-hmm",
        // "Okay") from noise, which would then be injected into the user's document. Gate on
        // signal level so silence never reaches the model. (Silero VAD replaces this later.)
        let level = rms(&samples);
        let thresh = silence_threshold_or(f32::from_bits(self.silence_gate.load(SeqCst)));
        if level < thresh {
            println!("\r(silence — rms {level:.4} < gate {thresh:.4})          ");
            self.record("", self.mode, 0, 0, "silence");
            return;
        }
        let wav = std::env::temp_dir().join(format!("nib_utt_{}.wav", std::process::id()));
        if write_wav_16k(&wav, &samples).is_err() {
            eprintln!("  (failed to write temp wav)");
            return;
        }
        let mut target = self.probe.snapshot(Duration::from_millis(150));
        nib_target::classify(&mut target);
        let mode = effective_mode(self.mode, &target);

        let t0 = Instant::now();
        let result = self.sidecar.transcribe(mode.token(), &wav);
        // A restart during that call can change what the sidecar supports (e.g. the GGUF failed to
        // reload). Re-clamp so the tray/overlay never keep showing a mode that's now a no-op.
        let clamped = self.mode.clamp_available(self.sidecar.has_llm());
        if clamped != self.mode {
            println!("\r(sidecar lost LLM support — falling back)   ");
            self.set_mode(clamped);
        }
        let _ = std::fs::remove_file(&wav);
        let text = match result {
            Some(t) => match usable_transcript(&t) {
                Some(t) => t.to_string(),
                None if t.starts_with("__error__") => {
                    // A real sidecar failure — surface it, distinctly from silence, and keep the
                    // error text (it was previously swallowed as "(no speech)").
                    eprintln!(
                        "  sidecar error: {}",
                        t.trim_start_matches("__error__").trim()
                    );
                    println!("\r(transcription failed — see error above)   ");
                    self.record(
                        &target.exe,
                        mode,
                        0,
                        t0.elapsed().as_millis() as u64,
                        "failed",
                    );
                    return;
                }
                None => {
                    println!("\r(no speech)              ");
                    self.record(
                        &target.exe,
                        mode,
                        0,
                        t0.elapsed().as_millis() as u64,
                        "no-speech",
                    );
                    return;
                }
            },
            None => return, // sidecar died; it already logged
        };
        let ms = t0.elapsed().as_millis();
        let key = target_key(&target);
        let typed = self.user_input.count();
        self.prev_ended_ws = carry_spacing(
            self.prev_target.as_deref(),
            &key,
            self.prev_ended_ws,
            self.prev_input,
            typed,
        );
        let text = apply_spacing(&text, self.prev_ended_ws);

        let words = text.split_whitespace().count();
        match inject_with_fallback(self.injector.as_ref(), &text, &target) {
            InjectOutcome::Inserted => {
                self.prev_ended_ws = Some(text.ends_with(|c: char| c.is_whitespace()));
                self.prev_target = Some(key);
                self.prev_input = typed;
                let note = if target.is_terminal {
                    "  [→ Raw]"
                } else {
                    ""
                };
                println!("\r→ \"{}\"  [{ms} ms, rms {level:.4}]{note}", text.trim());
                self.record(&target.exe, mode, words, ms as u64, "inserted");
            }
            InjectOutcome::Refused => {
                println!("\r(password field — not inserted; text kept)   ");
                self.record(&target.exe, mode, words, ms as u64, "refused");
            }
            InjectOutcome::Blocked | InjectOutcome::FocusChanged => {
                self.record(&target.exe, mode, words, ms as u64, "blocked");
                // An elevated target is the common, explicable cause: UIPI swallows our input
                // silently, so say so and keep the text rather than leaving the user guessing.
                if target.is_elevated {
                    println!(
                        "\r(that window runs as administrator — text kept, not inserted: \"{}\")   ",
                        text.trim()
                    );
                } else {
                    println!("\r(could not insert \"{}\")   ", text.trim());
                }
            }
        }
    }

    /// Note a finished utterance for the Diagnostics page. Off the hot path (after injection).
    fn record(&self, app: &str, mode: Mode, words: usize, ms: u64, outcome: &'static str) {
        self.stats.push(DictationRecord {
            at_unix_ms: 0,
            app: app.to_string(),
            mode: mode.token().to_string(),
            words,
            ms,
            outcome,
        });
    }

    /// Cycle to the next mode the running sidecar can actually serve (free build: Raw ↔ Auto).
    fn cycle_mode(&mut self) {
        let llm = self.sidecar.has_llm();
        self.set_mode(self.mode.next_available(llm));
    }

    fn announce_mode(&self) {
        println!("\rmode: {}                       ", self.mode.short_name());
    }

    fn on_learn(&mut self, mapping: &str) {
        if let Some(reply) = self.sidecar.learn(mapping) {
            println!("learned: {reply}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nib_cleanup::Mode;
    use nib_platform::TargetProfile;

    #[test]
    fn error_replies_are_never_injected() {
        // The regression this guards: `__error__ <python exception>` used to slip through and get
        // pasted into the user's document.
        assert_eq!(usable_transcript("__error__ bad wav"), None);
        assert_eq!(usable_transcript(""), None);
        assert_eq!(usable_transcript("hello world"), Some("hello world"));
        // "(no speech)" is NOT a sidecar sentinel — if it ever appears it's real dictated text.
        assert_eq!(usable_transcript("(no speech)"), Some("(no speech)"));
    }

    #[test]
    fn silence_is_gated_but_speech_is_not() {
        // A quick PTT tap with no speech still captures the 400 ms pre-roll of room tone; the ASR
        // would hallucinate filler from it. Room tone must fall below the gate, speech above it.
        let thresh = SILENCE_RMS;
        let digital_silence = vec![0.0f32; 8000];
        assert!(rms(&digital_silence) < thresh);

        // Mic hiss / room tone: very low-amplitude noise.
        let room_tone: Vec<f32> = (0..8000)
            .map(|i| ((i * 7919 % 1000) as f32 / 1000.0 - 0.5) * 0.004)
            .collect();
        assert!(rms(&room_tone) < thresh, "room tone should be gated");

        // Speech-level signal: a 0.15-amplitude tone is far above conversational noise floors.
        let speech: Vec<f32> = (0..8000).map(|i| (i as f32 * 0.08).sin() * 0.15).collect();
        assert!(rms(&speech) > thresh, "speech must not be gated");
    }

    #[test]
    fn spacing_memory_is_dropped_when_focus_moves_or_user_types() {
        // Same target, keyboard untouched: the memory is what makes consecutive utterances join
        // up with a single space.
        assert_eq!(
            carry_spacing(Some("notepad|Edit|"), "notepad|Edit|", Some(false), 7, 7),
            Some(false)
        );
        // SAME target, but the user typed in between — they may well have typed the separator
        // themselves, which is exactly the reported double space. Add nothing.
        assert_eq!(
            carry_spacing(Some("notepad|Edit|"), "notepad|Edit|", Some(false), 7, 9),
            None
        );
        // Different app: we no longer know what sits before the caret, so add nothing. This is the
        // stray-leading-space case — dictating into a fresh window used to inherit the memory of
        // the previous window's last injection.
        assert_eq!(
            carry_spacing(Some("notepad|Edit|"), "code|Edit|", Some(false), 7, 7),
            None
        );
        // Different control in the SAME app (e.g. moving between fields) counts as a move too.
        assert_eq!(
            carry_spacing(Some("code|Edit|A"), "code|Edit|B", Some(false), 7, 7),
            None
        );
        // Nothing injected yet.
        assert_eq!(carry_spacing(None, "notepad|Edit|", None, 0, 0), None);
    }

    #[test]
    fn spacing_joins_utterances_without_doubling() {
        assert_eq!(apply_spacing("hello", None), "hello"); // first utterance: no leading space
        assert_eq!(apply_spacing("world", Some(false)), " world"); // previous ended mid-sentence
        assert_eq!(apply_spacing("world", Some(true)), "world"); // previous already ended in ws
        assert_eq!(apply_spacing(" world", Some(false)), " world"); // don't double up
    }

    /// A password field must refuse on every route — the whole point of detecting `IsPassword`.
    #[test]
    fn password_targets_only_ever_refuse() {
        use nib_platform::{default_routes, InjectRoute};
        let pw = TargetProfile {
            exe: "creds.exe".into(),
            is_password: true,
            // Even combined with flags that normally widen the chain.
            is_terminal: true,
            is_remote_session: true,
            ..Default::default()
        };
        assert_eq!(default_routes(&pw), vec![InjectRoute::Refuse]);
    }

    #[test]
    fn terminals_force_raw_regardless_of_mode() {
        let term = TargetProfile {
            exe: "windowsterminal.exe".into(),
            is_terminal: true,
            ..Default::default()
        };
        let editor = TargetProfile {
            exe: "notepad.exe".into(),
            ..Default::default()
        };
        for m in [Mode::Raw, Mode::Auto, Mode::Polish, Mode::Email] {
            assert_eq!(effective_mode(m, &term), Mode::Raw, "{m:?} in a terminal");
            assert_eq!(effective_mode(m, &editor), m, "{m:?} in a text field");
        }
    }
}
