//! Hotkey configuration: parse combo strings like `"Ctrl+Alt+M"` and load `hotkeys.toml` into
//! `nib_platform::Binding`s. Pure — VK codes are plain u16 (standard Windows virtual-key values),
//! so this crate never touches `windows::`. The hook (`nib-win32`) re-derives the modifier
//! bitmask and main key from `Binding::keys`.
#![forbid(unsafe_code)]

pub mod settings;
pub use settings::Settings;

use std::path::Path;

use nib_platform::{Binding, ChordAction};

/// Modifier bits used while parsing (Ctrl=1, Alt=2, Shift=4, Win=8).
pub const MOD_CTRL: u8 = 1;
pub const MOD_ALT: u8 = 2;
pub const MOD_SHIFT: u8 = 4;
pub const MOD_WIN: u8 = 8;

// Canonical virtual-key codes emitted into `Binding::keys` for each modifier.
const VK_CTRL: u16 = 0x11;
const VK_ALT: u16 = 0x12;
const VK_SHIFT: u16 = 0x10;
const VK_LWIN: u16 = 0x5B;

/// Arming delay: Ctrl+Win+Arrow etc. still pass through for this window before PTT engages.
pub const ARMING_MS: u16 = 120;

/// Modifier name → bit. Accepts common aliases (super/meta/cmd = Win).
pub fn name_to_mod(s: &str) -> Option<u8> {
    match s.trim().to_ascii_lowercase().as_str() {
        "ctrl" | "control" => Some(MOD_CTRL),
        "alt" => Some(MOD_ALT),
        "shift" => Some(MOD_SHIFT),
        "win" | "super" | "meta" | "cmd" => Some(MOD_WIN),
        _ => None,
    }
}

/// Key name → virtual-key code. Single alphanumerics, F1–F12, and Space/Enter/Tab.
pub fn name_to_vk(s: &str) -> Option<u16> {
    let s = s.trim();
    if s.chars().count() == 1 {
        let c = s.chars().next().unwrap().to_ascii_uppercase();
        if c.is_ascii_alphanumeric() {
            return Some(c as u16);
        }
    }
    let low = s.to_ascii_lowercase();
    if let Some(n) = low.strip_prefix('f') {
        if let Ok(num) = n.parse::<u16>() {
            if (1..=12).contains(&num) {
                return Some(0x70 + num - 1); // VK_F1..VK_F12
            }
        }
    }
    match low.as_str() {
        "space" => Some(0x20),
        "enter" | "return" => Some(0x0D),
        "tab" => Some(0x09),
        _ => None,
    }
}

/// Parse a combo like `"Ctrl+Alt+M"` into `(mods_bitmask, key_vk)`; `key_vk == 0` for a
/// modifier-only combo (e.g. push-to-talk).
pub fn parse_combo(v: &str) -> (u8, u16) {
    let (mut mods, mut key) = (0u8, 0u16);
    for part in v.split('+') {
        let p = part.trim();
        if let Some(m) = name_to_mod(p) {
            mods |= m;
        } else if let Some(k) = name_to_vk(p) {
            key = k;
        }
    }
    (mods, key)
}

fn mod_to_vk(m: u8) -> u16 {
    match m {
        MOD_CTRL => VK_CTRL,
        MOD_ALT => VK_ALT,
        MOD_SHIFT => VK_SHIFT,
        MOD_WIN => VK_LWIN,
        _ => 0,
    }
}

/// Build a `Binding` from a combo string. `keys` = canonical modifier VKs (in Ctrl/Alt/Shift/Win
/// order) plus the optional main key. Returns None if the combo has no modifiers.
pub fn binding_from_combo(v: &str, suppress: bool) -> Option<Binding> {
    let (mods, key) = parse_combo(v);
    if mods == 0 {
        return None;
    }
    let mut keys = Vec::new();
    for m in [MOD_CTRL, MOD_ALT, MOD_SHIFT, MOD_WIN] {
        if mods & m != 0 {
            keys.push(mod_to_vk(m));
        }
    }
    if key != 0 {
        keys.push(key);
    }
    Some(Binding {
        keys,
        arming_ms: ARMING_MS,
        suppress,
    })
}

