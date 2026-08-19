//! Dev harness: synthetic envelope + headless frame dump.
//!
//! Test-only: the always-on overlay always renders live `sampler` audio. These helpers drive the
//! deterministic headless PNG dump (`dump_frames`) that the render/PNG unit tests exercise, and
//! let an agent view and iterate on a style without a window, mic, or model.

use crate::{render_frame, Anim, OverlayStyle, DT, NBARS, OH, OW};

/// Scripted voice bands for preview/dump: silence -> sharp attack -> syllabic sustain -> decay,
/// looping every 4 s. Squared half-sine "syllables" give repeated onsets so forks keep firing.
fn synth_bands(t: f32, bands: &mut [f32; 3]) {
    let t = t % 4.0;
    let syl = |t: f32| (std::f32::consts::TAU * 2.6 * t).sin().max(0.0).powi(2);
    let (low, mid, high) = if t < 0.35 {
        (0.02, 0.015, 0.01)
    } else if t < 0.55 {
        let r = ((t - 0.35) / 0.20).powf(1.5);
        (0.85 * r, 0.70 * r, 0.55 * r)
    } else if t < 3.10 {
        (
            0.30 + 0.55 * syl(t),
            0.22 + 0.48 * syl(t + 0.07),
            0.10 + 0.50 * syl(t).powi(2) * (0.6 + 0.4 * (std::f32::consts::TAU * 0.9 * t).sin()),
        )
    } else if t < 3.70 {
        let d = (-(t - 3.10) * 6.0).exp();
        (0.55 * d, 0.45 * d, 0.30 * d)
    } else {
        (0.02, 0.015, 0.01)
    };
    bands[0] = low.clamp(0.0, 1.0);
    bands[1] = mid.clamp(0.0, 1.0);
    bands[2] = high.clamp(0.0, 1.0);
}

/// Bars preview: spread the 3 synthetic bands across the bars with per-bar wiggle.
fn synth_levels(t: f32, levels: &mut [f32; NBARS]) {
    let mut bands = [0f32; 3];
    synth_bands(t, &mut bands);
    for (i, l) in levels.iter_mut().enumerate() {
        let u = i as f32 / (NBARS - 1) as f32;
        let base = if u < 0.5 {
            bands[0] + (bands[1] - bands[0]) * (u * 2.0)
        } else {
            bands[1] + (bands[2] - bands[1]) * ((u - 0.5) * 2.0)
        };
        *l = (base * (0.85 + 0.15 * (t * 7.0 + i as f32 * 1.7).sin())).clamp(0.0, 1.0);
    }
}

const DUMP_FRAMES: u32 = 240; // full 4 s scripted loop @ 60 fps
const DUMP_EVERY: u32 = 8; // 30 spaced PNGs + an 11-frame consecutive motion window = 41 files
const DUMP_GAP: i32 = 4; // divider rows between the two composites

/// Premultiplied source-over onto an opaque background color.
fn over_bg(p: u32, bg: (u32, u32, u32)) -> u32 {
    let a = p >> 24;
    let inv = 255 - a;
    let r = (((p >> 16) & 0xFF) + bg.0 * inv / 255).min(255);
    let g = (((p >> 8) & 0xFF) + bg.1 * inv / 255).min(255);
    let b = ((p & 0xFF) + bg.2 * inv / 255).min(255);
    0xFF00_0000 | (r << 16) | (g << 8) | b
}

/// One frame composited over dark (#1e1e1e, top) and light (#f0f0f0, bottom) backgrounds.
fn composite_over(src: &[u32], out: &mut [u32]) {
    for (i, &p) in src.iter().enumerate() {
        out[i] = over_bg(p, (0x1e, 0x1e, 0x1e));
        out[i + ((OH + DUMP_GAP) * OW) as usize] = over_bg(p, (0xf0, 0xf0, 0xf0));
    }
    for g in 0..DUMP_GAP {
        for x in 0..OW {
            out[((OH + g) * OW + x) as usize] = 0xFF80_8080;
        }
    }
}

fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (i, entry) in table.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
        *entry = c;
    }
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c = table[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    c ^ 0xFFFF_FFFF
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in data {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

fn png_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc_in = Vec::with_capacity(4 + data.len());
    crc_in.extend_from_slice(kind);
    crc_in.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_in).to_be_bytes());
}

