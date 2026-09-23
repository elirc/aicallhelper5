//! Pure DSP for the capture path: sample-format conversion + downmix to mono,
//! an anti-aliased, phase-continuous streaming resampler to 16 kHz, f32 -> i16
//! conversion with clamping, a 2048-sample framer and per-frame RMS.
//!
//! Nothing here touches a device or a thread; everything is deterministic and
//! unit-tested below.

use callcore_contract::config::{FRAME_SAMPLES, SAMPLE_RATE};
use callcore_contract::ports::AudioFrame;

// ───────────────────────────── downmix ─────────────────────────────

/// Interleaved input samples in one of the formats cpal may hand us.
#[derive(Debug, Clone, Copy)]
pub enum Samples<'a> {
    F32(&'a [f32]),
    I16(&'a [i16]),
    U16(&'a [u16]),
    I32(&'a [i32]),
}

impl Samples<'_> {
    pub fn len(&self) -> usize {
        match self {
            Samples::F32(s) => s.len(),
            Samples::I16(s) => s.len(),
            Samples::U16(s) => s.len(),
            Samples::I32(s) => s.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[inline]
fn i16_to_f32(v: i16) -> f32 {
    v as f32 / 32_768.0
}

#[inline]
fn u16_to_f32(v: u16) -> f32 {
    (v as f32 - 32_768.0) / 32_768.0
}

#[inline]
fn i32_to_f32(v: i32) -> f32 {
    (v as f64 / 2_147_483_648.0) as f32
}

fn downmix_with<T: Copy>(
    input: &[T],
    channels: usize,
    conv: impl Fn(T) -> f32,
    out: &mut Vec<f32>,
) {
    // `chunks_exact(0)` panics: a zero-channel chunk is a malformed driver
    // buffer. The capture callback runs this inside `catch_unwind`, so such a
    // chunk is dropped, never the stream.
    assert!(channels > 0, "downmix: zero channels");
    if channels == 1 {
        out.extend(input.iter().map(|&v| conv(v)));
        return;
    }
    let scale = 1.0 / channels as f32;
    out.reserve(input.len() / channels);
    for frame in input.chunks_exact(channels) {
        let sum: f32 = frame.iter().map(|&v| conv(v)).sum();
        out.push(sum * scale);
    }
    // A trailing partial frame (never produced by a sane driver) is dropped.
}

/// Downmix interleaved `channels`-channel audio to mono f32 in -1..1 by
/// averaging the channels. Appends to `out`.
///
/// # Panics
/// When `channels == 0` (see [`downmix_with`]); callers on the audio callback
/// guard with `catch_unwind`.
pub fn downmix_into(input: Samples<'_>, channels: u16, out: &mut Vec<f32>) {
    let ch = channels as usize;
    match input {
        Samples::F32(s) => downmix_with(s, ch, |v| v, out),
        Samples::I16(s) => downmix_with(s, ch, i16_to_f32, out),
        Samples::U16(s) => downmix_with(s, ch, u16_to_f32, out),
        Samples::I32(s) => downmix_with(s, ch, i32_to_f32, out),
    }
}

/// Convenience wrapper around [`downmix_into`].
pub fn downmix(input: Samples<'_>, channels: u16) -> Vec<f32> {
    let mut out = Vec::new();
    downmix_into(input, channels, &mut out);
    out
}

// ───────────────────────────── f32 -> i16, RMS ─────────────────────────────

/// Convert one f32 sample (nominal -1..1) to i16, clamping out-of-range input.
/// NaN maps to 0.
#[inline]
pub fn f32_to_i16(v: f32) -> i16 {
    if v.is_nan() {
        return 0;
    }
    (v.clamp(-1.0, 1.0) * 32_767.0).round() as i16
}

/// RMS of i16 samples normalized to 0..1 (full-scale square wave = 1.0).
pub fn rms(samples: &[i16]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples
        .iter()
        .map(|&s| {
            let v = s as f64 / 32_768.0;
            v * v
        })
        .sum();
    ((sum / samples.len() as f64).sqrt() as f32).clamp(0.0, 1.0)
}

// ───────────────────────────── resampler ─────────────────────────────

/// Output cutoff (Hz) of the anti-aliasing filter.
pub const CUTOFF_HZ: f64 = 7_200.0;
/// Filter length in input samples at 48 kHz; scaled up for higher input rates
/// so the transition band stays the same width in Hz.
pub const BASE_TAPS: usize = 64;
/// Kaiser window beta (~70 dB stop-band).
pub const KAISER_BETA: f64 = 7.0;
/// Above this many polyphase phases the coefficient table is not
/// precomputed (odd rates with a tiny gcd); coefficients are computed per
/// output sample instead. Same numbers, just slower.
const MAX_TABLE_PHASES: u64 = 2_048;

fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        let t = a % b;
        a = b;
        b = t;
    }
    a
}

/// Zeroth-order modified Bessel function of the first kind (power series).
fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let half = x / 2.0;
    for k in 1..200 {
        let f = half / k as f64;
        term *= f * f;
        sum += term;
        if term < sum * 1e-17 {
            break;
        }
    }
    sum
}

#[derive(Debug, Clone)]
struct Kernel {
    taps: usize,
    /// Normalized cutoff relative to the INPUT rate (cycles/sample).
    fc: f64,
    i0_beta: f64,
}

impl Kernel {
    /// Continuous windowed-sinc kernel, `tau` in input samples, support
    /// |tau| < taps/2.
    fn eval(&self, tau: f64) -> f64 {
        let half = self.taps as f64 / 2.0;
        let r = tau / half;
        if r.abs() >= 1.0 {
            return 0.0;
        }
        let x = 2.0 * self.fc * tau;
        let sinc = if x.abs() < 1e-12 {
            1.0
        } else {
            (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x)
        };
        let w = bessel_i0(KAISER_BETA * (1.0 - r * r).sqrt()) / self.i0_beta;
        2.0 * self.fc * sinc * w
    }

    /// Coefficients for fractional offset `frac` (0..1), applied to input
    /// samples `x[i - (taps/2 - 1) + k]`, `k in 0..taps`. Normalized to unit
    /// DC gain.
    fn phase(&self, frac: f64) -> Vec<f32> {
        let half = (self.taps / 2) as f64;
        let raw: Vec<f64> = (0..self.taps)
            .map(|k| self.eval(frac + half - 1.0 - k as f64))
            .collect();
        let sum: f64 = raw.iter().sum();
        raw.iter().map(|c| (c / sum) as f32).collect()
    }
}

#[derive(Debug, Clone)]
enum Mode {
    Passthrough,
    Filter {
        kernel: Kernel,
        /// Precomputed per-phase coefficients, `table[p]` for phase p/L.
        table: Option<Vec<Vec<f32>>>,
    },
}

/// Streaming rational resampler (`in_rate` -> 16 kHz) built on a Kaiser-
/// windowed sinc evaluated polyphase. History is carried across calls, so the
/// output is bit-identical however the input is chunked. Output sample `n`
/// sits exactly at input time `n * in_rate / 16000` (zero phase, no drift:
/// all position math is integer).
#[derive(Debug, Clone)]
pub struct Resampler {
    in_rate: u32,
    /// Upsample factor (output rate / gcd).
    l: u64,
    /// Downsample factor (input rate / gcd).
    m: u64,
    mode: Mode,
    taps: usize,
    /// Buffered input; `buf[0]` is absolute input index `buf_start`
    /// (negative indices are the implicit zeros before the stream).
    buf: Vec<f32>,
    buf_start: i64,
    /// Total input samples received.
    in_count: u64,
    /// Index of the next output sample.
    next_out: u64,
}

impl Resampler {
    /// # Panics
    /// If `in_rate == 0`.
    pub fn new(in_rate: u32) -> Self {
        assert!(in_rate > 0, "resampler: zero input rate");
        let out = SAMPLE_RATE as u64;
        let g = gcd(in_rate as u64, out);
        let l = out / g;
        let m = in_rate as u64 / g;
        if in_rate == SAMPLE_RATE {
            return Self {
                in_rate,
                l,
                m,
                mode: Mode::Passthrough,
                taps: 0,
                buf: Vec::new(),
                buf_start: 0,
                in_count: 0,
                next_out: 0,
            };
        }
        let mut taps =
            BASE_TAPS.max(((BASE_TAPS as f64) * in_rate as f64 / 48_000.0).round() as usize);
        taps += taps % 2; // even
        let cutoff = CUTOFF_HZ.min(0.45 * in_rate as f64);
        let kernel = Kernel {
            taps,
            fc: cutoff / in_rate as f64,
            i0_beta: bessel_i0(KAISER_BETA),
        };
        let table = (l <= MAX_TABLE_PHASES)
            .then(|| (0..l).map(|p| kernel.phase(p as f64 / l as f64)).collect());
        let half = (taps / 2) as i64;
        Self {
            in_rate,
            l,
            m,
            mode: Mode::Filter { kernel, table },
            taps,
            buf: Vec::new(),
            buf_start: -(half - 1),
            in_count: 0,
            next_out: 0,
        }
        .with_zero_history()
    }

    fn with_zero_history(mut self) -> Self {
        let half = self.taps / 2;
        self.buf = vec![0.0; half.saturating_sub(1)];
        self
    }

    pub fn in_rate(&self) -> u32 {
        self.in_rate
    }

    /// Filter length in input samples (0 for passthrough).
    pub fn taps(&self) -> usize {
        self.taps
    }

    /// Total output samples a stream of `in_samples` inputs produces once
    /// flushed: `ceil(in_samples * 16000 / in_rate)`.
    pub fn total_output_len(in_rate: u32, in_samples: u64) -> u64 {
        let out = SAMPLE_RATE as u64;
        let g = gcd(in_rate as u64, out);
        let (l, m) = (out / g, in_rate as u64 / g);
        (in_samples * l).div_ceil(m)
    }

    /// Output samples produced so far.
    pub fn produced(&self) -> u64 {
        self.next_out
    }

    fn compute(&self, n: u64) -> f32 {
        let Mode::Filter { kernel, table } = &self.mode else {
            unreachable!("compute in passthrough");
        };
        let pos = n * self.m;
        let i = (pos / self.l) as i64;
        let p = pos % self.l;
        let half = (self.taps / 2) as i64;
        let first = i - (half - 1);
        let start = (first - self.buf_start) as usize;
        let window = &self.buf[start..start + self.taps];
        let owned;
        let coeffs: &[f32] = match table {
            Some(t) => &t[p as usize],
            None => {
                owned = kernel.phase(p as f64 / self.l as f64);
                &owned
            }
        };
        let mut acc = 0.0f32;
        for (c, x) in coeffs.iter().zip(window) {
            acc += c * x;
        }
        acc
    }

    /// Push input samples; append every output sample whose full filter
    /// support is now available to `out`.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if matches!(self.mode, Mode::Passthrough) {
            out.extend_from_slice(input);
            self.in_count += input.len() as u64;
            self.next_out = self.in_count;
            return;
        }
        self.buf.extend_from_slice(input);
        self.in_count += input.len() as u64;
        let half = (self.taps / 2) as i64;
        let end = self.buf_start + self.buf.len() as i64; // exclusive
        loop {
            let i = ((self.next_out * self.m) / self.l) as i64;
            if i + half >= end {
                break;
            }
            out.push(self.compute(self.next_out));
            self.next_out += 1;
        }
        self.trim();
    }

    /// Emit the remaining outputs (those at input times before the end of the
    /// stream), treating samples past the end as silence. After a flush the
    /// resampler is exhausted: total output == [`Self::total_output_len`].
    pub fn flush(&mut self, out: &mut Vec<f32>) {
        if matches!(self.mode, Mode::Passthrough) {
            return;
        }
        let half = self.taps / 2;
        self.buf.extend(std::iter::repeat(0.0).take(half + 1));
        let total = Self::total_output_len(self.in_rate, self.in_count);
        while self.next_out < total {
            out.push(self.compute(self.next_out));
            self.next_out += 1;
        }
        self.trim();
    }

    fn trim(&mut self) {
        let half = (self.taps / 2) as i64;
        let i = ((self.next_out * self.m) / self.l) as i64;
        let keep_from = i - (half - 1);
        let drop = (keep_from - self.buf_start).clamp(0, self.buf.len() as i64) as usize;
        if drop > 0 {
            self.buf.drain(..drop);
            self.buf_start += drop as i64;
        }
    }
}

