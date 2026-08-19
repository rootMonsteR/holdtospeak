//! Bars style — the classic log-spaced FFT spectrum on a dark rounded panel.

use crate::mode::mode_colors;
use crate::paint::{fill_rect, fill_round_rect, premul};
use crate::{NBARS, OH, OW};

pub(crate) fn render_bars(px: &mut [u32], levels: &[f32; NBARS], mode: u8) {
    for p in px.iter_mut() {
        *p = 0;
    }
    fill_round_rect(px, OW, OH, 16, premul(18, 18, 26, 205)); // dark translucent panel
    let (c0, c1) = mode_colors(mode);
    let pad = 24;
    let gap = 3;
    let bw = ((OW - 2 * pad) - gap * (NBARS as i32 - 1)) / NBARS as i32;
    let base_y = OH - 10;
    let max_h = OH - 16;
    for (i, &lvl) in levels.iter().enumerate() {
        let h = (lvl * max_h as f32) as i32;
        let x0 = pad + i as i32 * (bw + gap);
        let t = i as f32 / (NBARS as f32 - 1.0);
        let r = (c0.0 as f32 + (c1.0 as f32 - c0.0 as f32) * t) as u8;
        let g = (c0.1 as f32 + (c1.1 as f32 - c0.1 as f32) * t) as u8;
        let b = (c0.2 as f32 + (c1.2 as f32 - c0.2 as f32) * t) as u8;
        let color = premul(r, g, b, 255);
        fill_rect(px, x0, base_y - h.max(2), bw, h.max(2), color);
    }
}