/// The resolved hotkey set: push-to-talk (modifier-only) plus the optional secondary chords.
///
/// Order matters — [`Hotkeys::chords`] flattens these into the indexed slice the hook streams
/// back, so the numbering here is the contract between config and the event consumer.
pub struct Hotkeys {
    pub ptt: Binding,
    pub cycle_mode: Option<Binding>,
    pub cycle_style: Option<Binding>,
    pub quit: Option<Binding>,
}

impl Default for Hotkeys {
    fn default() -> Self {
        Hotkeys {
            ptt: binding_from_combo("Ctrl+Win", true).expect("valid default ptt"),
            cycle_mode: binding_from_combo("Ctrl+Alt+M", true),
            // Overlay theme. `O` for overlay: Ctrl+Alt+T is taken by a terminal in most Linux
            // muscle memory and by several Windows tools, and this has to be safe to press.
            cycle_style: binding_from_combo("Ctrl+Alt+O", true),
            // Quit needs a hotkey because the console window is easy to lose behind other windows,
            // and the banner advertises `q` without saying it means "typed into that console".
            quit: binding_from_combo("Ctrl+Alt+Q", true),
        }
    }
}

impl Hotkeys {
    /// Flatten the configured chords into (binding, action) pairs, skipping any that are unset or
    /// invalid. Built in one place so the hook's indices and the consumer's meanings cannot drift.
    pub fn chords(&self) -> (Vec<Binding>, Vec<ChordAction>) {
        let mut b = Vec::new();
        let mut a = Vec::new();
        for (binding, action) in [
            (&self.cycle_mode, ChordAction::CycleMode),
            (&self.cycle_style, ChordAction::CycleStyle),
            (&self.quit, ChordAction::Quit),
        ] {
            if let Some(x) = binding {
                b.push(x.clone());
                a.push(action);
            }
        }
        (b, a)
    }

    /// Like [`Hotkeys::chords`] but positional: always three slots, in `CycleMode`, `CycleStyle`,
    /// `Quit` order, with a disabled chord as an empty (never-matching) binding. The hook treats a
    /// slot with no modifiers or no main key as inert, so this keeps slot *indices* stable — which
    /// is what lets the settings window rebind live without the forwarder's action table drifting.
    pub fn chord_slots(&self) -> (Vec<Binding>, Vec<ChordAction>) {
        let inert = Binding {
            keys: Vec::new(),
            arming_ms: ARMING_MS,
            suppress: true,
        };
        let slot = |b: &Option<Binding>| b.clone().unwrap_or_else(|| inert.clone());
        (
            vec![
                slot(&self.cycle_mode),
                slot(&self.cycle_style),
                slot(&self.quit),
            ],
            vec![
                ChordAction::CycleMode,
                ChordAction::CycleStyle,
                ChordAction::Quit,
            ],
        )
    }
}

/// Apply one configured chord: an explicit `off`/`none` clears it, a valid combo replaces it,
/// and anything unparseable leaves the default alone rather than silently disabling the key.
fn set_chord(slot: &mut Option<Binding>, v: &str) {
    if matches!(v.trim().to_ascii_lowercase().as_str(), "off" | "none" | "") {
        *slot = None;
        return;
    }
    let (m, key) = parse_combo(v);
    if m != 0 && key != 0 {
        *slot = binding_from_combo(v, true);
    }
}

/// Human-readable combo for a binding (`"Ctrl+Alt+M"`), the inverse of [`binding_from_combo`]:
/// modifiers in Ctrl/Alt/Shift/Win order, then the main key by its config name.
pub fn combo_name(b: &Binding) -> String {
    b.keys
        .iter()
        .map(|&vk| match vk {
            VK_CTRL => "Ctrl".to_string(),
            VK_ALT => "Alt".to_string(),
            VK_SHIFT => "Shift".to_string(),
            VK_LWIN | 0x5C => "Win".to_string(),
            0x20 => "Space".to_string(),
            0x0D => "Enter".to_string(),
            0x09 => "Tab".to_string(),
            0x30..=0x5A => ((vk as u8) as char).to_string(),
            0x70..=0x7B => format!("F{}", vk - 0x6F),
            other => format!("VK{other:#04X}"),
        })
        .collect::<Vec<_>>()
        .join("+")
}

