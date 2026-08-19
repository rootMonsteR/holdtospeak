//! nib-audio — see docs/design/01-core-app-design.md for this crate's responsibility.
//!
//! Always-on microphone capture with a 400 ms look-back ring, ported from the validated
//! `spikes/vslice` prototype. The ring uses absolute-sample accounting so a burst of rapid
//! push-to-talk taps can never lose audio: an utterance's `[start_abs, end_abs)` range is
//! frozen in absolute sample indices, and a pending counter keeps released-but-not-yet-copied
//! samples alive even while a previous utterance is still being transcribed. See [`Capture`].
//!
//! The prototype kept the capture state in `static` globals shared with the LL keyboard hook.
//! Here that state lives inside [`Capture`] (an `Arc<Shared>` cloned into the cpal callback).
//! The hotkey forwarder drives `begin`/`end`/`cancel` at key-event time via the `Send + Sync`
//! [`CaptureControl`]; the pipeline thread only does `take_utterance` when it's ready.
#![forbid(unsafe_code)]

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// Look-back captured before push-to-talk is pressed, so the first word is never clipped.
const PREROLL_MS: usize = 400;

/// A frozen, absolute-sample range for one released utterance (defined by the trait wall).
///
/// The indices are positions in the ever-growing stream of mono samples the capture callback
/// has pushed (they are *not* offsets into the ring buffer, which is trimmed continuously).
/// [`Capture::take_utterance`] maps them back into the live ring under the lock.
pub use nib_platform::Utterance;

/// State shared between the owning [`Capture`] and the cpal audio callback.
///
/// `total_samples` is mutated *only* under the `buf` lock, which keeps `base = total - buf.len()`
/// a stable mapping from absolute indices to ring offsets for any reader holding the lock.
struct Shared {
    /// The look-back ring of mono samples at the capture rate.
    buf: Mutex<Vec<f32>>,
    /// Mono samples ever pushed. Mutated only under the `buf` lock.
    total_samples: AtomicUsize,
    /// Absolute index of the current utterance's start (pre-roll included), frozen on `begin`.
    rec_start_abs: AtomicUsize,
    /// True while an utterance is being recorded (the callback stops trimming).
    recording: AtomicBool,
    /// Released-but-not-yet-copied utterances. Keeps their samples alive against trimming.
    pending: AtomicUsize,
    /// Pre-roll length in samples at the capture rate. Immutable after `start`.
    preroll: usize,
    /// The capture (device) sample rate in Hz. Immutable after `start`.
    in_rate: u32,
}

/// Always-on microphone capture with a pre-roll ring.
///
/// Owns the cpal [`Stream`](cpal::Stream) (kept alive for the lifetime of `Capture`) and the shared
/// ring/atomics. Because `cpal::Stream` is `!Send`, `Capture` is `!Send` too: create and use it on
/// one thread (see the module docs).
pub struct Capture {
    shared: Arc<Shared>,
    device_name: String,
    // Held only to keep the audio stream running; dropped with `Capture`. `cpal::Stream` is !Send.
    _stream: cpal::Stream,
}

/// A `Send + Sync` read-only handle onto the live capture ring, for the overlay's voice spectrum.
/// Unlike [`Capture`] it carries no `!Send` stream, so it can move to the overlay thread.
pub struct AudioMonitor {
    shared: Arc<Shared>,
}

impl AudioMonitor {
    /// The most recent up to `n` mono samples at the capture rate (fewer if the ring is shorter).
    pub fn latest(&self, n: usize) -> Vec<f32> {
        let buf = self.shared.buf.lock().unwrap();
        let start = buf.len().saturating_sub(n);
        buf[start..].to_vec()
    }

    /// The capture sample rate (Hz) — needed to map FFT bins to frequencies.
    pub fn sample_rate(&self) -> u32 {
        self.shared.in_rate
    }
}

