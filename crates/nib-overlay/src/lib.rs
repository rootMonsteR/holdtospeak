//! `nib-overlay` — the platform-independent half of the always-on voice-spectrum overlay:
//! the pluggable styles ([`OverlayStyle`]), the spectrum DSP ([`compute_bars`] /
//! [`compute_bands`]), the per-frame animation state ([`Anim`]), and every pixel of rendering
//! ([`render_frame`]).
//!
//! Nothing here touches an OS: a caller hands in a `&mut [u32]` frame buffer of `OW * OH`
//! premultiplied-ARGB pixels and gets it painted. `nib-win32`'s `overlay.rs` owns the layered
//! window, the DIB blit and the message pump; a future Mac backend implements the same seam.
//! See docs/design/01-core-app-design.md §7.
#![forbid(unsafe_code)]

mod dsp;
mod font;
#[cfg(test)]
mod harness;
mod mode;
mod paint;
mod render;
mod rng;
mod style;

use render::hud::HudState;
use render::volt::VoltState;

pub use dsp::{compute_bands, compute_bars, fft_spectrum, hann_window, plan_fft};
pub use style::{OverlayStyle, STYLE_MAX_INDEX};

/// Panel width in px.
pub const OW: i32 = 460;
/// Panel height in px.
pub const OH: i32 = 84;
/// Bar count for the Bars style (the width of [`Anim`]'s level array).
pub const NBARS: usize = 32;
/// FFT window size — how many of the freshest mono samples each frame consumes.
pub const FFT_N: usize = 2048;
/// Gap between the panel's bottom edge and the bottom of the screen, in px.
pub const MARGIN_BOTTOM: i32 = 90;
/// Default spectrum sensitivity (the host may override it live, e.g. from `NIB_GAIN`).
pub const GAIN: f32 = 12.0;

const FMAX: f32 = 5000.0; // upper freq bound — vocal energy is mostly < 5 kHz
const DT: f32 = 1.0 / 60.0; // nominal frame time (16 ms loop)

/// Per-frame animation state shared by the live window loop and the headless dumper.
///
/// One `Anim` holds every style's clocks, so rebuilding it (`Anim::new()`) is how the host
/// replays the boot animation on a theme switch or a fresh push-to-talk activation.
pub struct Anim {
    levels: [f32; NBARS],
    bands: [f32; 3],
    wave_phase: f32,
    volt: VoltState,
    hud: HudState,
    t: f32,
}

impl Anim {
    pub fn new() -> Self {
        Anim {
            levels: [0.0; NBARS],
            bands: [0.0; 3],
            wave_phase: 0.0,
            volt: VoltState::new(),
            hud: HudState::new(),
            t: 0.0,
        }
    }

    /// Per-bar levels for the Bars style — fill via [`compute_bars`] before [`render_frame`].
    pub fn levels_mut(&mut self) -> &mut [f32; NBARS] {
        &mut self.levels
    }

    /// Low/mid/high voice bands for every other style — fill via [`compute_bands`].
    pub fn bands_mut(&mut self) -> &mut [f32; 3] {
        &mut self.bands
    }

    /// Advance the animation clock by one nominal frame (the loop is fixed-step at 60 fps, so
    /// the host paces frames to match rather than passing a measured delta).
    pub fn tick(&mut self) {
        self.t += DT;
    }
}

impl Default for Anim {
    fn default() -> Self {
        Self::new()
    }
}

/// Advance one frame and paint px. levels/bands must already be filled (live or synthetic).
/// `mode` (0=Raw..3=Email) drives the per-theme accent color + the Hud mode label.
pub fn render_frame(px: &mut [u32], style: OverlayStyle, a: &mut Anim, mode: u8) {
    match style {
        OverlayStyle::Bars => render::bars::render_bars(px, &a.levels, mode),
        OverlayStyle::Wave => {
            a.wave_phase += 0.13 + 0.24 * ((a.bands[0] + a.bands[1] + a.bands[2]) / 3.0);
            render::wave::render_wave(px, &a.bands, a.wave_phase, mode);
        }
        OverlayStyle::Volt => {
            a.volt.step(&a.bands);
            a.volt.render(px, mode);
        }
        OverlayStyle::Hud => {
            a.hud.step(&a.bands);
            a.hud.render(px, mode);
        }
    }
}