/// Write `hotkeys.toml` from `hk` in the same shape [`load`] reads — a disabled chord is written
/// as `off`, so the file always says what every key does (and doesn't).
pub fn save(path: &Path, hk: &Hotkeys) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let chord = |b: &Option<Binding>| match b {
        Some(b) => format!("\"{}\"", combo_name(b)),
        None => "off".to_string(),
    };
    let text = format!(
        "\
# HoldToSpeak hotkeys. Plain text on purpose — edit freely; `off` disables a chord.
# Modifiers: Ctrl, Alt, Shift, Win. Push-to-talk is modifiers only; the others need a main key.

ptt         = \"{ptt}\"
cycle_mode  = {cycle_mode}
cycle_style = {cycle_style}
quit        = {quit}
",
        ptt = combo_name(&hk.ptt),
        cycle_mode = chord(&hk.cycle_mode),
        cycle_style = chord(&hk.cycle_style),
        quit = chord(&hk.quit),
    );
    std::fs::write(path, text)
}

/// Load hotkeys from a simple `key = combo` file (`hotkeys.toml`). Blank / `#`-comment lines are
/// ignored; a missing file yields defaults. Values may be optionally quoted. A `cycle_mode`
/// combo needs a main key (else it's ignored).
pub fn load(path: &Path) -> Hotkeys {
    let mut hk = Hotkeys::default();
    let Ok(text) = std::fs::read_to_string(path) else {
        return hk;
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let v = v.trim().trim_matches('"');
        match k.trim().to_ascii_lowercase().as_str() {
            "ptt" => {
                if let Some(b) = binding_from_combo(v, true) {
                    hk.ptt = b;
                }
            }
            // Every chord needs a modifier AND a main key; a modifier-only chord would fight
            // push-to-talk. `off`/`none` disables one.
            "cycle_mode" | "cycle" => set_chord(&mut hk.cycle_mode, v),
            "cycle_style" | "cycle_overlay" | "style" => set_chord(&mut hk.cycle_style, v),
            "quit" => set_chord(&mut hk.quit, v),
            _ => {}
        }
    }
    hk
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The hook reports a chord by INDEX, so the pairing between position and meaning is a
    /// contract. Disabling one chord must shift the rest's indices *together with* their actions,
    /// never renumber one without the other — that would silently rebind a key to a new action.
    /// `save` is what the settings window writes; `load` is what the app reads on the next start.
    /// They must agree exactly, with a disabled chord surviving as `off`.
    #[test]
    fn save_then_load_round_trips_including_off() {
        let dir = std::env::temp_dir().join(format!("nib_hk_{}", std::process::id()));
        let p = dir.join("hotkeys.toml");
        let hk = Hotkeys {
            ptt: binding_from_combo("Ctrl+Shift", true).unwrap(),
            cycle_mode: binding_from_combo("Ctrl+Alt+F3", true),
            cycle_style: None,
            quit: binding_from_combo("Win+Shift+Q", true),
        };
        save(&p, &hk).unwrap();
        let back = load(&p);
        assert_eq!(combo_name(&back.ptt), "Ctrl+Shift");
        assert_eq!(
            back.cycle_mode.as_ref().map(combo_name).as_deref(),
            Some("Ctrl+Alt+F3")
        );
        assert!(back.cycle_style.is_none(), "off must read back as disabled");
        assert_eq!(
            back.quit.as_ref().map(combo_name).as_deref(),
            Some("Shift+Win+Q")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `combo_name` is the inverse of `binding_from_combo` (modulo canonical modifier order).
    #[test]
    fn combo_name_inverts_parsing() {
        for (input, canonical) in [
            ("Ctrl+Win", "Ctrl+Win"),
            ("win+ctrl", "Ctrl+Win"),
            ("Ctrl+Alt+M", "Ctrl+Alt+M"),
            ("Shift+Space", "Shift+Space"),
            ("Alt+F12", "Alt+F12"),
        ] {
            let b = binding_from_combo(input, true).unwrap();
            assert_eq!(combo_name(&b), canonical, "{input}");
        }
    }

    /// Positional slots never shift: a disabled chord is an inert placeholder, not a gap.
    #[test]
    fn chord_slots_are_positional_and_inert_when_off() {
        let hk = Hotkeys {
            cycle_mode: None,
            ..Hotkeys::default()
        };
        let (b, a) = hk.chord_slots();
        assert_eq!(b.len(), 3);
        assert!(b[0].keys.is_empty(), "disabled slot is inert");
        assert_eq!(
            a,
            vec![
                ChordAction::CycleMode,
                ChordAction::CycleStyle,
                ChordAction::Quit
            ]
        );
    }

    #[test]
    fn chord_indices_travel_with_their_actions() {
        let (b, a) = Hotkeys::default().chords();
        assert_eq!(b.len(), a.len());
        assert_eq!(
            a,
            vec![
                ChordAction::CycleMode,
                ChordAction::CycleStyle,
                ChordAction::Quit
            ]
        );

        let hk = Hotkeys {
            cycle_mode: None,
            ..Hotkeys::default()
        };
        let (b2, a2) = hk.chords();
        assert_eq!(b2.len(), 2);
        assert_eq!(a2, vec![ChordAction::CycleStyle, ChordAction::Quit]);
    }

    /// `off` disables a chord; junk leaves the default alone rather than silently killing the key.
    #[test]
    fn chord_can_be_disabled_but_junk_keeps_the_default() {
        let mut slot = binding_from_combo("Ctrl+Alt+Q", true);
        set_chord(&mut slot, "off");
        assert!(slot.is_none(), "`off` must disable");

        let mut slot = binding_from_combo("Ctrl+Alt+Q", true);
        set_chord(&mut slot, "not a combo");
        assert!(slot.is_some(), "unparseable input must not disable the key");

        let mut slot = None;
        set_chord(&mut slot, "Ctrl+Shift+F5");
        assert!(slot.is_some(), "a valid combo must set it");
    }

    #[test]
    fn parse_combo_basics() {
        assert_eq!(parse_combo("Ctrl+Win"), (MOD_CTRL | MOD_WIN, 0));
        assert_eq!(parse_combo("Ctrl+Alt+M"), (MOD_CTRL | MOD_ALT, 0x4D));
        assert_eq!(
            parse_combo(" ctrl + shift + F5 "),
            (MOD_CTRL | MOD_SHIFT, 0x74)
        );
        assert_eq!(parse_combo("super+space"), (MOD_WIN, 0x20));
        assert_eq!(parse_combo("bogus"), (0, 0));
    }

    #[test]
    fn name_to_vk_cases() {
        assert_eq!(name_to_vk("a"), Some(b'A' as u16));
        assert_eq!(name_to_vk("F1"), Some(0x70));
        assert_eq!(name_to_vk("F12"), Some(0x7B));
        assert_eq!(name_to_vk("F13"), None);
        assert_eq!(name_to_vk("enter"), Some(0x0D));
        assert_eq!(name_to_vk("ctrl"), None);
    }

    #[test]
    fn binding_keys_are_canonical_vks() {
        let ptt = binding_from_combo("Ctrl+Win", true).unwrap();
        assert_eq!(ptt.keys, vec![VK_CTRL, VK_LWIN]);
        let cycle = binding_from_combo("Ctrl+Alt+M", true).unwrap();
        assert_eq!(cycle.keys, vec![VK_CTRL, VK_ALT, 0x4D]);
        assert!(binding_from_combo("M", true).is_none()); // no modifier → rejected
    }
}