// ───────────────────────────── framer ─────────────────────────────

/// Collects i16 samples into 2048-sample frames (with RMS).
#[derive(Debug, Clone, Default)]
pub struct Framer {
    pending: Vec<i16>,
}

impl Framer {
    pub fn new() -> Self {
        Self {
            pending: Vec::with_capacity(FRAME_SAMPLES),
        }
    }

    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// Append samples; push every completed frame to `out`.
    pub fn push(&mut self, samples: &[i16], out: &mut Vec<AudioFrame>) {
        let mut rest = samples;
        while !rest.is_empty() {
            let need = FRAME_SAMPLES - self.pending.len();
            let take = need.min(rest.len());
            self.pending.extend_from_slice(&rest[..take]);
            rest = &rest[take..];
            if self.pending.len() == FRAME_SAMPLES {
                let samples =
                    std::mem::replace(&mut self.pending, Vec::with_capacity(FRAME_SAMPLES));
                let rms = rms(&samples);
                out.push(AudioFrame { samples, rms });
            }
        }
    }

    /// The final partial frame, if any samples are pending.
    pub fn flush(&mut self) -> Option<AudioFrame> {
        if self.pending.is_empty() {
            return None;
        }
        let samples = std::mem::take(&mut self.pending);
        let rms = rms(&samples);
        Some(AudioFrame { samples, rms })
    }
}

