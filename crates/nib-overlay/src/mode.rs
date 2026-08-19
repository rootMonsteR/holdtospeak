//! How the active cleanup mode (0=Raw..3=Email) is surfaced by the styles: an accent gradient
//! every theme tints with, plus the Hud's text callout.

/// Overlay-facing mode label. Kept local (no dependency on other crates) — mirrors nib-core's
/// `mode_label`: 0=Raw, 1=Auto, 2=Polish, 3=Email; any other index falls back to Polish.
pub(crate) fn mode_label(index: u8) -> &'static str {
    match index {
        0 => "RAW",
        1 => "AUTO",
        3 => "EMAIL",
        _ => "POLISH",
    }
}

// Per-mode accent (c0 -> c1 gradient across bars) so the spectrum's color shows the active mode.
pub(crate) fn mode_colors(m: u8) -> ((u8, u8, u8), (u8, u8, u8)) {
    match m {
        0 => ((175, 178, 190), (232, 234, 244)), // Raw   — neutral grey/white
        1 => ((40, 170, 255), (90, 220, 255)),   // Auto  — blue/cyan
        3 => ((190, 80, 255), (240, 120, 225)),  // Email — purple/magenta
        _ => ((40, 220, 150), (150, 240, 110)),  // Polish — green
    }
}
