//! Volt style — a crackling electric-blue energy beam: a voice-driven base waveform + three
//! octaves of keyframe-eased value noise (coarse path drifts ~150 ms, fine detail crackles
//! ~33 ms — that fast-fine/slow-coarse split is what reads as electricity), onset-triggered
//! lightning forks, spark particles, and a 3-pass bloom (deep-blue halo, cyan glow, white-hot
//! core). A thin mode-colored accent line under the beam keeps the active-mode signal.
//!
//! All tunables live in the const block below — visual iteration is editing here + re-dumping
//! frames. Perf: ~50k lightweight pixel blends/frame, <0.5 ms — no optimization needed at 60 fps.

use crate::mode::mode_colors;
use crate::paint::{blend_add, blend_add_pre, fill_round_rect, mix_rgb, premul, smoothstep};
use crate::rng::Rng;
use crate::{DT, OH, OW};

const VOLT_PAD: i32 = 18; // beam endpoint inset from panel edges
const VOLT_COLS: usize = (OW - 2 * VOLT_PAD) as usize;

// voice-driven base path (low-frequency waveform under the jitter)
const VOLT_VOICE_AMP: f32 = 12.0; // px @ full low-band energy
const VOLT_VOICE_CYCLES: f32 = 1.4;
const VOLT_VOICE_AMP2: f32 = 6.0; // secondary mid-band sine
const VOLT_VOICE_CYCLES2: f32 = 2.9;
const VOLT_IDLE_VOICE: f32 = 0.10; // amplitude floor while silent

// fractal jitter: 3 octaves of value noise, keyframe-eased
const JIT_OCTAVES: usize = 3;
const JIT_PTS: [usize; JIT_OCTAVES] = [9, 17, 33]; // lattice points per octave
const JIT_AMP: [f32; JIT_OCTAVES] = [9.0, 5.0, 3.5]; // px — crackle needs the fine octave loud
const JIT_FRAMES: [f32; JIT_OCTAVES] = [9.0, 5.0, 2.0]; // re-roll period: ~150/83/33 ms
const JIT_IDLE: f32 = 0.22; // jitter scale floor while silent
const JIT_MAX_PTS: usize = 33;

// energy + onset detection
const ONSET_GAIN: f32 = 9.0;
const ONSET_DECAY: f32 = 0.86; // per-frame (~100 ms tail)
const VOLT_IDLE_THRESH: f32 = 0.06; // below = idle: no branches or sparks

// branches (lightning forks)
const MAX_BRANCHES: usize = 5;
const BRANCH_SEGS: usize = 7;
const BRANCH_LIFE: (f32, f32) = (0.15, 0.40); // seconds
const BRANCH_LEN: (f32, f32) = (24.0, 60.0); // px, scaled by energy
const BRANCH_P_ONSET: f32 = 0.70; // spawn prob/frame @ full onset
const BRANCH_P_ENERGY: f32 = 0.10; // spawn prob/frame @ full sustained energy
const BRANCH_ALPHA: f32 = 0.85;
const BRANCH_GROW: f32 = 0.30; // first fraction of life spent extending
const BRANCH_SCALE: f32 = 1.05; // bloom radius multiplier vs the beam — forks need real aura

// sparks
const MAX_SPARKS: usize = 12;
const SPARK_LIFE: (f32, f32) = (0.12, 0.25);
const SPARK_P: f32 = 0.12; // base prob/frame while speaking
const SPARK_P_ONSET: f32 = 0.80;
const SPARK_VY: (f32, f32) = (30.0, 90.0); // px/s ejection away from midline
const SPARK_DRAG: f32 = 0.96;

// 3-pass bloom (quadratic falloff for halo/mid, linear for the hot core)
const HALO_R: f32 = 10.0;
const HALO_A: f32 = 0.45;
const MID_R: f32 = 3.5;
const MID_A: f32 = 0.85;
const CORE_R: f32 = 1.2;
const CORE_A: f32 = 0.95;
const GHOST_ALPHA: f32 = 0.22; // trailing afterimage arc (depth, hugs the beam)

// arc re-strike: a strong onset snaps the whole coarse path to a new configuration —
// real arcs jump between discharge paths rather than only wobbling
const STRIKE_THRESH: f32 = 0.55;
const STRIKE_COOLDOWN: f32 = 0.25; // s