// ───────────────────────────── pipeline ─────────────────────────────

/// Mono f32 at the device rate -> 16 kHz i16 frames.
#[derive(Debug, Clone)]
pub struct Pipeline {
    resampler: Resampler,
    framer: Framer,
    scratch: Vec<f32>,
    scratch_i16: Vec<i16>,
}

impl Pipeline {
    pub fn new(in_rate: u32) -> Self {
        Self {
            resampler: Resampler::new(in_rate),
            framer: Framer::new(),
            scratch: Vec::new(),
            scratch_i16: Vec::new(),
        }
    }

    pub fn in_rate(&self) -> u32 {
        self.resampler.in_rate()
    }

    pub fn push(&mut self, mono: &[f32], out: &mut Vec<AudioFrame>) {
        self.scratch.clear();
        self.resampler.process(mono, &mut self.scratch);
        self.emit(out);
    }

    fn emit(&mut self, out: &mut Vec<AudioFrame>) {
        self.scratch_i16.clear();
        self.scratch_i16
            .extend(self.scratch.iter().map(|&v| f32_to_i16(v)));
        self.framer.push(&self.scratch_i16, out);
    }

    /// Switch to a new input rate mid-stream: the old resampler's tail is
    /// flushed into the framer (frame continuity kept), then a fresh
    /// resampler takes over. No-op when the rate is unchanged.
    pub fn change_rate(&mut self, in_rate: u32, out: &mut Vec<AudioFrame>) {
        if in_rate == self.resampler.in_rate() {
            return;
        }
        self.scratch.clear();
        self.resampler.flush(&mut self.scratch);
        self.emit(out);
        self.resampler = Resampler::new(in_rate);
    }

