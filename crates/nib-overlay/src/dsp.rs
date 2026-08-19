//! Spectrum DSP shared by every style: one windowed FFT per frame, reduced either to `NBARS`
//! log-spaced bar levels (Bars) or to 3 smoothed voice bands (every other style).

use std::sync::Arc;

use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};

use crate::{FFT_N, FMAX, NBARS};

/// The forward FFT plan the render loop reuses every frame (sized for [`FFT_N`]).
pub fn plan_fft() -> Arc<dyn Fft<f32>> {
    FftPlanner::<f32>::new().plan_fft_forward(FFT_N)
}

/// The Hann window matching [`plan_fft`] — built once, applied to every frame's samples.
pub fn hann_window() -> Vec<f32> {
    (0..FFT_N)
        .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (FFT_N as f32 - 1.0)).cos())
        .collect()
}

/// Windowed FFT of the freshest FFT_N mic samples (shared by every style).
pub fn fft_spectrum(samples: &[f32], fft: &dyn Fft<f32>, hann: &[f32]) -> Vec<Complex<f32>> {
    let mut spec = vec![Complex::<f32>::new(0.0, 0.0); FFT_N];
    let start = samples.len().saturating_sub(FFT_N);
    let tail = &samples[start..];
    // right-align the samples in the FFT window (leading zeros if short)
    let off = FFT_N - tail.len();
    for (i, s) in tail.iter().enumerate() {
        spec[off + i].re = *s * hann[off + i];
    }
    fft.process(&mut spec);
    spec
}

pub fn compute_bars(
    samples: &[f32],
    in_rate: u32,
    fft: &dyn Fft<f32>,
    hann: &[f32],
    levels: &mut [f32; NBARS],
    gain: f32,
) {
    let spec = fft_spectrum(samples, fft, hann);
    let half = FFT_N / 2;
    let fpb = in_rate as f32 / FFT_N as f32;
    let (fmin, fmax) = (80.0f32, FMAX);
    for (bar, level) in levels.iter_mut().enumerate() {
        let t0 = bar as f32 / NBARS as f32;
        let t1 = (bar + 1) as f32 / NBARS as f32;
        let f0 = fmin * (fmax / fmin).powf(t0);
        let f1 = fmin * (fmax / fmin).powf(t1);
        let b0 = ((f0 / fpb) as usize).clamp(1, half - 1);
        let b1 = ((f1 / fpb) as usize).clamp(b0 + 1, half);
        let mut peak = 0f32;
        for c in &spec[b0..b1] {
            let m = c.norm();
            if m > peak {
                peak = m;
            }
        }
        // normalize (FFT unnormalized -> divide by N), sqrt to compress, gain. Tunable.
        let v = ((peak / FFT_N as f32).sqrt() * gain).clamp(0.0, 1.0);
        let cur = *level;
        *level = if v > cur { v } else { cur * 0.74 + v * 0.26 }; // fast attack, quicker decay
    }
}

/// Low / mid / high voice-band energies, smoothed (softer attack/decay than the bars).
pub fn compute_bands(
    samples: &[f32],
    in_rate: u32,
    fft: &dyn Fft<f32>,
    hann: &[f32],
    bands: &mut [f32; 3],
    gain: f32,
) {
    let spec = fft_spectrum(samples, fft, hann);
    let half = FFT_N / 2;
    let fpb = in_rate as f32 / FFT_N as f32;
    let ranges = [(80.0f32, 400.0f32), (400.0, 1500.0), (1500.0, FMAX)];
    for (band, (f0, f1)) in bands.iter_mut().zip(ranges) {
        let b0 = ((f0 / fpb) as usize).clamp(1, half - 1);
        let b1 = ((f1 / fpb) as usize).clamp(b0 + 1, half);
        let mut peak = 0f32;
        for c in &spec[b0..b1] {
            peak = peak.max(c.norm());
        }
        let v = ((peak / FFT_N as f32).sqrt() * gain).clamp(0.0, 1.0);
        let cur = *band;
        *band = if v > cur {
            cur * 0.45 + v * 0.55
        } else {
            cur * 0.86 + v * 0.14
        };
    }
}