// palette: the signature electric blue (user decision: always blue; mode shown via accent line)
const VOLT_TINT: (u8, u8, u8) = (64, 140, 255);
const VOLT_BG: (u8, u8, u8, u8) = (8, 10, 18, 150); // backdrop panel
const VOLT_IDLE_INT: f32 = 0.65; // beam intensity floor
const VOLT_SHIMMER: f32 = 0.08; // slow idle shimmer amplitude
const ACCENT_Y: i32 = OH - 8; // mode-colored accent line position
const ACCENT_ALPHA: f32 = 0.50;

#[derive(Clone, Copy, Default)]
struct Branch {
    life: f32,
    total: f32, // life <= 0 => slot free
    // Root column: the root Y is read from the LIVE path every frame so the fork stays
    // attached to the beam as it moves — a frozen root visibly detaches within one lifetime.
    col: usize,
    dx: f32,
    dy: f32,                 // px per segment
    wob: [f32; BRANCH_SEGS], // cumulative random-walk lateral offsets (jagged discharge)
}

#[derive(Clone, Copy, Default)]
struct Spark {
    life: f32,
    total: f32,
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
}

struct VoltPalette {
    outer: (u8, u8, u8),
    mid: (u8, u8, u8),
    core: (u8, u8, u8),
}

fn volt_palette() -> VoltPalette {
    VoltPalette {
        outer: mix_rgb(VOLT_TINT, (0, 0, 40), 0.30), // deep saturated blue aura
        mid: mix_rgb(VOLT_TINT, (255, 255, 255), 0.18), // stays visibly blue, not washed out
        core: mix_rgb(VOLT_TINT, (255, 255, 255), 0.88),
    }
}

/// Halo + glow + core in a single vertical scan: per pixel, sum the three weighted
/// premultiplied contributions and blend once.
fn volt_column(px: &mut [u32], x: i32, y: f32, pal: &VoltPalette, intensity: f32, scale: f32) {
    if !(0..OW).contains(&x) {
        return;
    }
    let (sh, sm, sc) = (HALO_R * scale, MID_R * scale, CORE_R * scale);
    let reach = sh.ceil() as i32;
    let yc = y as i32;
    for dy in -reach..=reach {
        let py = yc + dy;
        if !(0..OH).contains(&py) {
            continue;
        }
        let d = ((py as f32 + 0.5) - y).abs();
        let wh = HALO_A * (1.0 - d / sh).max(0.0).powi(2);
        let wm = MID_A * (1.0 - d / sm).max(0.0).powi(2);
        let wc = CORE_A * (1.0 - d / sc).max(0.0);
        let wsum = wh + wm + wc;
        if wsum <= 0.004 {
            continue;
        }
        // Normalize when the weights over-sum so channel <= alpha always holds — premultiplied
        // pixels violating that invariant over-brighten on light backgrounds (white fringing).
        let wn = intensity * (wsum.min(1.0) / wsum);
        let pr = (pal.outer.0 as f32 * wh + pal.mid.0 as f32 * wm + pal.core.0 as f32 * wc) * wn;
        let pg = (pal.outer.1 as f32 * wh + pal.mid.1 as f32 * wm + pal.core.1 as f32 * wc) * wn;
        let pb = (pal.outer.2 as f32 * wh + pal.mid.2 as f32 * wm + pal.core.2 as f32 * wc) * wn;
        let pa = wsum.min(1.0) * intensity * 255.0;
        blend_add_pre(
            &mut px[(py * OW + x) as usize],
            pr.min(255.0),
            pg.min(255.0),
            pb.min(255.0),
            pa.min(255.0),
        );
    }
}

pub(crate) struct VoltState {
    rng: Rng,
    t: f32,
    phase: f32,
    jit_a: [[f32; JIT_MAX_PTS]; JIT_OCTAVES],
    jit_b: [[f32; JIT_MAX_PTS]; JIT_OCTAVES],
    jit_u: [f32; JIT_OCTAVES],
    path: [f32; VOLT_COLS],
    ghost: [f32; VOLT_COLS], // slow-follow copy of the path — drawn as a faint afterimage
    energy: f32,
    onset: f32,
    prev_energy: f32,
    last_strike: f32,
    branches: [Branch; MAX_BRANCHES],
    sparks: [Spark; MAX_SPARKS],
}

