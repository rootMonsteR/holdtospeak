//! Pixel primitives shared by every style: premultiplied-ARGB color math, saturating additive
//! blending, and the panel-shape fills. All of them address the frame buffer as `OW`-wide rows.

use crate::{OH, OW};

#[inline]
pub(crate) fn premul(r: u8, g: u8, b: u8, a: u8) -> u32 {
    let a32 = a as u32;
    let pr = r as u32 * a32 / 255;
    let pg = g as u32 * a32 / 255;
    let pb = b as u32 * a32 / 255;
    (a32 << 24) | (pr << 16) | (pg << 8) | pb
}

#[inline]
pub(crate) fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t) as u8
}

pub(crate) fn mix_rgb(a: (u8, u8, u8), b: (u8, u8, u8), t: f32) -> (u8, u8, u8) {
    (
        lerp_u8(a.0, b.0, t),
        lerp_u8(a.1, b.1, t),
        lerp_u8(a.2, b.2, t),
    )
}

#[inline]
pub(crate) fn smoothstep(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// Additive blend of a premultiplied pixel (saturating): overlapping glows brighten.
#[inline]
pub(crate) fn blend_add(p: &mut u32, r: u8, g: u8, b: u8, a: f32) {
    let ai = (a.clamp(0.0, 1.0) * 255.0) as u32;
    if ai == 0 {
        return;
    }
    let (sr, sg, sb) = (
        r as u32 * ai / 255,
        g as u32 * ai / 255,
        b as u32 * ai / 255,
    );
    blend_add_raw(p, ai, sr, sg, sb);
}

/// Additive blend of an already-premultiplied f32 contribution (each channel 0..255).
#[inline]
pub(crate) fn blend_add_pre(p: &mut u32, pr: f32, pg: f32, pb: f32, pa: f32) {
    blend_add_raw(p, pa as u32, pr as u32, pg as u32, pb as u32);
}

/// Shared saturating additive tail for premultiplied pixels (used by both blenders).
#[inline]
fn blend_add_raw(p: &mut u32, sa: u32, sr: u32, sg: u32, sb: u32) {
    let (da, dr, dg, db) = (*p >> 24, (*p >> 16) & 0xFF, (*p >> 8) & 0xFF, *p & 0xFF);
    *p = ((da + sa).min(255) << 24)
        | ((dr + sr).min(255) << 16)
        | ((dg + sg).min(255) << 8)
        | (db + sb).min(255);
}

pub(crate) fn fill_rect(px: &mut [u32], x: i32, y: i32, w: i32, h: i32, color: u32) {
    for yy in y.max(0)..(y + h).min(OH) {
        let row = yy * OW;
        for xx in x.max(0)..(x + w).min(OW) {
            px[(row + xx) as usize] = color;
        }
    }
}

pub(crate) fn fill_round_rect(px: &mut [u32], w: i32, h: i32, rad: i32, color: u32) {
    for yy in 0..h {
        for xx in 0..w {
            // a pixel is clipped when it falls in a corner square but outside that corner's arc
            let corner = |cx: i32, cy: i32| {
                let dx = cx - xx;
                let dy = cy - yy;
                dx * dx + dy * dy > rad * rad
            };
            let clipped = (xx < rad && yy < rad && corner(rad, rad))
                || (xx >= w - rad && yy < rad && corner(w - rad - 1, rad))
                || (xx < rad && yy >= h - rad && corner(rad, h - rad - 1))
                || (xx >= w - rad && yy >= h - rad && corner(w - rad - 1, h - rad - 1));
            if !clipped {
                px[(yy * OW + xx) as usize] = color;
            }
        }
    }
}

/// Filled rectangle with 45-degree chamfered corners — the sci-fi plate silhouette.
pub(crate) fn fill_chamfer_rect(px: &mut [u32], w: i32, h: i32, cut: i32, color: u32) {
    for y in 0..h {
        for x in 0..w {
            let inside = x + y >= cut
                && (w - 1 - x) + y >= cut
                && x + (h - 1 - y) >= cut
                && (w - 1 - x) + (h - 1 - y) >= cut;
            if inside {
                px[(y * OW + x) as usize] = color;
            }
        }
    }
}