impl Capture {
    /// Open the default input device and start the always-on pre-roll ring.
    ///
    /// The stream begins playing immediately, so ~400 ms of look-back audio is always buffered.
    /// Returns `Err` if there is no default input device/config or the stream cannot be built.
    /// The capture sample rate is available afterwards via [`Capture::sample_rate`].
    pub fn start() -> Result<Capture, String> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| "no default input (microphone) device".to_string())?;
        let device_name = device.name().unwrap_or_default();
        let cfg = device
            .default_input_config()
            .map_err(|e| format!("no default input config: {e}"))?;
        let in_rate = cfg.sample_rate().0;
        let channels = cfg.channels() as usize;
        let preroll = (in_rate as usize * PREROLL_MS) / 1000;

        let shared = Arc::new(Shared {
            buf: Mutex::new(Vec::new()),
            total_samples: AtomicUsize::new(0),
            rec_start_abs: AtomicUsize::new(0),
            recording: AtomicBool::new(false),
            pending: AtomicUsize::new(0),
            preroll,
            in_rate,
        });

        // The callback always appends; while idle (nothing recording or pending) it trims the ring
        // to the last `preroll` samples. It never trims audio an un-taken utterance still needs.
        let err_fn = |e: cpal::StreamError| eprintln!("cpal stream error: {e}");
        let stream = match cfg.sample_format() {
            cpal::SampleFormat::F32 => {
                let shared = shared.clone();
                device.build_input_stream(
                    &cfg.config(),
                    move |data: &[f32], _: &_| {
                        let mut b = shared.buf.lock().unwrap();
                        let mut n = 0usize;
                        for frame in data.chunks(channels) {
                            b.push(frame.iter().sum::<f32>() / channels as f32);
                            n += 1;
                        }
                        // Under the lock: base = total - len stays stable for readers.
                        shared.total_samples.fetch_add(n, SeqCst);
                        shared.trim_if_idle(&mut b);
                    },
                    err_fn,
                    None,
                )
            }
            cpal::SampleFormat::I16 => {
                let shared = shared.clone();
                device.build_input_stream(
                    &cfg.config(),
                    move |data: &[i16], _: &_| {
                        let mut b = shared.buf.lock().unwrap();
                        let mut n = 0usize;
                        for frame in data.chunks(channels) {
                            let sum: f32 = frame.iter().map(|&s| s as f32 / 32768.0).sum();
                            b.push(sum / channels as f32);
                            n += 1;
                        }
                        shared.total_samples.fetch_add(n, SeqCst);
                        shared.trim_if_idle(&mut b);
                    },
                    err_fn,
                    None,
                )
            }
            other => return Err(format!("unsupported sample format {other:?}")),
        }
        .map_err(|e| format!("build_input_stream: {e}"))?;

        stream.play().map_err(|e| format!("stream.play: {e}"))?;

        Ok(Capture {
            shared,
            device_name,
            _stream: stream,
        })
    }

    /// The capture (device) sample rate in Hz.
    pub fn sample_rate(&self) -> u32 {
        self.shared.in_rate
    }

    /// The device/mic display name (for the console banner).
    pub fn device_name(&self) -> String {
        self.device_name.clone()
    }

    /// A cheap `Send + Sync` handle onto the live ring, for the overlay's spectrum. Reading it
    /// never disturbs capture or the utterance accounting.
    pub fn monitor(&self) -> AudioMonitor {
        AudioMonitor {
            shared: self.shared.clone(),
        }
    }

    /// A `Send + Sync` control handle that freezes the recording window (begin/end) from *another*
    /// thread — the hotkey forwarder — so the freeze happens at key-event time even while this
    /// owning thread is blocked transcribing a previous utterance (that ordering is what keeps
    /// back-to-back dictation from losing audio). Shares the same ring + atomics.
    pub fn control(&self) -> CaptureControl {
        CaptureControl {
            shared: self.shared.clone(),
        }
    }

    /// Extract the utterance's mono samples at the capture rate, resample to 16 kHz mono, and
    /// decrement pending. Call when ready to transcribe.
    ///
    /// The ring is never cleared: the next utterance may already be recording into it; the callback
    /// resumes trimming once nothing is recording or pending.
    pub fn take_utterance(&self, u: &Utterance) -> Vec<f32> {
        let samples = self.shared.take_raw(u);
        resample_to_16k(&samples, self.shared.in_rate)
    }
}

impl nib_platform::AudioCapture for Capture {
    fn take_utterance(&self, u: &Utterance) -> Vec<f32> {
        Capture::take_utterance(self, u)
    }
    fn device_name(&self) -> String {
        Capture::device_name(self)
    }
}

impl Shared {
    /// Freeze the utterance start at `total_samples - preroll`. (total_samples may be one callback
    /// chunk (~10 ms) stale; immaterial versus the 400 ms pre-roll.)
    fn begin_utterance(&self) {
        let start = self.total_samples.load(SeqCst).saturating_sub(self.preroll);
        self.rec_start_abs.store(start, SeqCst);
        self.recording.store(true, SeqCst);
    }

    /// Freeze the utterance end + mark it pending. Order matters — pending is incremented *before*
    /// `recording` is cleared so the audio callback can never trim this utterance's samples away.
    fn end_utterance(&self) -> Utterance {
        self.pending.fetch_add(1, SeqCst);
        self.recording.store(false, SeqCst);
        Utterance {
            start_abs: self.rec_start_abs.load(SeqCst),
            end_abs: self.total_samples.load(SeqCst),
        }
    }

