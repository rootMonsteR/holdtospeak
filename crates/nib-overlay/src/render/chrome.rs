//! The status row every non-HUD theme wears.
//!
//! HUD reads as designed because it is *informative*: it says it is transmitting, which cleanup
//! mode is active, and how strong the signal is. Bars, Wave and Volt were decoration only —
//! nothing on screen told you the mode, or even that the mic was being heard, so switching theme
//! meant giving up information.
//!
//! This module is the shared answer: the same three facts, drawn identically in every theme. The
//! rule the themes follow is that picking one changes how the overlay LOOKS, never what it TELLS
//! you. Each theme keeps its own artwork below [`ROW_H`].

use crate::font::draw_text;
use crate::mode::{mode_colors, mode_label};
use crate::paint::{blend_add, mix_rgb};
use crate::{OH, OW};

/// Vertical space the status row occupies. Themes draw their artwork below this so the two can
/// never collide, whatever the level does.
pub(crate) const ROW_H: i32 = 17;

const PAD_X: i32 = 15;
const TEXT_Y: i32 = 5;
const DOT_R: f32 = 2.6;

/// Segments in the right-hand input meter.
const SEGS: i32 = 5;
const SEG_W: i32 = 3;
const SEG_GAP: i32 = 2;

/// Draw the shared status row.
///
/// `level` (0..1) is overall input strength and drives both the live dot's brightness and the
/// segment meter. A dot that sat at constant brightness read as "frozen" on a silent mic, so it
/// tracks the signal instead — the overlay should look alive only when it actually hears you.
pub(crate) fn status_row(px: &mut [u32], mode: u8, level: f32) {
    let (c0, c1) = mode_colors(mode);
    let level = level.clamp(0.0, 1.0);

    // ---- live dot ----------------------------------------------------------------------------
    let (cx, cy) = (PAD_X as f32, (TEXT_Y + 2) as f32);
    let bright = 0.40 + 0.60 * level;
    let r = DOT_R.ceil() as i32 + 1;
    for dy in -r..=r {
        for dx in -r..=r {
            let d = ((dx * dx + dy * dy) as f32).sqrt();
            // Soft edge rather than a hard circle: at this size aliasing is very visible.
            let f = (1.0 - (d - DOT_R + 0.8) / 1.6).clamp(0.0, 1.0);
            if f <= 0.0 {
                continue;
            }
            let (x, y) = ((cx + dx as f32) as i32, (cy + dy as f32) as i32);
            if (0..OW).contains(&x) && (0..OH).contains(&y) {
                blend_add(&mut px[(y * OW + x) as usize], c1.0, c1.1, c1.2, bright * f);
            }
        }
    }

    // ---- mode label --------------------------------------------------------------------------
    draw_text(px, PAD_X + 8, TEXT_Y, mode_label(mode), c1, 0.80);

    // ---- input meter, right-aligned ----------------------------------------------------------
    let total = SEGS * SEG_W + (SEGS - 1) * SEG_GAP;
    let x0 = OW - PAD_X - total;
    // `ceil` so any audible signal lights the first segment: a meter that stays dark while you
    // are clearly speaking is worse than one that is slightly generous.
    let lit = (level * SEGS as f32).ceil() as i32;
    for i in 0..SEGS {
        let h = 2 + i * 2;
        let x = x0 + i * (SEG_W + SEG_GAP);
        let y = TEXT_Y + 6 - h;
        let c = mix_rgb(c0, c1, i as f32 / (SEGS - 1) as f32);
        let a = if i < lit { 0.85 } else { 0.14 };
        for yy in y..y + h {
            for xx in x..x + SEG_W {
                if (0..OW).contains(&xx) && (0..OH).contains(&yy) {
                    blend_add(&mut px[(yy * OW + xx) as usize], c.0, c.1, c.2, a);
                }
            }
        }
    }
}