impl VoltState {
    pub(crate) fn new() -> Self {
        let mut rng = Rng::new(0x9E37_79B9_7F4A_7C15);
        let mut jit_a = [[0f32; JIT_MAX_PTS]; JIT_OCTAVES];
        let mut jit_b = [[0f32; JIT_MAX_PTS]; JIT_OCTAVES];
        for o in 0..JIT_OCTAVES {
            for i in 0..JIT_PTS[o] {
                jit_a[o][i] = rng.signed();
                jit_b[o][i] = rng.signed();
            }
        }
        VoltState {
            rng,
            t: 0.0,
            phase: 0.0,
            jit_a,
            jit_b,
            jit_u: [0.0; JIT_OCTAVES],
            path: [OH as f32 / 2.0; VOLT_COLS],
            ghost: [OH as f32 / 2.0; VOLT_COLS],
            energy: 0.0,
            onset: 0.0,
            prev_energy: 0.0,
            last_strike: -1.0,
            branches: [Branch::default(); MAX_BRANCHES],
            sparks: [Spark::default(); MAX_SPARKS],
        }
    }

    /// Advance one DT frame: energy/onset, jitter easing, path rebuild, branch/spark lifecycle.
    pub(crate) fn step(&mut self, bands: &[f32; 3]) {
        self.t += DT;
        let energy = (bands[0] + bands[1] + bands[2]) / 3.0;
        self.onset = (self.onset * ONSET_DECAY)
            .max(((energy - self.prev_energy) * ONSET_GAIN).clamp(0.0, 1.0));
        self.prev_energy = energy;
        self.energy = energy;
        self.phase += 0.10 + 0.22 * energy;

        // arc re-strike: snap the coarse octave to a brand-new path on a strong onset
        if self.onset > STRIKE_THRESH
            && energy > VOLT_IDLE_THRESH
            && self.t - self.last_strike > STRIKE_COOLDOWN
        {
            self.last_strike = self.t;
            for i in 0..JIT_PTS[0] {
                self.jit_a[0][i] = self.rng.signed();
                self.jit_b[0][i] = self.rng.signed();
            }
            self.jit_u[0] = 0.0;
        }

        // advance the jitter keyframes and compute eased lattice values
        let mut eased = [[0f32; JIT_MAX_PTS]; JIT_OCTAVES];
        for o in 0..JIT_OCTAVES {
            self.jit_u[o] += 1.0 / JIT_FRAMES[o];
            if self.jit_u[o] >= 1.0 {
                self.jit_u[o] -= 1.0;
                for i in 0..JIT_PTS[o] {
                    self.jit_a[o][i] = self.jit_b[o][i];
                    self.jit_b[o][i] = self.rng.signed();
                }
            }
            let s = smoothstep(self.jit_u[o]);
            for i in 0..JIT_PTS[o] {
                eased[o][i] = self.jit_a[o][i] + (self.jit_b[o][i] - self.jit_a[o][i]) * s;
            }
        }

        // rebuild the beam path
        let cy = OH as f32 / 2.0;
        let scale_coarse = JIT_IDLE + (1.0 - JIT_IDLE) * energy;
        let scale_fine =
            JIT_IDLE + (1.0 - JIT_IDLE) * (0.6 * energy + 1.2 * self.onset + bands[2]).min(1.0);
        for i in 0..VOLT_COLS {
            let u = i as f32 / (VOLT_COLS - 1) as f32;
            let env = (std::f32::consts::PI * u).sin();
            let voice = (VOLT_IDLE_VOICE + bands[0]).min(1.0)
                * VOLT_VOICE_AMP
                * (std::f32::consts::TAU * VOLT_VOICE_CYCLES * u + self.phase).sin()
                + (VOLT_IDLE_VOICE + bands[1]).min(1.0)
                    * VOLT_VOICE_AMP2
                    * (std::f32::consts::TAU * VOLT_VOICE_CYCLES2 * u - 1.6 * self.phase).sin();
            let mut noise = 0.0;
            for o in 0..JIT_OCTAVES {
                let scale = if o == JIT_OCTAVES - 1 {
                    scale_fine
                } else {
                    scale_coarse
                };
                let p = u * (JIT_PTS[o] - 1) as f32;
                let i0 = (p as usize).min(JIT_PTS[o] - 2);
                let v =
                    eased[o][i0] + (eased[o][i0 + 1] - eased[o][i0]) * smoothstep(p - i0 as f32);
                noise += JIT_AMP[o] * scale * v;
            }
            self.path[i] = cy + env * (voice + noise);
            self.ghost[i] = self.ghost[i] * 0.55 + self.path[i] * 0.45; // tight ~1-frame lag
        }

        // branches + sparks (only while actually speaking)
        for b in self.branches.iter_mut() {
            b.life -= DT;
        }
        for s in self.sparks.iter_mut() {
            s.life -= DT;
            s.x += s.vx * DT;
            s.y += s.vy * DT;
            s.vx *= SPARK_DRAG;
            s.vy *= SPARK_DRAG;
        }
        if energy > VOLT_IDLE_THRESH {
            if self.rng.next_f32() < BRANCH_P_ONSET * self.onset + BRANCH_P_ENERGY * energy {
                self.spawn_branch();
            }
            if self.rng.next_f32() < SPARK_P * energy + SPARK_P_ONSET * self.onset {
                self.spawn_spark();
            }
        }
    }