    /// Abandon an in-progress utterance without producing a range: clears `recording` WITHOUT
    /// incrementing `pending` (there will be no `take_utterance` to balance it). Without this, a
    /// cancelled hold would leave `recording` stuck true and the callback would never trim —
    /// unbounded ring growth.
    fn cancel_utterance(&self) {
        self.recording.store(false, SeqCst);
    }

    /// Map the frozen absolute range into the live ring and copy it out, then release the hold.
    /// Bounds are clamped, so a stale range yields a short/empty (never panicking) slice.
    fn take_raw(&self, u: &Utterance) -> Vec<f32> {
        let samples = {
            let b = self.buf.lock().unwrap();
            // total_samples only changes under this lock, so `base` is stable here.
            let base = self.total_samples.load(SeqCst) - b.len();
            let s = u.start_abs.saturating_sub(base).min(b.len());
            let e = u.end_abs.saturating_sub(base).min(b.len());
            b[s..e.max(s)].to_vec()
        };
        self.pending.fetch_sub(1, SeqCst); // copy done; callback may trim again
        samples
    }

    /// The callback's idle trim: keep only the pre-roll while nothing is recording or pending.
    /// Factored out of the cpal closures so the H1 invariant ("a released-but-untaken utterance's
    /// samples are never trimmed") is unit-testable without a device.
    fn trim_if_idle(&self, b: &mut Vec<f32>) {
        if !self.recording.load(SeqCst) && self.pending.load(SeqCst) == 0 {
            let excess = b.len().saturating_sub(self.preroll);
            if excess > 0 {
                b.drain(0..excess);
            }
        }
    }

    /// Test-only constructor (no device).
    #[cfg(test)]
    fn for_test(preroll: usize, in_rate: u32) -> Arc<Shared> {
        Arc::new(Shared {
            buf: Mutex::new(Vec::new()),
            total_samples: AtomicUsize::new(0),
            rec_start_abs: AtomicUsize::new(0),
            recording: AtomicBool::new(false),
            pending: AtomicUsize::new(0),
            preroll,
            in_rate,
        })
    }

    /// Test-only: what the cpal callback does — append mono samples, then idle-trim.
    #[cfg(test)]
    fn push_for_test(&self, samples: &[f32]) {
        let mut b = self.buf.lock().unwrap();
        b.extend_from_slice(samples);
        self.total_samples.fetch_add(samples.len(), SeqCst);
        self.trim_if_idle(&mut b);
    }
}

/// A `Send + Sync` handle that freezes the utterance window from any thread (see [`Capture::control`]).
/// The actual (`!Send`) buffer read stays on the owning thread via [`Capture::take_utterance`].
pub struct CaptureControl {
    shared: Arc<Shared>,
}

impl CaptureControl {
    /// Freeze the utterance start (PTT down).
    pub fn begin_utterance(&self) {
        self.shared.begin_utterance();
    }

    /// Freeze the utterance end + mark pending (PTT up); returns the range for `take_utterance`.
    pub fn end_utterance(&self) -> Utterance {
        self.shared.end_utterance()
    }

    /// Abandon an in-progress utterance (hook Cancel / arming abort): unfreezes the ring without
    /// creating a pending range. Safe to call when nothing is recording.
    pub fn cancel_utterance(&self) {
        self.shared.cancel_utterance();
    }
}

// ---- audio: resample to 16 kHz mono ----------------------------------------------------------
// Proper anti-aliased sinc resampling (rubato). Naive linear downsampling aliases high
// frequencies into the speech band and hurts ASR accuracy, so it's only the fallback.

/// Resample mono `input` at `in_rate` Hz to 16 kHz mono.
pub fn resample_to_16k(input: &[f32], in_rate: u32) -> Vec<f32> {
    if in_rate == 16000 || input.len() < 2 {
        return input.to_vec();
    }
    match resample_sinc(input, in_rate) {
        Some(v) if v.len() > 1 => v,
        _ => resample_linear(input, in_rate),
    }
}

fn resample_sinc(input: &[f32], in_rate: u32) -> Option<Vec<f32>> {
    use rubato::{
        Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
    };
    let params = SincInterpolationParameters {
        sinc_len: 256,
        f_cutoff: 0.95,
        interpolation: SincInterpolationType::Linear,
        oversampling_factor: 160,
        window: WindowFunction::BlackmanHarris2,
    };
    let ratio = 16000.0 / in_rate as f64;
    let mut r = SincFixedIn::<f32>::new(ratio, 1.1, params, input.len(), 1).ok()?;
    let out = r.process(&[input.to_vec()], None).ok()?;
    out.into_iter().next()
}