    /// Resampler tail + final partial frame.
    pub fn flush(&mut self, out: &mut Vec<AudioFrame>) {
        self.scratch.clear();
        self.resampler.flush(&mut self.scratch);
        self.emit(out);
        if let Some(f) = self.framer.flush() {
            out.push(f);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, freq: f64, amp: f64, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| {
                (amp * (2.0 * std::f64::consts::PI * freq * i as f64 / rate as f64).sin()) as f32
            })
            .collect()
    }

    fn resample_all(rate: u32, input: &[f32]) -> Vec<f32> {
        let mut r = Resampler::new(rate);
        let mut out = Vec::new();
        r.process(input, &mut out);
        r.flush(&mut out);
        out
    }

    /// RMS of the middle of a signal (skipping filter edges).
    fn mid_rms(x: &[f32]) -> f64 {
        let skip = x.len() / 10;
        let mid = &x[skip..x.len() - skip];
        (mid.iter().map(|&v| (v as f64) * (v as f64)).sum::<f64>() / mid.len() as f64).sqrt()
    }

    fn db(ratio: f64) -> f64 {
        20.0 * ratio.log10()
    }

    #[test]
    fn downmix_stereo_f32_averages_channels() {
        let out = downmix(Samples::F32(&[1.0, 0.0, 0.5, 0.5, -1.0, 1.0]), 2);
        assert_eq!(out, vec![0.5, 0.5, 0.0]);
    }

