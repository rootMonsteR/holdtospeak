//! Hud style — the tactical comms overlay: the "talking to mission control" panel from Halo /
//! CoD / Splinter Cell. Angular dark plate with corner brackets and scanlines, a scrolling
//! mirrored voiceprint (comms oscilloscope), a blinking TRANSMITTING indicator, the cleanup mode
//! as a text callout, and a signal meter. Boots with a brief flicker when the PTT opens the
//! channel.

use crate::font::{draw_text, text_width};
use crate::mode::mode_label;
use crate::paint::{blend_add, fill_chamfer_rect, fill_rect, lerp_u8, premul, smoothstep};
use crate::{DT, OH, OW};

const HUD_HIST: usize = 142; // voiceprint history slices
const HUD_WAVE_X: i32 = 56; // waveform region left edge (right of the AI orb)
const HUD_WAVE_W: i32 = OW - 16 - HUD_WAVE_X; // waveform region width
const HUD_WAVE_Y: i32 = 46; // waveform midline
const HUD_WAVE_MAX: f32 = 16.0; // half-height px
const HUD_PUSH_EVERY: u32 = 2; // scroll speed: one new slice per N frames
const HUD_BOOT: f32 = 0.14; // boot-flicker duration s
const HUD_BLINK: f32 = 0.8; // transmit-dot blink period s
const HUD_CUT: i32 = 10; // chamfered corner size (sci-fi plate silhouette)
const HUD_ORB: (i32, i32) = (30, 46); // AI-core orb center

const HUD_BG: (u8, u8, u8, u8) = (8, 12, 18, 212);
const HUD_SCAN: (u8, u8, u8, u8) = (5, 8, 13, 212); // scanline rows
const HUD_EDGE: (u8, u8, u8) = (49, 214, 255); // cyan
const HUD_TEXT: (u8, u8, u8) = (140, 225, 255);
const HUD_DIM: (u8, u8, u8) = (36, 110, 140);
const HUD_WHITE: (u8, u8, u8) = (225, 245, 255);
const HUD_AMBER: (u8, u8, u8) = (255, 176, 46); // transmit dot

/// Additive rectangle (border pieces, brackets, meter bars).
fn hud_rect(px: &mut [u32], x: i32, y: i32, w: i32, h: i32, color: (u8, u8, u8), alpha: f32) {
    for yy in y.max(0)..(y + h).min(OH) {
        for xx in x.max(0)..(x + w).min(OW) {
            blend_add(
                &mut px[(yy * OW + xx) as usize],
                color.0,
                color.1,
                color.2,
                alpha,
            );
        }
    }
}

/// Breathing AI-core orb: white-hot center, expanding ring that swells with voice level,
/// soft halo. The "assistant is listening" focal point.
fn hud_orb(px: &mut [u32], level: f32, t: f32, boot: f32) {
    let (cx, cy) = HUD_ORB;
    let ring_r = 6.0 + 2.8 * level + 0.7 * (t * 2.1).sin();
    for dy in -12i32..=12 {
        for dx in -12i32..=12 {
            let (x, y) = (cx + dx, cy + dy);
            if !(0..OW).contains(&x) || !(0..OH).contains(&y) {
                continue;
            }
            let d = ((dx * dx + dy * dy) as f32).sqrt();
            let core = (1.0 - d / 3.8).max(0.0);
            let ring = (1.0 - (d - ring_r).abs() / 1.7).max(0.0);
            let halo = (1.0 - d / 12.0).max(0.0) * 0.16 * (0.4 + 0.6 * level);
            let a = (core * 0.95 + ring * 0.85 + halo).min(1.0) * boot;
            if a > 0.012 {
                let c = (
                    lerp_u8(HUD_EDGE.0, HUD_WHITE.0, core),
                    lerp_u8(HUD_EDGE.1, HUD_WHITE.1, core),
                    lerp_u8(HUD_EDGE.2, HUD_WHITE.2, core),
                );
                blend_add(&mut px[(y * OW + x) as usize], c.0, c.1, c.2, a);
            }
        }
    }
}

// `t` is a virtual clock (DT per frame): it doubles as time-since-channel-open because the
// state is rebuilt whenever the overlay hides or the theme switches. Virtual rather than wall
// time keeps headless dumps deterministic; under heavy load the timecode can lag wall time —
// accepted for the spike.
pub(crate) struct HudState {
    t: f32,
    frame: u32,
    level: f32, // smoothed voice level 0..1
    hist: [f32; HUD_HIST],
}

