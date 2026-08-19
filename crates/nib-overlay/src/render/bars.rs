//! Bars style — a studio spectrum meter: log-spaced FFT bars with falling peak-holds and a
//! reflection, on a dark rounded panel.
//!
//! The original was a flat block of solid rectangles, which read as a generic media-player
//! visualiser. Three things do the work here: a vertical gradient so each bar has a bright tip
//! rather than a flat top, **peak-hold caps** that hang and fall (the detail that makes real
//! meters feel responsive — they show what just happened, not only what is happening), and a
//! short reflection that grounds the bars instead of leaving them floating on the panel edge.

use crate::mode::mode_colors;
use crate::paint::{blend_add, fill_round_rect, mix_rgb, premul};
use crate::render::chrome;
use crate::{NBARS, OH, OW};

/// How fast a peak-hold cap falls, per frame at 60 fps. Slow enough to read as a held value,
/// fast enough that it never lags the voice.
const PEAK_FALL: f32 = 0.011;

const PAD_X: i32 = 22;
const GAP: i32 = 3;
/// Baseline the bars stand on, leaving room beneath for the reflection.
const BASE_Y: i32 = OH - 19;
/// Reflection height as a fraction of the bar, and how strong it starts.
const REFLECT_FRAC: f32 = 0.42;
const REFLECT_ALPHA: f32 = 0.22;

pub(crate) struct BarsState {
    peaks: [f32; NBARS],
}

impl BarsState {
    pub(crate) fn new() -> Self {
        BarsState {
            peaks: [0.0; NBARS],
        }
    }

    /// Track each bar's recent maximum, decaying steadily once the signal drops below it.
    pub(crate) fn step(&mut self, levels: &[f32; NBARS]) {
        for (p, &l) in self.peaks.iter_mut().zip(levels.iter()) {
            *p = if l > *p { l } else { (*p - PEAK_FALL).max(l) };
        }
    }

    pub(crate) fn render(&self, px: &mut [u32], levels: &[f32; NBARS], mode: u8) {
        for p in px.iter_mut() {
            *p = 0;
        }
        fill_round_rect(px, OW, OH, 16, premul(18, 18, 26, 205));

        let (c0, c1) = mode_colors(mode);
        let top = chrome::ROW_H + 3;
        let max_h = BASE_Y - top;
        let bw = ((OW - 2 * PAD_X) - GAP * (NBARS as i32 - 1)) / NBARS as i32;

        for (i, &lvl) in levels.iter().enumerate() {
            let x0 = PAD_X + i as i32 * (bw + GAP);
            let h = ((lvl.clamp(0.0, 1.0) * max_h as f32) as i32).max(2);

            // Body: gradient bottom -> top, so the tip is the brightest part of the bar.
            for y in (BASE_Y - h)..BASE_Y {
                let t = (BASE_Y - y) as f32 / max_h as f32;
                let c = mix_rgb(c0, c1, t.clamp(0.0, 1.0).powf(0.65));
                // Slight lift near the very tip reads as a highlight without a separate pass.
                let a = 0.88 + 0.12 * t;
                for x in x0..x0 + bw {
                    if (0..OW).contains(&x) && (0..OH).contains(&y) {
                        blend_add(&mut px[(y * OW + x) as usize], c.0, c.1, c.2, a);
                    }
                }
            }

            // Reflection: mirrored, fading out. Grounds the bar on the panel.
            let rh = (h as f32 * REFLECT_FRAC) as i32;
            for k in 0..rh {
                let y = BASE_Y + 2 + k;
                if y >= OH {
                    break;
                }
                let f = 1.0 - k as f32 / rh.max(1) as f32;
                let c = mix_rgb(c0, c1, 0.25);
                for x in x0..x0 + bw {
                    if (0..OW).contains(&x) {
                        blend_add(
                            &mut px[(y * OW + x) as usize],
                            c.0,
                            c.1,
                            c.2,
                            REFLECT_ALPHA * f * f,
                        );
                    }
                }
            }

            // Peak-hold cap, floating just above the current level.
            let ph = (self.peaks[i].clamp(0.0, 1.0) * max_h as f32) as i32;
            let py = BASE_Y - ph.max(3) - 2;
            if py > top {
                for y in py..(py + 2) {
                    for x in x0..x0 + bw {
                        if (0..OW).contains(&x) && (0..OH).contains(&y) {
                            blend_add(&mut px[(y * OW + x) as usize], c1.0, c1.1, c1.2, 0.95);
                        }
                    }
                }
            }
        }

        // Overall level for the shared row: the loudest band, not the mean — a mean across 32
        // bins reads far too quiet on speech, which is mostly low-frequency energy.
        let level = levels.iter().copied().fold(0.0f32, f32::max);
        chrome::status_row(px, mode, level);
    }
}
