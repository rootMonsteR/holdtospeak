//! Wave style — Siri-style flowing curves: each layer is a sine whose amplitude tracks a smoothed
//! voice band (low/mid/high) and whose phase drifts every frame, drawn with a soft additive glow.
//! A quiet hold shows a gentle idle ripple so the overlay still reads as "listening".

use crate::mode::mode_colors;
use crate::paint::{blend_add, fill_round_rect, lerp_u8, premul};
use crate::{OH, OW};

pub(crate) struct WaveLayer {
    cycles: f32, // full sine periods across the panel
    speed: f32,  // phase multiplier (sign = direction)
    band: usize, // which of bands[3] drives the amplitude
    alpha: f32,  // layer opacity
}

const WAVE_LAYERS: [WaveLayer; 3] = [
    WaveLayer {
        cycles: 1.6,
        speed: 1.0,
        band: 0,
        alpha: 0.90,
    },
    WaveLayer {
        cycles: 2.7,
        speed: -1.4,
        band: 1,
        alpha: 0.62,
    },
    WaveLayer {
        cycles: 4.1,
        speed: 1.9,
        band: 2,
        alpha: 0.45,
    },
];

/// Vertical glow around the curve point: bright ~2 px core, soft falloff above/below.
fn glow_column(px: &mut [u32], x: i32, y: f32, r: u8, g: u8, b: u8, alpha: f32) {
    let yc = y as i32;
    for dy in -3i32..=3 {
        let py = yc + dy;
        if !(0..OH).contains(&py) {
            continue;
        }
        let dist = ((py as f32 + 0.5) - y).abs();
        let fall = (1.0 - dist / 3.5).clamp(0.0, 1.0);
        let core = if dist < 1.2 { 1.0 } else { 0.55 };
        blend_add(
            &mut px[(py * OW + x) as usize],
            r,
            g,
            b,
            alpha * fall * fall * core,
        );
    }
}

pub(crate) fn render_wave(px: &mut [u32], bands: &[f32; 3], phase: f32, mode: u8) {
    for p in px.iter_mut() {
        *p = 0;
    }
    // fainter, sleeker backdrop than the bars panel — the glow does the talking
    fill_round_rect(px, OW, OH, 20, premul(14, 14, 22, 150));
    let (c0, c1) = mode_colors(mode);
    let pad = 18;
    let span = (OW - 2 * pad) as f32;
    let cy = OH as f32 / 2.0;
    let max_amp = OH as f32 * 0.36;
    for (li, l) in WAVE_LAYERS.iter().enumerate() {
        let t = li as f32 / (WAVE_LAYERS.len() - 1) as f32;
        let (r, g, b) = (
            lerp_u8(c0.0, c1.0, t),
            lerp_u8(c0.1, c1.1, t),
            lerp_u8(c0.2, c1.2, t),
        );
        let amp = 0.08 + bands[l.band] * 0.92; // 0.08 = idle ripple while quiet
        for xi in 0..(OW - 2 * pad) {
            let u = xi as f32 / span;
            let env = (std::f32::consts::PI * u).sin(); // taper to the midline at the edges
            let y = cy
                + amp
                    * max_amp
                    * env
                    * (std::f32::consts::TAU * l.cycles * u + phase * l.speed + li as f32 * 1.7)
                        .sin();
            glow_column(px, pad + xi, y, r, g, b, l.alpha * (0.35 + 0.65 * env));
        }
    }
}
