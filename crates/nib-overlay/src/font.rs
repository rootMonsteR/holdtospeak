//! The built-in 5x6 pixel font (uppercase + digits) used by the Hud style's labels.

use crate::paint::blend_add;
use crate::{OH, OW};

/// 5x6 pixel font (uppercase + digits), 5 bits per row, top row in the highest bits.
pub(crate) fn glyph(c: char) -> u32 {
    const fn rows(r: [u32; 6]) -> u32 {
        (r[0] << 25) | (r[1] << 20) | (r[2] << 15) | (r[3] << 10) | (r[4] << 5) | r[5]
    }
    match c {
        'A' => rows([0b01110, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001]),
        'C' => rows([0b01111, 0b10000, 0b10000, 0b10000, 0b10000, 0b01111]),
        'E' => rows([0b11111, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111]),
        'G' => rows([0b01111, 0b10000, 0b10000, 0b10011, 0b10001, 0b01111]),
        'H' => rows([0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001]),
        'I' => rows([0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b11111]),
        'K' => rows([0b10001, 0b10010, 0b11100, 0b10010, 0b10001, 0b10001]),
        'L' => rows([0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111]),
        'M' => rows([0b10001, 0b11011, 0b10101, 0b10001, 0b10001, 0b10001]),
        'N' => rows([0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001]),
        'O' => rows([0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110]),
        'P' => rows([0b11110, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000]),
        'R' => rows([0b11110, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001]),
        'S' => rows([0b01111, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110]),
        'T' => rows([0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100]),
        'U' => rows([0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110]),
        'V' => rows([0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100]),
        'W' => rows([0b10001, 0b10001, 0b10001, 0b10101, 0b11011, 0b10001]),
        'X' => rows([0b10001, 0b01010, 0b00100, 0b00100, 0b01010, 0b10001]),
        '0' => rows([0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b01110]),
        '1' => rows([0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b01110]),
        '2' => rows([0b01110, 0b10001, 0b00010, 0b00100, 0b01000, 0b11111]),
        '3' => rows([0b11110, 0b00001, 0b00110, 0b00001, 0b00001, 0b11110]),
        '4' => rows([0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010]),
        '5' => rows([0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b11110]),
        '6' => rows([0b01110, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110]),
        '7' => rows([0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000]),
        '8' => rows([0b01110, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110]),
        '9' => rows([0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b01110]),
        ':' => rows([0b00000, 0b00100, 0b00000, 0b00000, 0b00100, 0b00000]),
        '.' => rows([0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00100]),
        _ => 0, // space and anything undefined render as a gap
    }
}

/// Draw a string; returns the x just past the last glyph. 6 px advance per character.
pub(crate) fn draw_text(
    px: &mut [u32],
    x: i32,
    y: i32,
    s: &str,
    color: (u8, u8, u8),
    alpha: f32,
) -> i32 {
    let mut cx = x;
    for c in s.chars() {
        let g = glyph(c);
        if g != 0 {
            for row in 0..6i32 {
                let bits = (g >> (25 - row * 5)) & 0b11111;
                for col in 0..5i32 {
                    if bits & (0b10000 >> col) != 0 {
                        let (gx, gy) = (cx + col, y + row);
                        if (0..OW).contains(&gx) && (0..OH).contains(&gy) {
                            blend_add(
                                &mut px[(gy * OW + gx) as usize],
                                color.0,
                                color.1,
                                color.2,
                                alpha,
                            );
                        }
                    }
                }
            }
        }
        cx += 6;
    }
    cx
}

pub(crate) fn text_width(s: &str) -> i32 {
    s.chars().count() as i32 * 6 - 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hud_strings_have_glyphs() {
        // every non-space character the Hud style renders must exist in the pixel font
        for s in [
            "TRANSMITTING",
            "RAW",
            "AUTO",
            "POLISH",
            "EMAIL",
            "LINK 01 . SECURE",
            "0123456789:",
        ] {
            for c in s.chars().filter(|&c| c != ' ') {
                assert_ne!(glyph(c), 0, "missing glyph for {c:?} in {s:?}");
            }
        }
    }
}