    #[test]
    fn downmix_mono_is_identity() {
        let out = downmix(Samples::F32(&[0.1, -0.2, 0.3]), 1);
        assert_eq!(out, vec![0.1, -0.2, 0.3]);
    }

    #[test]
    fn downmix_six_channels() {
        let frame = [0.6f32, 0.0, 0.0, 0.0, 0.0, 0.0];
        let out = downmix(Samples::F32(&frame), 6);
        assert!((out[0] - 0.1).abs() < 1e-6);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn downmix_integer_formats_scale_to_unit_range() {
        assert_eq!(
            downmix(Samples::I16(&[i16::MIN, 0, 16_384]), 1),
            vec![-1.0, 0.0, 0.5]
        );
        assert_eq!(
            downmix(Samples::U16(&[0, 32_768, 49_152]), 1),
            vec![-1.0, 0.0, 0.5]
        );
        assert_eq!(
            downmix(Samples::I32(&[i32::MIN, 0, 1 << 30]), 1),
            vec![-1.0, 0.0, 0.5]
        );
        // stereo i16
        assert_eq!(downmix(Samples::I16(&[16_384, -16_384]), 2), vec![0.0]);
    }

    #[test]
    fn downmix_drops_trailing_partial_frame() {
        let out = downmix(Samples::F32(&[0.2, 0.4, 0.9]), 2);
        assert_eq!(out.len(), 1);
    }

    #[test]
    #[should_panic(expected = "zero channels")]
    fn downmix_zero_channels_panics_for_callback_guard() {
        downmix(Samples::F32(&[0.0]), 0);
    }

    #[test]
    fn f32_to_i16_clamps_and_rounds() {
        assert_eq!(f32_to_i16(0.0), 0);
        assert_eq!(f32_to_i16(1.0), 32_767);
        assert_eq!(f32_to_i16(-1.0), -32_767);
        assert_eq!(f32_to_i16(5.0), 32_767);
        assert_eq!(f32_to_i16(-5.0), -32_767);
        assert_eq!(f32_to_i16(f32::INFINITY), 32_767);
        assert_eq!(f32_to_i16(f32::NEG_INFINITY), -32_767);
        assert_eq!(f32_to_i16(f32::NAN), 0);
        assert_eq!(f32_to_i16(0.5), 16_384);
    }

    #[test]
    fn rms_normalized() {
        assert_eq!(rms(&[]), 0.0);
        assert_eq!(rms(&[0; 100]), 0.0);
        let full = vec![i16::MIN; 64];
        assert!((rms(&full) - 1.0).abs() < 1e-6);
        let half: Vec<i16> = (0..64)
            .map(|i| if i % 2 == 0 { 16_384 } else { -16_384 })
            .collect();
        assert!((rms(&half) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn framer_emits_full_frames_and_flushes_partial() {
        let mut f = Framer::new();
        let mut out = Vec::new();
        let data: Vec<i16> = (0..5_000).map(|i| (i % 1000) as i16).collect();
        for chunk in data.chunks(333) {
            f.push(chunk, &mut out);
        }
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|fr| fr.samples.len() == FRAME_SAMPLES));
        let last = f.flush().expect("partial");
        assert_eq!(last.samples.len(), 5_000 - 2 * FRAME_SAMPLES);
        let joined: Vec<i16> = out
            .iter()
            .chain(std::iter::once(&last))
            .flat_map(|fr| fr.samples.clone())
            .collect();
        assert_eq!(joined, data);
        assert!(f.flush().is_none());
        assert_eq!(out[0].rms, rms(&out[0].samples));
    }

    #[test]
    fn framer_exact_multiple_has_no_partial() {
        let mut f = Framer::new();
        let mut out = Vec::new();
        f.push(&vec![1; FRAME_SAMPLES * 3], &mut out);
        assert_eq!(out.len(), 3);
        assert!(f.flush().is_none());
    }

    #[test]
    fn passthrough_at_16k_is_identity() {
        let x = sine(16_000, 440.0, 0.5, 3_000);
        let mut r = Resampler::new(16_000);
        let mut out = Vec::new();
        r.process(&x[..1_000], &mut out);
        r.process(&x[1_000..], &mut out);
        r.flush(&mut out);
        assert_eq!(out, x);
        assert_eq!(r.taps(), 0);
    }

    #[test]
    fn one_khz_sine_keeps_amplitude_within_1db_at_common_rates() {
        for rate in [44_100, 48_000, 96_000, 22_050, 32_000, 88_200, 8_000] {
            let x = sine(rate, 1_000.0, 0.5, rate as usize);
            let y = resample_all(rate, &x);
            let ratio = mid_rms(&y) / mid_rms(&x);
            assert!(db(ratio).abs() < 1.0, "rate {rate}: {:.3} dB", db(ratio));
        }
    }

    #[test]
    fn resampled_sine_has_correct_frequency_and_phase() {
        // Output sample n must equal the sine at time n/16000 (zero phase).
        let rate = 44_100;
        let x = sine(rate, 1_000.0, 0.5, rate as usize);
        let y = resample_all(rate, &x);
        let expect = sine(16_000, 1_000.0, 0.5, y.len());
        let skip = 200;
        let max_err = y[skip..y.len() - skip]
            .iter()
            .zip(&expect[skip..expect.len() - skip])
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(max_err < 0.01, "max err {max_err}");
    }

    #[test]
    fn twelve_khz_at_48k_is_attenuated_at_least_40db() {
        let x = sine(48_000, 12_000.0, 0.5, 48_000);
        let y = resample_all(48_000, &x);
        let att = db(mid_rms(&y) / mid_rms(&x));
        assert!(att <= -40.0, "only {att:.1} dB");
    }

    #[test]
    fn aliasing_tones_are_attenuated_at_other_rates() {
        for (rate, freq) in [
            (44_100u32, 11_025.0),
            (96_000, 20_000.0),
            (32_000, 12_000.0),
            (22_050, 10_000.0),
        ] {
            let x = sine(rate, freq, 0.5, rate as usize);
            let y = resample_all(rate, &x);
            let att = db(mid_rms(&y) / mid_rms(&x));
            assert!(att <= -40.0, "rate {rate} tone {freq}: {att:.1} dB");
        }
    }

    #[test]
    fn dc_passes_with_unit_gain() {
        for rate in [44_100, 48_000, 96_000, 22_050, 11_025] {
            let x = vec![0.25f32; rate as usize / 2];
            let y = resample_all(rate, &x);
            let taps = Resampler::new(rate).taps();
            // Away from the start/end edges the output is exactly the DC level.
            let edge = taps * 16_000 / rate as usize + 2;
            for &v in &y[edge..y.len() - edge] {
                assert!((v - 0.25).abs() < 1e-5, "rate {rate}: {v}");
            }
        }
    }

    #[test]
    fn chunk_split_invariance_is_bit_exact() {
        for rate in [44_100u32, 48_000, 96_000, 22_050, 32_000, 16_000, 11_025] {
            let x = sine(rate, 1_234.5, 0.7, 3_000);
            let whole = resample_all(rate, &x);
            let proto = Resampler::new(rate);
            // Every single split point for two chunks.
            for split in (0..=x.len()).step_by(41).chain([1, 2, x.len() - 1]) {
                let mut r = proto.clone();
                let mut out = Vec::new();
                r.process(&x[..split], &mut out);
                r.process(&x[split..], &mut out);
                r.flush(&mut out);
                assert_eq!(out, whole, "rate {rate} split {split}");
            }
            // Many irregular chunks, including empty and 1-sample chunks.
            let mut r = proto.clone();
            let mut out = Vec::new();
            let mut pos = 0;
            let sizes = [0usize, 1, 7, 480, 1, 2, 1_023, 3, 441, 0, 17];
            let mut k = 0;
            while pos < x.len() {
                let n = sizes[k % sizes.len()].min(x.len() - pos);
                r.process(&x[pos..pos + n], &mut out);
                pos += n;
                k += 1;
            }
            r.flush(&mut out);
            assert_eq!(out, whole, "rate {rate} irregular chunks");
        }
    }

    #[test]
    fn output_length_is_exact_over_long_runs_without_drift() {
        for rate in [44_100u32, 48_000, 22_050, 96_000, 32_000, 44_056] {
            let mut r = Resampler::new(rate);
            let mut out = Vec::new();
            let chunk = vec![0.1f32; 4_410 + 7];
            let mut fed: u64 = 0;
            // 20 s of audio per rate in odd-sized chunks (all position math is
            // integer, so a longer run cannot drift differently; kept short
            // because tests run unoptimized).
            let target = rate as u64 * 20;
            let half = (r.taps() / 2) as u64;
            while fed < target {
                let n = chunk.len().min((target - fed) as usize);
                r.process(&chunk[..n], &mut out);
                fed += n as u64;
                // Latency stays bounded: never more than ~taps/2 input samples behind.
                let lag = Resampler::total_output_len(rate, fed) - r.produced();
                let bound = (half + 2) * 16_000 / rate as u64 + 2;
                assert!(lag <= bound, "rate {rate}: lag {lag} > {bound}");
            }
            r.flush(&mut out);
            let expect = Resampler::total_output_len(rate, fed);
            assert_eq!(out.len() as u64, expect, "rate {rate}");
            assert_eq!(expect, (fed * 16_000).div_ceil(rate as u64));
        }
    }

    #[test]
    fn total_output_len_formula() {
        assert_eq!(Resampler::total_output_len(48_000, 3), 1);
        assert_eq!(Resampler::total_output_len(48_000, 4), 2);
        assert_eq!(Resampler::total_output_len(44_100, 441), 160);
        assert_eq!(Resampler::total_output_len(44_100, 442), 161);
        assert_eq!(Resampler::total_output_len(16_000, 5), 5);
        assert_eq!(Resampler::total_output_len(8_000, 5), 10);
    }

    #[test]
    fn untabled_odd_rate_matches_precision() {
        // gcd(16000, 44_057) == 1 -> 16000 phases, coefficients computed on the fly.
        let rate = 44_057;
        let x = sine(rate, 1_000.0, 0.5, rate as usize / 2);
        let y = resample_all(rate, &x);
        assert!(db(mid_rms(&y) / mid_rms(&x)).abs() < 1.0);
        assert_eq!(
            y.len() as u64,
            Resampler::total_output_len(rate, x.len() as u64)
        );
    }

    #[test]
    fn empty_flush_produces_nothing() {
        let mut r = Resampler::new(48_000);
        let mut out = Vec::new();
        r.flush(&mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn pipeline_produces_frames_and_exact_total() {
        let mut p = Pipeline::new(48_000);
        let mut out = Vec::new();
        let x = sine(48_000, 500.0, 0.3, 48_000);
        for c in x.chunks(480) {
            p.push(c, &mut out);
        }
        p.flush(&mut out);
        let total: usize = out.iter().map(|f| f.samples.len()).sum();
        assert_eq!(total, 16_000);
        assert!(out[..out.len() - 1]
            .iter()
            .all(|f| f.samples.len() == FRAME_SAMPLES));
        assert_eq!(out.last().unwrap().samples.len(), 16_000 % FRAME_SAMPLES);
        // 0.3 amplitude sine -> RMS ~0.212.
        assert!((out[2].rms - 0.212).abs() < 0.01, "rms {}", out[2].rms);
    }

    #[test]
    fn pipeline_rate_change_keeps_exact_counts() {
        let mut p = Pipeline::new(48_000);
        let mut out = Vec::new();
        p.push(&vec![0.1; 30_001], &mut out);
        p.change_rate(44_100, &mut out);
        p.push(&vec![0.1; 22_051], &mut out);
        p.flush(&mut out);
        let total: u64 = out.iter().map(|f| f.samples.len() as u64).sum();
        let expect = Resampler::total_output_len(48_000, 30_001)
            + Resampler::total_output_len(44_100, 22_051);
        assert_eq!(total, expect);
    }
}