impl HudState {
    pub(crate) fn new() -> Self {
        HudState {
            t: 0.0,
            frame: 0,
            level: 0.0,
            hist: [0.0; HUD_HIST],
        }
    }

    pub(crate) fn step(&mut self, bands: &[f32; 3]) {
        self.t += DT;
        self.frame = self.frame.wrapping_add(1);
        // voice-weighted level with fast attack / slow decay (voiceprint reads syllables)
        let v = (bands[0] * 0.5 + bands[1] * 0.35 + bands[2] * 0.15).clamp(0.0, 1.0);
        self.level = if v > self.level {
            self.level * 0.5 + v * 0.5
        } else {
            self.level * 0.90 + v * 0.10
        };
        if self.frame % HUD_PUSH_EVERY == 0 {
            self.hist.copy_within(1.., 0);
            self.hist[HUD_HIST - 1] = self.level;
        }
    }

    pub(crate) fn render(&self, px: &mut [u32], mode: u8) {
        for p in px.iter_mut() {
            *p = 0;
        }
        // boot: quick flicker-in when the channel opens, then steady
        let boot = smoothstep((self.t / HUD_BOOT).min(1.0))
            * if self.t < HUD_BOOT {
                0.55 + 0.45 * (self.t * 90.0).sin().abs()
            } else {
                1.0
            };

        // chamfered plate + scanlines (kept clear of the corner cuts)
        fill_chamfer_rect(
            px,
            OW,
            OH,
            HUD_CUT,
            premul(HUD_BG.0, HUD_BG.1, HUD_BG.2, HUD_BG.3),
        );
        for y in (5..OH - 4).step_by(3) {
            let inset = (HUD_CUT + 1 - y).max(HUD_CUT + 1 - (OH - 1 - y)).max(4);
            fill_rect(
                px,
                inset,
                y,
                OW - 2 * inset,
                1,
                premul(HUD_SCAN.0, HUD_SCAN.1, HUD_SCAN.2, HUD_SCAN.3),
            );
        }

        // frame: glowing top/bottom edge lines + bright diagonal accents along the chamfers
        let f = HUD_CUT + 3;
        hud_rect(px, f, 2, OW - 2 * f, 1, HUD_EDGE, 0.40 * boot);
        hud_rect(px, f, OH - 3, OW - 2 * f, 1, HUD_EDGE, 0.22 * boot);
        hud_rect(px, 2, f, 1, OH - 2 * f, HUD_DIM, 0.30 * boot);
        hud_rect(px, OW - 3, f, 1, OH - 2 * f, HUD_DIM, 0.30 * boot);
        let dlen = ((f + 1) as f32 * boot) as i32;
        for i in 0..=dlen.min(f) {
            let pts = [
                (i, f - i, 1),
                (OW - 1 - i, f - i, 1),
                (i, OH - 1 - (f - i), -1),
                (OW - 1 - i, OH - 1 - (f - i), -1),
            ];
            for (x, y, inward) in pts {
                if (0..OW).contains(&x) && (0..OH).contains(&y) {
                    blend_add(
                        &mut px[(y * OW + x) as usize],
                        HUD_EDGE.0,
                        HUD_EDGE.1,
                        HUD_EDGE.2,
                        0.9 * boot,
                    );
                    let y2 = y + inward;
                    if (0..OH).contains(&y2) {
                        blend_add(
                            &mut px[(y2 * OW + x) as usize],
                            HUD_EDGE.0,
                            HUD_EDGE.1,
                            HUD_EDGE.2,
                            0.30 * boot,
                        );
                    }
                }
            }
        }

        // header: blinking transmit dot + label, mode callout right-aligned
        let dot_on = (self.t % HUD_BLINK) < HUD_BLINK * 0.55;
        if dot_on {
            hud_rect(px, 14, 10, 4, 4, HUD_AMBER, 0.95 * boot);
            hud_rect(px, 13, 9, 6, 6, HUD_AMBER, 0.25 * boot); // soft halo
        } else {
            hud_rect(px, 14, 10, 4, 4, HUD_AMBER, 0.25 * boot);
        }
        let flicker = 0.82 + 0.10 * (self.t * 7.3).sin();
        let after = draw_text(px, 24, 9, "TRANSMITTING", HUD_TEXT, flicker * boot);
        // transmission timecode — the mission-control touch
        let secs = self.t as u32;
        let tc = format!("{:02}:{:02}", (secs / 60).min(99), secs % 60);
        draw_text(px, after + 8, 9, &tc, HUD_DIM, 0.9 * boot);
        let label = mode_label(mode);
        draw_text(
            px,
            OW - 16 - text_width(label),
            9,
            label,
            HUD_WHITE,
            0.95 * boot,
        );

        // AI-core orb: breathing focal point left of the waveform
        hud_orb(px, self.level, self.t, boot);

        // voiceprint: smooth mirrored envelope, newest at the right — a bright contour over a
        // translucent fill (assistant-style), not blocky bars
        hud_rect(
            px,
            HUD_WAVE_X,
            HUD_WAVE_Y,
            HUD_WAVE_W,
            1,
            HUD_DIM,
            0.30 * boot,
        );
        for sx in 0..HUD_WAVE_W {
            let fpos = sx as f32 / (HUD_WAVE_W - 1) as f32 * (HUD_HIST - 1) as f32;
            let i0 = (fpos as usize).min(HUD_HIST - 2);
            let frac = fpos - i0 as f32;
            let h01 = self.hist[i0] * (1.0 - frac) + self.hist[i0 + 1] * frac;
            let recency = fpos / (HUD_HIST - 1) as f32;
            let bright = 0.25 + 0.75 * recency * recency; // the fresh edge pops
                                                          // true silence collapses to the thin baseline instead of a chunky 3 px band
            if h01 < 0.03 {
                continue;
            }
            // .max AFTER the cast: a float floor below 1.0 would truncate to 0 (invisible)
            let hi = ((h01 * HUD_WAVE_MAX) as i32).max(1);
            let x = HUD_WAVE_X + sx;
            let c = (
                lerp_u8(HUD_DIM.0, HUD_EDGE.0, bright),
                lerp_u8(HUD_DIM.1, HUD_EDGE.1, bright),
                lerp_u8(HUD_DIM.2, HUD_EDGE.2, bright),
            );
            hud_rect(
                px,
                x,
                HUD_WAVE_Y - hi,
                1,
                hi * 2 + 1,
                c,
                (0.09 + 0.15 * bright) * boot,
            );
            for s in [-1i32, 1] {
                let ye = HUD_WAVE_Y + s * hi;
                if (0..OH).contains(&ye) {
                    blend_add(
                        &mut px[(ye * OW + x) as usize],
                        c.0,
                        c.1,
                        c.2,
                        (0.50 + 0.45 * bright) * boot,
                    );
                }
            }
            // white-hot caps on loud fresh syllables
            if recency > 0.85 && h01 > 0.60 {
                for s in [-1i32, 1] {
                    let yc = HUD_WAVE_Y + s * (hi + 1);
                    if (0..OH).contains(&yc) {
                        blend_add(
                            &mut px[(yc * OW + x) as usize],
                            HUD_WHITE.0,
                            HUD_WHITE.1,
                            HUD_WHITE.2,
                            0.85 * boot,
                        );
                    }
                }
            }
        }

        // radar sweep: a faint vertical line drifting across the voiceprint
        let sweep_x = HUD_WAVE_X + ((self.t * 80.0) as i32 % HUD_WAVE_W);
        hud_rect(
            px,
            sweep_x,
            HUD_WAVE_Y - HUD_WAVE_MAX as i32 - 2,
            1,
            HUD_WAVE_MAX as i32 * 2 + 5,
            HUD_WHITE,
            0.10 * boot,
        );

        // footer: link tag left, live signal meter right
        draw_text(px, 16, OH - 15, "LINK 01 . SECURE", HUD_DIM, 0.8 * boot);
        for i in 0..5i32 {
            let lit = self.level > (i as f32 + 0.5) / 5.0;
            let bh = 3 + i * 2;
            hud_rect(
                px,
                OW - 16 - (5 - i) * 4,
                OH - 10 - bh,
                2,
                bh,
                if lit { HUD_EDGE } else { HUD_DIM },
                if lit { 0.9 } else { 0.35 } * boot,
            );
        }
    }
}