/// Minimal PNG writer — 8-bit truecolor RGB, zlib stream of *stored* (uncompressed) deflate
/// blocks, hand-rolled CRC32/Adler32. No crates; universally readable, unlike BMP.
fn write_png(path: &std::path::Path, w: i32, h: i32, px: &[u32]) -> std::io::Result<()> {
    // raw scanlines: filter byte 0 + RGB per pixel
    let mut raw = Vec::with_capacity((h * (1 + w * 3)) as usize);
    for y in 0..h {
        raw.push(0u8);
        for x in 0..w {
            let p = px[(y * w + x) as usize];
            raw.push((p >> 16) as u8);
            raw.push((p >> 8) as u8);
            raw.push(p as u8);
        }
    }
    // zlib wrapper around stored deflate blocks (max 65535 bytes each)
    let mut z = Vec::with_capacity(raw.len() + raw.len() / 65535 * 5 + 16);
    z.extend_from_slice(&[0x78, 0x01]);
    let mut off = 0;
    while off < raw.len() {
        let n = (raw.len() - off).min(65535);
        let last = off + n == raw.len();
        z.push(u8::from(last));
        z.extend_from_slice(&(n as u16).to_le_bytes());
        z.extend_from_slice(&(!(n as u16)).to_le_bytes());
        z.extend_from_slice(&raw[off..off + n]);
        off += n;
    }
    z.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&(w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit, truecolor, deflate, no filter, no interlace

    let mut out = Vec::with_capacity(z.len() + 100);
    out.extend_from_slice(&[137, 80, 78, 71, 13, 10, 26, 10]);
    png_chunk(&mut out, b"IHDR", &ihdr);
    png_chunk(&mut out, b"IDAT", &z);
    png_chunk(&mut out, b"IEND", &[]);
    std::fs::write(path, out)
}

/// Headless render of the scripted loop to PNG frames — lets an agent view and iterate on a
/// style without a window, mic, or model. `mode` (0=Raw..3=Email) selects the theme accent.
fn dump_frames(dir: &std::path::Path, style: OverlayStyle, mode: u8) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let mut px = vec![0u32; (OW * OH) as usize];
    let mut out = vec![0u32; (OW * (OH * 2 + DUMP_GAP)) as usize];
    let mut anim = Anim::new();
    for f in 0..DUMP_FRAMES {
        if style.needs_levels() {
            synth_levels(anim.t, &mut anim.levels);
        } else {
            synth_bands(anim.t, &mut anim.bands);
        }
        anim.tick();
        render_frame(&mut px, style, &mut anim, mode);
        if f % DUMP_EVERY == 0 || (96..108).contains(&f) {
            // frames every DUMP_EVERY for coverage, plus a consecutive window for judging
            // MOTION (fork attachment, strobing) frame to frame
            composite_over(&px, &mut out);
            write_png(
                &dir.join(format!("frame_{f:03}.png")),
                OW,
                OH * 2 + DUMP_GAP,
                &out,
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synth_bands_shape() {
        let mut bands = [0f32; 3];
        let mut peak = 0f32;
        for f in 0..240 {
            synth_bands(f as f32 * DT, &mut bands);
            for &b in &bands {
                assert!((0.0..=1.0).contains(&b), "band out of range at frame {f}");
            }
            peak = peak.max(bands[0]);
        }
        assert!(peak > 0.5, "attack/sustain never got loud");
        synth_bands(0.1, &mut bands); // silence phase
        assert!(bands[0] < 0.05);
    }

    #[test]
    fn png_layout() {
        let path = std::env::temp_dir().join(format!("nib_png_test_{}.png", std::process::id()));
        write_png(&path, 2, 2, &[0xFF12_3456; 4]).unwrap();
        let data = std::fs::read(&path).unwrap();
        assert_eq!(&data[0..8], &[137, 80, 78, 71, 13, 10, 26, 10]);
        assert_eq!(&data[12..16], b"IHDR");
        assert_eq!(
            u32::from_be_bytes([data[16], data[17], data[18], data[19]]),
            2
        ); // width
        assert_eq!(
            u32::from_be_bytes([data[20], data[21], data[22], data[23]]),
            2
        ); // height
        assert_eq!(&data[data.len() - 8..data.len() - 4], b"IEND");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn dump_frames_writes_png() {
        // Exercises the full headless pipeline (synth -> render_frame for every style ->
        // composite -> write_png) for both input shapes: Bars drives per-bar levels, Hud the
        // 3 voice bands. Deterministic, so it doubles as a render smoke test.
        let base = std::env::temp_dir().join(format!("nib_overlay_dump_{}", std::process::id()));
        for (style, mode) in [(OverlayStyle::Bars, 0u8), (OverlayStyle::Hud, 3u8)] {
            let dir = base.join(format!("{style:?}"));
            dump_frames(&dir, style, mode).unwrap();
            assert!(
                dir.join("frame_000.png").exists(),
                "no first frame for {style:?}"
            );
        }
        let _ = std::fs::remove_dir_all(&base);
    }
}