fn resample_linear(input: &[f32], in_rate: u32) -> Vec<f32> {
    let out_len = (input.len() as u64 * 16000 / in_rate as u64) as usize;
    let mut out = Vec::with_capacity(out_len);
    let step = in_rate as f64 / 16000.0;
    for i in 0..out_len {
        let pos = i as f64 * step;
        let i0 = pos.floor() as usize;
        let frac = pos - i0 as f64;
        let a = input[i0.min(input.len() - 1)];
        let b = input[(i0 + 1).min(input.len() - 1)];
        out.push(a + (b - a) * frac as f32);
    }
    out
}

/// Write an f32 mono 16 kHz slice to a 16-bit PCM WAV (samples are clamped to `[-1, 1]`).
pub fn write_wav_16k(path: &Path, samples_16k: &[f32]) -> std::io::Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 16000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).map_err(to_io)?;
    for &s in samples_16k {
        let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
        w.write_sample(v).map_err(to_io)?;
    }
    w.finalize().map_err(to_io)
}

/// Map a `hound::Error` into `std::io::Error`, preserving the underlying I/O error when present.
fn to_io(e: hound::Error) -> std::io::Error {
    match e {
        hound::Error::IoError(io) => io,
        other => std::io::Error::other(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The H1 regression: an utterance released while the consumer is busy (its `take` delayed)
    /// must survive continued callback activity untouched — `pending` holds the ring open.
    #[test]
    fn released_utterance_survives_busy_consumer() {
        let s = Shared::for_test(4, 16000); // tiny 4-sample pre-roll to force aggressive trimming
        s.push_for_test(&[0.0; 8]); // idle chatter → trimmed to pre-roll
        assert_eq!(s.buf.lock().unwrap().len(), 4);

        s.begin_utterance(); // PTT down (freeze happens at event time)
        s.push_for_test(&[1.0; 6]); // the speech
        let u = s.end_utterance(); // PTT up — consumer is "busy", take is delayed

        // Callback keeps running while the previous utterance is still untaken…
        s.push_for_test(&[2.0; 50]);
        s.push_for_test(&[3.0; 50]);

        // …and the released range must still be fully intact.
        let audio = s.take_raw(&u);
        assert_eq!(audio.iter().filter(|&&v| v == 1.0).count(), 6);
        assert_eq!(s.pending.load(SeqCst), 0);

        // With the hold released, the next idle push trims back down to the pre-roll.
        s.push_for_test(&[0.0; 8]);
        assert_eq!(s.buf.lock().unwrap().len(), 4);
    }

    /// Cancel must unfreeze the ring WITHOUT leaking `pending` — a cancelled hold that left
    /// `recording` true (or `pending` above zero) would grow the ring forever.
    #[test]
    fn cancel_unfreezes_without_leaking_pending() {
        let s = Shared::for_test(4, 16000);
        s.begin_utterance();
        s.push_for_test(&[1.0; 100]); // recording: nothing trimmed
        assert_eq!(s.buf.lock().unwrap().len(), 100);

        s.cancel_utterance();
        assert_eq!(s.pending.load(SeqCst), 0);
        s.push_for_test(&[0.0; 8]); // idle again → trims to pre-roll
        assert_eq!(s.buf.lock().unwrap().len(), 4);
    }

    #[test]
    fn resample_passthrough_at_16k() {
        let input: Vec<f32> = (0..100).map(|i| i as f32 / 100.0).collect();
        assert_eq!(resample_to_16k(&input, 16000), input);
    }

    #[test]
    fn resample_linear_halves_length() {
        let input = vec![0.5f32; 32000];
        let out = resample_linear(&input, 32000);
        assert_eq!(out.len(), 16000);
        assert!(out.iter().all(|&v| (v - 0.5).abs() < 1e-6));
    }

    #[test]
    fn wav_round_trip_and_clamp() {
        let path = std::env::temp_dir().join(format!("nib_audio_test_{}.wav", std::process::id()));
        let samples = [0.0f32, 0.5, -0.5, 2.0, -2.0];
        write_wav_16k(&path, &samples).unwrap();
        let mut r = hound::WavReader::open(&path).unwrap();
        let spec = r.spec();
        assert_eq!(
            (spec.channels, spec.sample_rate, spec.bits_per_sample),
            (1, 16000, 16)
        );
        let vals: Vec<i16> = r.samples::<i16>().map(|s| s.unwrap()).collect();
        assert_eq!(vals.len(), samples.len());
        assert_eq!(vals[3], 32767); // +2.0 clamped
        assert_eq!(vals[4], -32767); // -2.0 clamped
        let _ = std::fs::remove_file(&path);
    }
}
