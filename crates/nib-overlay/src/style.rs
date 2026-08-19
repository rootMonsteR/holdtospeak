//! The pluggable overlay themes and their tray/CLI identities.

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OverlayStyle {
    Bars,
    Wave,
    Volt,
    Hud,
    // Planned (see OVERLAY-STYLES.md): Pill
}

impl OverlayStyle {
    /// Every style, in tray-menu display order (most-preferred first). Single source of truth for
    /// the picker, so adding a variant can't leave the menu stale or out of bounds.
    pub const ALL: &'static [Self] = &[Self::Hud, Self::Volt, Self::Wave, Self::Bars];

    /// Label shown in the tray's theme picker.
    pub const fn menu_label(self) -> &'static str {
        match self {
            Self::Bars => "Overlay: Bars",
            Self::Wave => "Overlay: Wave",
            Self::Volt => "Overlay: Volt (electric)",
            Self::Hud => "Overlay: HUD (comms)",
        }
    }

    pub fn from_token(s: &str) -> Option<Self> {
        match s {
            "bars" => Some(Self::Bars),
            "wave" => Some(Self::Wave),
            "volt" => Some(Self::Volt),
            "hud" => Some(Self::Hud),
            _ => None,
        }
    }

    /// Bars renders from per-bar levels; every other style renders from the 3 voice bands.
    /// Single source of truth for the input-shape dispatch (run loop + dump_frames).
    pub fn needs_levels(self) -> bool {
        matches!(self, Self::Bars)
    }

    pub const fn index(self) -> u8 {
        match self {
            Self::Bars => 0,
            Self::Wave => 1,
            Self::Volt => 2,
            Self::Hud => 3,
        }
    }

    pub const fn from_index(i: u8) -> Self {
        match i {
            0 => Self::Bars,
            1 => Self::Wave,
            2 => Self::Volt,
            _ => Self::Hud,
        }
    }
}

/// Highest style index accepted by the tray menu dispatch (see nib-win32's tray.rs).
/// The test below pins this to `ALL` — if you add a style (e.g. Pill), the test forces you to
/// update `ALL`, `index`/`from_index`, and this constant together, or the tray menu would show
/// the new style but silently drop clicks on it.
pub const STYLE_MAX_INDEX: u8 = OverlayStyle::Hud.index();

#[cfg(test)]
mod tests {
    use super::*;

    /// ALL ↔ index() ↔ STYLE_MAX_INDEX must stay contiguous and consistent, or the tray menu
    /// (which iterates ALL but range-checks against STYLE_MAX_INDEX) desyncs.
    #[test]
    fn styles_are_contiguous_and_max_index_matches() {
        let mut seen = vec![false; OverlayStyle::ALL.len()];
        for s in OverlayStyle::ALL {
            let i = s.index() as usize;
            assert!(i < seen.len(), "index {i} outside ALL's range");
            assert!(!seen[i], "duplicate index {i}");
            seen[i] = true;
            assert_eq!(OverlayStyle::from_index(s.index()), *s, "round trip {s:?}");
        }
        assert!(seen.iter().all(|&v| v), "ALL must cover every index once");
        let max = OverlayStyle::ALL.iter().map(|s| s.index()).max().unwrap();
        assert_eq!(STYLE_MAX_INDEX, max, "STYLE_MAX_INDEX drifted from ALL");
    }
}
