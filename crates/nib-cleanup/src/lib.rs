//! Cleanup modes, per-target mode selection, and the free tier's deterministic cleanup: the
//! personal [`Dictionary`] (applied in ALL modes) and the rule-based [`auto_tidy`]. The Pro
//! tier's LLM cleanup lives in the Python sidecar; this crate owns everything deterministic,
//! plus the rule that terminals/IDEs force Raw so a command is never rewritten.
#![forbid(unsafe_code)]

mod dictionary;
mod tidy;

pub use dictionary::Dictionary;
pub use tidy::auto_tidy;

use nib_platform::TargetProfile;
use nib_target::is_literal_exe;

/// A cleanup mode. `Raw` bypasses the LLM entirely; the others map to sidecar cleanup passes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Raw,
    Auto,
    Polish,
    Email,
}

impl Mode {
    /// Lowercase protocol token sent to the ASR/cleanup sidecar (raw/auto/polish/email).
    pub fn token(self) -> &'static str {
        match self {
            Mode::Raw => "raw",
            Mode::Auto => "auto",
            Mode::Polish => "polish",
            Mode::Email => "email",
        }
    }

    /// Uppercase short label shown in the overlay HUD.
    pub fn short_name(self) -> &'static str {
        match self {
            Mode::Raw => "RAW",
            Mode::Auto => "AUTO",
            Mode::Polish => "POLISH",
            Mode::Email => "EMAIL",
        }
    }

    /// Stable 0-based index (Raw, Auto, Polish, Email).
    pub fn index(self) -> u8 {
        match self {
            Mode::Raw => 0,
            Mode::Auto => 1,
            Mode::Polish => 2,
            Mode::Email => 3,
        }
    }

    /// Inverse of [`Mode::index`]; out-of-range values fall back to `Polish`.
    pub fn from_index(i: u8) -> Mode {
        match i {
            0 => Mode::Raw,
            1 => Mode::Auto,
            3 => Mode::Email,
            _ => Mode::Polish,
        }
    }

    /// Next mode in the cycle (wraps Email → Raw). Drives the mode-cycle hotkey.
    pub fn next(self) -> Mode {
        Mode::from_index((self.index() + 1) % 4)
    }

    /// Whether this mode needs the LLM sidecar (Pro). Raw/Auto are fully deterministic.
    pub fn needs_llm(self) -> bool {
        matches!(self, Mode::Polish | Mode::Email)
    }

    /// Next mode restricted to what the active sidecar supports: with `llm` the full cycle,
    /// without it Raw ↔ Auto only (the free tier's deterministic modes).
    pub fn next_available(self, llm: bool) -> Mode {
        if llm {
            self.next()
        } else if self == Mode::Raw {
            Mode::Auto
        } else {
            Mode::Raw
        }
    }

    /// Clamp a requested mode to what the active sidecar supports (Pro modes fall back to Auto).
    pub fn clamp_available(self, llm: bool) -> Mode {
        if !llm && self.needs_llm() {
            Mode::Auto
        } else {
            self
        }
    }

    /// Parse a CLI/token value like `"polish"`. Anything unknown defaults to `Polish`.
    pub fn parse(s: &str) -> Mode {
        match s.trim().to_ascii_lowercase().as_str() {
            "raw" => Mode::Raw,
            "auto" | "tidy" => Mode::Auto,
            "email" => Mode::Email,
            _ => Mode::Polish,
        }
    }
}

/// The mode actually applied to a target: terminals/IDEs force `Raw` so a command or identifier
/// is never rewritten, regardless of the selected mode.
pub fn effective_mode(selected: Mode, target: &TargetProfile) -> Mode {
    if target.is_terminal || is_literal_exe(&target.exe) {
        Mode::Raw
    } else {
        selected
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_round_trips() {
        for m in [Mode::Raw, Mode::Auto, Mode::Polish, Mode::Email] {
            assert_eq!(Mode::from_index(m.index()), m);
        }
    }

    #[test]
    fn cycle_wraps() {
        assert_eq!(Mode::Raw.next(), Mode::Auto);
        assert_eq!(Mode::Email.next(), Mode::Raw);
    }

    #[test]
    fn parse_defaults_to_polish() {
        assert_eq!(Mode::parse("raw"), Mode::Raw);
        assert_eq!(Mode::parse("EMAIL"), Mode::Email);
        assert_eq!(Mode::parse("nonsense"), Mode::Polish);
    }

    #[test]
    fn terminal_forces_raw() {
        let term = TargetProfile {
            exe: "cmd.exe".into(),
            is_terminal: true,
            ..Default::default()
        };
        assert_eq!(effective_mode(Mode::Polish, &term), Mode::Raw);

        let editor = TargetProfile {
            exe: "notepad.exe".into(),
            ..Default::default()
        };
        assert_eq!(effective_mode(Mode::Polish, &editor), Mode::Polish);
    }
}