    /// Pick a high-displacement column (forks discharge off peaks) — best of 4 random samples.
    fn peak_column(&mut self) -> usize {
        let cy = OH as f32 / 2.0;
        let mut best = 0usize; // always overwritten: best_d starts below any real distance
        let mut best_d = -1.0;
        for _ in 0..4 {
            let c = (self.rng.next_f32() * (VOLT_COLS - 1) as f32) as usize;
            let d = (self.path[c] - cy).abs();
            if d > best_d {
                best_d = d;
                best = c;
            }
        }
        best
    }

    fn spawn_branch(&mut self) {
        let col = self.peak_column();
        let cy = OH as f32 / 2.0;
        let y0 = self.path[col];
        let away = if (y0 - cy).abs() < 0.5 {
            if self.rng.next_f32() < 0.5 {
                -1.0
            } else {
                1.0
            }
        } else {
            (y0 - cy).signum()
        };
        let len = self.rng.range(BRANCH_LEN.0, BRANCH_LEN.1) * (0.5 + 0.5 * self.energy);
        let seg = len * 0.7 / BRANCH_SEGS as f32;
        // cumulative random walk = a jagged lightning stroke, not a square zigzag
        let mut wob = [0f32; BRANCH_SEGS];
        let mut acc = 0f32;
        for w in wob.iter_mut().skip(1) {
            acc += self.rng.signed() * 4.0;
            *w = acc;
        }
        let total = self.rng.range(BRANCH_LIFE.0, BRANCH_LIFE.1);
        if let Some(b) = self.branches.iter_mut().find(|b| b.life <= 0.0) {
            *b = Branch {
                life: total,
                total,
                col,
                dx: self.rng.signed() * seg * 1.1, // diagonal lean — forks shouldn't all stand straight
                dy: away * seg,
                wob,
            };
        }
    }

    fn spawn_spark(&mut self) {
        let col = self.peak_column();
        let cy = OH as f32 / 2.0;
        let y0 = self.path[col];
        let away = if y0 < cy { -1.0 } else { 1.0 };
        let total = self.rng.range(SPARK_LIFE.0, SPARK_LIFE.1);
        if let Some(s) = self.sparks.iter_mut().find(|s| s.life <= 0.0) {
            *s = Spark {
                life: total,
                total,
                x: (VOLT_PAD + col as i32) as f32,
                y: y0,
                vx: self.rng.signed() * 40.0,
                vy: away * self.rng.range(SPARK_VY.0, SPARK_VY.1),
            };
        }
    }

