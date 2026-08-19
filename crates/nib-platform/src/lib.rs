//! `nib-platform` — the trait wall.
//!
//! Pure traits and POD types that isolate every OS-specific capability. `nib-win32`
//! (and later `nib-macos`) are the only implementers; `nib-pipeline` depends on these
//! traits and never on a platform crate. This is what makes the Mac port a swap rather
//! than a rewrite. See docs/design/01-core-app-design.md §11.
//!
//! The `cargo xtask check-layering` CI step fails the build if any crate other than
//! `nib-win32` / `*-sys` imports `windows::` — so this boundary is enforced, not aspirational.
#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::Duration;

/// A running count of input the **user** produced, shared from the platform's keyboard hook to
/// the pipeline.
///
/// Deliberately a plain atomic counter rather than a trait: it carries no OS concepts, so it can
/// live on this side of the wall and let `nib-pipeline` ask "did the user touch the keyboard since
/// I last typed?" without knowing that a keyboard hook exists.
///
/// Why the pipeline needs it: smart spacing remembers whether *our* last injection ended in
/// whitespace, but that memory describes the caret as we left it. If the user has typed, moved the
/// caret, or pressed space in between, the memory is stale and applying it produces a stray space.
#[derive(Debug, Default, Clone)]
pub struct InputWitness(Arc<AtomicU64>);

impl InputWitness {
    /// Record one piece of user-originated input.
    ///
    /// The platform hook must call this **only** for genuine keystrokes — text this app
    /// synthesises has to be filtered out first, or every injection would invalidate its own
    /// spacing memory and consecutive utterances would never join up.
    pub fn note(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }

    /// The current count. Only ever compared against an earlier reading for equality, so
    /// wrapping is irrelevant and `Relaxed` ordering is sufficient.
    pub fn count(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

/// Health of the global keyboard hook. Exists in the trait because macOS `CGEventTap`
/// gets OS-disabled exactly like Windows `LowLevelHooksTimeout` unhooks us — both need
/// the same watchdog shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookHealth {
    Healthy,
    Recovering,
    Lost,
}

/// A hotkey combination (modifier-only combos like Ctrl+Win are supported).
#[derive(Debug, Clone)]
pub struct Binding {
    pub keys: Vec<u16>,
    pub arming_ms: u16,
    pub suppress: bool,
}

#[derive(Debug, Clone)]
pub enum HotkeyEvent {
    Armed,
    PttDown {
        qpc: u64,
    },
    PttUp {
        qpc: u64,
    },
    Cancel,
    /// A configured secondary chord fired. The payload is the chord's index in the slice handed
    /// to [`HotkeySource::start`]; the source stays policy-free and the consumer decides what
    /// each index means. Indexed rather than named so adding a chord (theme cycle, quit) needs no
    /// new event variant and no change on this side of the wall.
    Chord(u8),
}

/// What a configured secondary chord means.
///
/// Lives here rather than in `nib-config` because both the config (which builds the chord list)
/// and the pipeline (which acts on it) need the vocabulary, and the pipeline deliberately does
/// not depend on the config crate. The hook itself stays policy-free — it only reports indices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChordAction {
    CycleMode,
    CycleStyle,
    Quit,
}

/// Semantic injection route — never a Win32 constant, so macOS maps the same enum.
/// (`ClipboardPasteAlt` = Shift+Insert on Windows, Cmd+Shift+V on macOS;
/// `UnicodeKeystroke` = `SendInput KEYEVENTF_UNICODE` / `CGEventKeyboardSetUnicodeString`.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InjectRoute {
    ClipboardPaste,
    ClipboardPasteAlt,
    UnicodeKeystroke,
    Refuse,
}

/// What the target focused control looks like — drives mode routing and password refusal.
#[derive(Debug, Clone, Default)]
pub struct TargetProfile {
    pub exe: String,
    pub control_type: Option<String>,
    pub class_name: Option<String>,
    pub is_password: bool,
    pub is_terminal: bool,
    pub is_remote_session: bool,
    /// The target runs at a higher integrity level than we do, so the OS (UIPI) will silently
    /// swallow injected input. Drives a "text kept" warning — never a silent drop.
    pub is_elevated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InjectOutcome {
    Inserted,
    Refused,
    FocusChanged,
    Blocked,
}

// NOTE: there is deliberately no `OverlaySurface` trait / `OverlayState` enum here. The overlay
// splits differently from the other capabilities: its pixels are platform-independent (the pure
// `nib-overlay` crate) and only the window surface is per-OS (`nib_win32::Win32Overlay`), which the
// composition root wires directly with shared atoms. A trait in between described a seam that
// nothing implemented, so it was removed rather than left as a fiction. Revisit if/when the overlay
// grows a state machine the pipeline must drive.

// --- The trait wall -------------------------------------------------------------------

pub trait TargetProbe {
    /// Snapshot the focused target within `budget`; degrade to exe-name-only on timeout.
    fn snapshot(&self, budget: Duration) -> TargetProfile;

