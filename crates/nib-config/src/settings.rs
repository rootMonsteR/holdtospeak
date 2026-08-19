//! `settings.toml` — the user's preferences, in plain text they can read and edit.
//!
//! Being hand-editable is deliberate: a keyboard-hook-and-microphone app asks for a lot of trust,
//! and a config you can open in Notepad (rather than a registry blob or an opaque database) is
//! part of earning it. Unknown keys are ignored and malformed values fall back to the default, so
//! a typo degrades one setting instead of refusing to start.

use std::path::Path;

/// Everything the app reads at startup, with defaults that work on a fresh install.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// Cleanup mode to start in: `raw` or `auto` (the free tier's two modes).
    pub mode: String,
    /// Show the floating voice-spectrum overlay while push-to-talk is held.
    pub overlay: bool,
    /// Overlay theme: `hud`, `volt`, `wave`, `bars`.
    pub overlay_style: String,
    /// RMS below which an utterance is treated as silence and never sent to the recognizer.
    /// Raise it in a noisy room; lower it for a very quiet microphone.
    pub silence_rms: f32,
    /// Which sidecar to run: `native` (free, no Python) or `python` (dev/Pro, LLM cleanup).
    pub sidecar: String,
    /// Start automatically when you sign in to Windows.
    pub autostart: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            mode: "auto".into(),
            overlay: true,
            overlay_style: "hud".into(),
            // Matches nib-pipeline's SILENCE_RMS; calibrated against a quiet headset.
            silence_rms: 0.003,
            sidecar: "native".into(),
            autostart: false,
        }
    }
}

/// The commented template written on first run, so the file teaches its own options.
pub const TEMPLATE: &str = "\
# Nib settings. Plain text on purpose — edit freely, delete to restore defaults.
# Unknown keys are ignored; a bad value falls back to the default rather than failing to start.

# Cleanup mode at startup: raw | auto
mode = auto

# Floating voice-spectrum overlay, shown only while push-to-talk is held.
overlay = true
overlay_style = hud          # hud | volt | wave | bars

# Speech is only sent to the recognizer above this RMS level, so a stray keypress
# can't make the model hallucinate words. Raise in a noisy room, lower for a quiet mic.
silence_rms = 0.003

# native = the bundled Rust recognizer (no Python). python = dev/Pro sidecar with LLM cleanup.
sidecar = native

# Start Nib when you sign in to Windows.
autostart = false
";

/// Parse `settings.toml`. A missing file yields defaults (and the caller may write the template).
pub fn load(path: &Path) -> Settings {
    let mut s = Settings::default();
    let Ok(text) = std::fs::read_to_string(path) else {
        return s;
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        // Strip a trailing ` # comment` (only when preceded by whitespace, so a `#` inside a
        // value survives), then optional quotes.
        let v = match v.find(" #") {
            Some(i) => &v[..i],
            None => v,
        };
        let (k, v) = (k.trim().to_ascii_lowercase(), v.trim().trim_matches('"'));
        match k.as_str() {
            "mode" => s.mode = v.to_ascii_lowercase(),
            "overlay" => s.overlay = parse_bool(v, s.overlay),
            "overlay_style" => s.overlay_style = v.to_ascii_lowercase(),
            "silence_rms" => s.silence_rms = v.parse().unwrap_or(s.silence_rms),
            "sidecar" => s.sidecar = v.to_ascii_lowercase(),
            "autostart" => s.autostart = parse_bool(v, s.autostart),
            _ => {}
        }
    }
    s
}

/// Write the commented template if no settings file exists yet. Never overwrites the user's file.
pub fn ensure_template(path: &Path) -> std::io::Result<()> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, TEMPLATE)
}

fn parse_bool(v: &str, default: bool) -> bool {
    match v.to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => true,
        "false" | "no" | "off" | "0" => false,
        _ => default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_the_free_tier_shape() {
        let d = Settings::default();
        assert_eq!(d.mode, "auto");
        assert_eq!(d.sidecar, "native");
        assert!(d.overlay);
        assert!(!d.autostart);
    }

    #[test]
    fn parses_values_and_ignores_comments() {
        let dir = std::env::temp_dir().join(format!("nib_set_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("settings.toml");
        std::fs::write(
            &p,
            "# header\nmode = raw\noverlay = off   # not now\nsilence_rms = 0.01\nautostart = yes\nbogus = 1\n",
        )
        .unwrap();
        let s = load(&p);
        assert_eq!(s.mode, "raw");
        assert!(!s.overlay);
        assert_eq!(s.silence_rms, 0.01);
        assert!(s.autostart);
        // An unknown key must not disturb the rest.
        assert_eq!(s.sidecar, "native");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_bad_value_falls_back_instead_of_failing() {
        let dir = std::env::temp_dir().join(format!("nib_set2_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("settings.toml");
        std::fs::write(&p, "silence_rms = not-a-number\noverlay = maybe\n").unwrap();
        let s = load(&p);
        assert_eq!(s.silence_rms, Settings::default().silence_rms);
        assert_eq!(s.overlay, Settings::default().overlay);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_file_yields_defaults_and_template_round_trips() {
        let dir = std::env::temp_dir().join(format!("nib_set3_{}", std::process::id()));
        let p = dir.join("settings.toml");
        assert_eq!(load(&p), Settings::default());
        ensure_template(&p).unwrap();
        // The shipped template must parse back to exactly the defaults, or a fresh install would
        // silently behave differently from a default one.
        assert_eq!(load(&p), Settings::default());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
