//! Wave style — an oscilloscope trace: one composite waveform built from the three voice bands,
//! drawn over a faint graticule with a motion trail behind it.
//!
//! The original drew three independent sine layers of similar weight. Pretty, but it read as an
//! abstract screensaver — three crossing curves say "decoration", whereas one dominant trace over
//! a grid says "this is a measurement of your voice". So the bands now sum into a single line
//! (harmonics of one signal, not three signals), and two phase-lagged ghosts trail behind it so
//! motion is legible frame to frame rather than just busy.

use crate::mode::mode_colors;
use crate::paint::{blend_add, fill_round_rect, lerp_u8, premul};
use crate::render::chrome;
use crate::{OH, OW};

struct Harmonic {
    cycles: f32, // full periods across the panel
    speed: f32,  // phase multiplier (sign sets travel direction)
    band: usize, // which voice band drives its amplitude
    weight: f32, // contribution to the summed trace
}

/// Harmonics of ONE trace. Low band is the fundamental and carries the shape; the upper two add
/// detail so consonants visibly roughen the line instead of just raising it.
const HARMONICS: [Harmonic; 3] = [
    Harmonic {
        cycles: 2.1,
        speed: 1.0,
        band: 0,
        weight: 1.00,
    },
    Harmonic {
        cycles: 3.7,
        speed: -1.4,
        band: 1,
        weight: 0.58,
    },
    Harmonic {
        cycles: 6.9,
        speed: 1.9,
        band: 2,
        weight: 0.34,
    },
];

const PAD_X: i32 = 18;

/// Vertical glow around a trace point: bright core, soft falloff.
fn glow_column(px: &mut [u32], x: i32, y: f32, c: (u8, u8, u8), alpha: f32, reach: f32) {
    let yc = y as i32;
    let r = reach.ceil() as i32;
    for dy in -r..=r {
        let py = yc + dy;
        if !(0..OH).contains(&py) {
            continue;
        }
        let dist = ((py as f32 + 0.5) - y).abs();
        let fall = (1.0 - dist / reach).clamp(0.0, 1.0);
        let core = if dist < 1.1 { 1.0 } else { 0.5 };
        blend_add(
            &mut px[(py * OW + x) as usize],
            c.0,
            c.1,
            c.2,
            alpha * fall * fall * core,
        );
    }
}

/// Height of the trace at `u` (0..1 across the panel) for a given phase.
fn trace_y(u: f32, bands: &[f32; 3], phase: f32, cy: f32, max_amp: f32) -> f32 {
    // Taper to the midline at both edges so the trace enters and leaves cleanly.
    let env = (std::f32::consts::PI * u).sin();
    let mut sum = 0.0;
    for (hi, h) in HARMONICS.iter().enumerate() {
        let amp = 0.06 + bands[h.band] * 0.94; // 0.06 keeps a faint idle ripple while quiet
        sum += h.weight
            * amp
            * (std::f32::consts::TAU * h.cycles * u + phase * h.speed + hi as f32 * 1.7).sin();
    }
    // Normalise against the FUNDAMENTAL plus a share of the rest, not the full weight sum: the
    // harmonics rarely peak together, so dividing by the total flattened the trace into one lazy
    // bump. Soft-clip instead, which keeps the detail and still cannot punch through the panel.
    let norm = HARMONICS[0].weight + 0.45 * HARMONICS[1..].iter().map(|h| h.weight).sum::<f32>();
    cy + (sum / norm).clamp(-1.0, 1.0) * env * max_amp
}

/// Faint scope graticule: a dashed centre axis plus evenly spaced vertical ticks.
fn graticule(px: &mut [u32], top: i32, bottom: i32, cy: i32, c: (u8, u8, u8)) {
    for x in PAD_X..(OW - PAD_X) {
        if (x / 4) % 2 == 0 {
            blend_add(&mut px[(cy * OW + x) as usize], c.0, c.1, c.2, 0.17);
        }
    }
    let divisions = 8;
    let span = OW - 2 * PAD_X;
    for d in 1..divisions {
        let x = PAD_X + d * span / divisions;
        for y in top..bottom {
            // Ticks are short marks at the axis, not full-height rules: a full grid competes with
            // the trace for attention at this size.
            let near = (y - cy).abs();
            if near <= 5 {
                blend_add(&mut px[(y * OW + x) as usize], c.0, c.1, c.2, 0.20);
            }
        }
    }
}

pub(crate) fn render_wave(px: &mut [u32], bands: &[f32; 3], phase: f32, mode: u8) {
    for p in px.iter_mut() {
        *p = 0;
    }
    fill_round_rect(px, OW, OH, 18, premul(13, 14, 22, 172));

    let (c0, c1) = mode_colors(mode);
    let top = chrome::ROW_H + 2;
    let bottom = OH - 5;
    let cy = (top + bottom) as f32 / 2.0;
    let max_amp = (bottom - top) as f32 * 0.50;

    graticule(px, top, bottom, cy as i32, c0);

    // Two ghosts of the SAME trace at earlier phases, then the live trace on top. Drawing the
    // trail first means the bright core always wins where they overlap.
    let ghosts = [(-0.80f32, 0.20f32, 3.0f32), (-0.40, 0.36, 3.4)];
    let span = (OW - 2 * PAD_X) as f32;
    for (dphase, alpha, reach) in ghosts {
        for xi in 0..(OW - 2 * PAD_X) {
            let u = xi as f32 / span;
            let y = trace_y(u, bands, phase + dphase, cy, max_amp);
            let env = (std::f32::consts::PI * u).sin();
            let c = (
                lerp_u8(c0.0, c1.0, u),
                lerp_u8(c0.1, c1.1, u),
                lerp_u8(c0.2, c1.2, u),
            );
            glow_column(px, PAD_X + xi, y, c, alpha * (0.35 + 0.65 * env), reach);
        }
    }
    for xi in 0..(OW - 2 * PAD_X) {
        let u = xi as f32 / span;
        let y = trace_y(u, bands, phase, cy, max_amp);
        let env = (std::f32::consts::PI * u).sin();
        let c = (
            lerp_u8(c0.0, c1.0, u),
            lerp_u8(c0.1, c1.1, u),
            lerp_u8(c0.2, c1.2, u),
        );
        glow_column(px, PAD_X + xi, y, c, 0.95 * (0.4 + 0.6 * env), 4.0);
    }

    let level = (bands[0] + bands[1] + bands[2]) / 3.0;
    chrome::status_row(px, mode, level);
}