    pub(crate) fn render(&self, px: &mut [u32], mode: u8) {
        for p in px.iter_mut() {
            *p = 0;
        }
        fill_round_rect(
            px,
            OW,
            OH,
            20,
            premul(VOLT_BG.0, VOLT_BG.1, VOLT_BG.2, VOLT_BG.3),
        );
        let pal = volt_palette();

        // mode accent: a thin glowing underline in the active mode's bright color
        let (_, accent) = mode_colors(mode);
        for x in VOLT_PAD..(OW - VOLT_PAD) {
            let u = (x - VOLT_PAD) as f32 / (VOLT_COLS - 1) as f32;
            let env = (std::f32::consts::PI * u).sin().sqrt(); // long plateau, soft ends
            for dy in -1i32..=1 {
                let a = ACCENT_ALPHA * env * if dy == 0 { 1.0 } else { 0.30 };
                blend_add(
                    &mut px[((ACCENT_Y + dy) * OW + x) as usize],
                    accent.0,
                    accent.1,
                    accent.2,
                    a,
                );
            }
        }

        // ghost afterimage first (faint, slightly wider), then the live beam over it
        let base_int = (VOLT_IDLE_INT + 0.45 * self.energy + 0.35 * self.onset).min(1.35);
        for (i, &y) in self.ghost.iter().enumerate() {
            let u = i as f32 / (VOLT_COLS - 1) as f32;
            let env = (std::f32::consts::PI * u).sin();
            volt_column(
                px,
                VOLT_PAD + i as i32,
                y,
                &pal,
                base_int * GHOST_ALPHA * (0.35 + 0.65 * env),
                1.15,
            );
        }
        for (i, &y) in self.path.iter().enumerate() {
            let u = i as f32 / (VOLT_COLS - 1) as f32;
            let env = (std::f32::consts::PI * u).sin();
            let shimmer = 1.0 + VOLT_SHIMMER * (self.t * 2.3 + u * 4.0).sin();
            volt_column(
                px,
                VOLT_PAD + i as i32,
                y,
                &pal,
                base_int * (0.35 + 0.65 * env) * shimmer,
                1.0,
            );
        }

        // lightning forks: polyline joints grown over the first BRANCH_GROW of life
        for b in self.branches.iter().filter(|b| b.life > 0.0) {
            let age = b.total - b.life;
            let grow = (age / (b.total * BRANCH_GROW)).min(1.0);
            let fade = (b.life / (b.total * 0.5)).min(1.0) * BRANCH_ALPHA;
            let segs_visible = grow * (BRANCH_SEGS - 1) as f32;
            for j in 0..(BRANCH_SEGS - 1) {
                if (j as f32) >= segs_visible {
                    break;
                }
                // root follows the live beam: sample today's path at the stored column
                let x0 = (VOLT_PAD + b.col as i32) as f32;
                let y0 = self.path[b.col.min(VOLT_COLS - 1)];
                let (x1, y1) = (x0 + j as f32 * b.dx + b.wob[j], y0 + j as f32 * b.dy);
                let (x2, y2) = (
                    x0 + (j + 1) as f32 * b.dx + b.wob[j + 1],
                    y0 + (j + 1) as f32 * b.dy,
                );
                // taper: thick + bright at the root, thin toward the tip
                let along = j as f32 / (BRANCH_SEGS - 1) as f32;
                let scale = BRANCH_SCALE * (1.0 - 0.45 * along);
                let steps = (x2 - x1).abs().max((y2 - y1).abs()).ceil().max(1.0) as i32;
                for s in 0..=steps {
                    let t = s as f32 / steps as f32;
                    volt_column(
                        px,
                        (x1 + (x2 - x1) * t) as i32,
                        y1 + (y2 - y1) * t,
                        &pal,
                        fade * (1.2 - 0.5 * along),
                        scale,
                    );
                }
            }
        }

        // sparks: tiny hot particles
        for s in self.sparks.iter().filter(|s| s.life > 0.0) {
            let fade = (s.life / s.total).min(1.0);
            let (xi, yi) = (s.x as i32, s.y as i32);
            if (0..OW).contains(&xi) && (0..OH).contains(&yi) {
                blend_add(
                    &mut px[(yi * OW + xi) as usize],
                    pal.core.0,
                    pal.core.1,
                    pal.core.2,
                    fade,
                );
                for (nx, ny) in [(xi - 1, yi), (xi + 1, yi), (xi, yi - 1), (xi, yi + 1)] {
                    if (0..OW).contains(&nx) && (0..OH).contains(&ny) {
                        blend_add(
                            &mut px[(ny * OW + nx) as usize],
                            pal.mid.0,
                            pal.mid.1,
                            pal.mid.2,
                            0.4 * fade,
                        );
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volt_path_stays_in_panel() {
        let mut vs = VoltState::new();
        let bands = [1.0f32; 3];
        for _ in 0..300 {
            vs.step(&bands);
            for &y in vs.path.iter() {
                assert!(y >= 0.0 && y <= OH as f32, "path escaped panel: {y}");
            }
        }
    }
}