    /// Prime the accessibility stack for the focused window, ahead of a real [`Self::snapshot`].
    ///
    /// Chromium (and other lazy providers) only build their accessibility tree once a client asks
    /// for it, so the *first* query after focus returns stale defaults — measured: `IsPassword`
    /// reads `false` for ~500 ms on a real `<input type=password>`, then flips to `true`. Injecting
    /// on that first answer could type dictation into a password field.
    ///
    /// Called on push-to-talk key-DOWN so the tree is live by the time the user stops speaking —
    /// the speech itself covers the activation delay, costing no latency. Must be non-blocking.
    /// Default: no-op, for platforms without a lazy provider.
    fn warm(&self) {}
}

pub trait TextInjector {
    /// Ordered route chain for this target (primary first).
    fn routes(&self, target: &TargetProfile) -> Vec<InjectRoute>;
    fn inject(&self, text: &str, route: InjectRoute, target: &TargetProfile) -> InjectOutcome;
}

pub trait Autostart {
    fn set(&self, on: bool) -> std::io::Result<()>;
    fn get(&self) -> bool;
}

pub trait PathLayout {
    fn config_dir(&self) -> PathBuf;
    fn data_dir(&self) -> PathBuf;
}

/// An utterance's frozen sample range, in absolute capture-stream sample indices. Frozen at the
/// PTT edges so a burst of rapid presses can't lose audio while a previous utterance is still
/// being transcribed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Utterance {
    pub start_abs: usize,
    pub end_abs: usize,
}

/// Microphone capture with a look-back ring. The `!Send` half (the OS stream) stays on the owning
/// thread behind this trait; `begin`/`end` are driven from the hotkey thread via a separate
/// `Send + Sync` control handle so the freeze happens at key-event time.
pub trait AudioCapture {
    /// Extract the utterance's samples as 16 kHz mono, releasing the ring's hold on them.
    fn take_utterance(&self, u: &Utterance) -> Vec<f32>;
    /// The device/mic display name (for the console banner / diagnostics).
    fn device_name(&self) -> String;
}

/// The global push-to-talk hotkey source. Implementations install an OS keyboard hook on
/// their own thread, stream events to `sink`, self-heal stuck modifiers, and report health.
/// macOS `CGEventTap` fits the same shape (see `HookHealth`).
pub trait HotkeySource {
    /// Begin streaming events. `ptt` is the push-to-talk combo; each entry in `chords` emits
    /// [`HotkeyEvent::Chord`] carrying its own index when fired. Runs until the implementation is
    /// dropped.
    fn start(&self, ptt: Binding, chords: Vec<Binding>, sink: Sender<HotkeyEvent>);
    fn health(&self) -> HookHealth;
}

/// Chooses the route chain from a profile — a pure function, unit-testable without any OS.
/// Mirrors the design's injection table (design §2 / the synthesis injection chain).
pub fn default_routes(target: &TargetProfile) -> Vec<InjectRoute> {
    if target.is_password {
        return vec![InjectRoute::Refuse];
    }
    if target.is_remote_session {
        // RDP/Citrix: SendInput Unicode is primary (clipboard redirection is often DLP-blocked).
        return vec![InjectRoute::UnicodeKeystroke, InjectRoute::ClipboardPaste];
    }
    if target.is_terminal {
        return vec![
            InjectRoute::ClipboardPaste,
            InjectRoute::ClipboardPasteAlt, // Shift+Insert
            InjectRoute::UnicodeKeystroke,
        ];
    }
    vec![InjectRoute::ClipboardPaste, InjectRoute::UnicodeKeystroke]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_field_refuses() {
        let p = TargetProfile {
            is_password: true,
            ..Default::default()
        };
        assert_eq!(default_routes(&p), vec![InjectRoute::Refuse]);
    }

    #[test]
    fn remote_session_prefers_unicode() {
        let p = TargetProfile {
            is_remote_session: true,
            ..Default::default()
        };
        assert_eq!(default_routes(&p)[0], InjectRoute::UnicodeKeystroke);
    }

    #[test]
    fn terminal_offers_shift_insert_fallback() {
        let p = TargetProfile {
            is_terminal: true,
            ..Default::default()
        };
        assert!(default_routes(&p).contains(&InjectRoute::ClipboardPasteAlt));
    }
}
