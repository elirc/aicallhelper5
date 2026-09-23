//! Pure, platform-independent pieces of the WASAPI backend (unit-tested
//! everywhere): mix-format classification, raw-byte → f32 conversion and the
//! silence gap filler.

use std::time::Duration;

/// `WAVEFORMATEX::wFormatTag` values we understand.
pub const WAVE_FORMAT_PCM: u16 = 1;
pub const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
pub const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;

/// `WAVEFORMATEXTENSIBLE::SubFormat`, reduced to what matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubFormat {
    Pcm,
    Float,
    Other,
}

/// One interleaved sample encoding of the device mix format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleFormat {
    U8,
    I16,
    /// 24-bit little-endian packed in 3 bytes.
    I24,
    /// 32-bit container (also 24-in-32 left-justified, which scales the same).
    I32,
    F32,
    F64,
}

impl SampleFormat {
    pub fn bytes(self) -> usize {
        match self {
            SampleFormat::U8 => 1,
            SampleFormat::I16 => 2,
            SampleFormat::I24 => 3,
            SampleFormat::I32 | SampleFormat::F32 => 4,
            SampleFormat::F64 => 8,
        }
    }
}

/// The parts of a `WAVEFORMATEX(TENSIBLE)` the classifier needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawFormat {
    pub tag: u16,
    pub channels: u16,
    pub bits_per_sample: u16,
    pub block_align: u16,
    /// Only for `WAVE_FORMAT_EXTENSIBLE`.
    pub sub_format: Option<SubFormat>,
}

/// Map a device mix format to a [`SampleFormat`]. Errors are for logs only.
pub fn classify(f: RawFormat) -> Result<SampleFormat, String> {
    if f.channels == 0 {
        return Err("mix format has 0 channels".into());
    }
    let float = match f.tag {
        WAVE_FORMAT_PCM => false,
        WAVE_FORMAT_IEEE_FLOAT => true,
        WAVE_FORMAT_EXTENSIBLE => match f.sub_format {
            Some(SubFormat::Pcm) => false,
            Some(SubFormat::Float) => true,
            _ => return Err("unsupported extensible sub-format".into()),
        },
        tag => return Err(format!("unsupported format tag {tag:#06x}")),
    };
    let fmt = match (float, f.bits_per_sample) {
        (false, 8) => SampleFormat::U8,
        (false, 16) => SampleFormat::I16,
        (false, 24) => SampleFormat::I24,
        (false, 32) => SampleFormat::I32,
        (true, 32) => SampleFormat::F32,
        (true, 64) => SampleFormat::F64,
        (_, bits) => {
            return Err(format!(
                "unsupported {} sample width {bits}",
                if float { "float" } else { "PCM" }
            ))
        }
    };
    if usize::from(f.block_align) != fmt.bytes() * usize::from(f.channels) {
        return Err(format!(
            "block align {} does not match {} channels of {fmt:?}",
            f.block_align, f.channels
        ));
    }
    Ok(fmt)
}

/// Convert little-endian interleaved device bytes to f32 in -1..1, replacing
/// `out`'s contents. A trailing partial sample is ignored.
pub fn convert_to_f32(bytes: &[u8], fmt: SampleFormat, out: &mut Vec<f32>) {
    out.clear();
    let n = fmt.bytes();
    out.reserve(bytes.len() / n);
    let chunks = bytes.chunks_exact(n);
    match fmt {
        SampleFormat::U8 => out.extend(chunks.map(|b| (f32::from(b[0]) - 128.0) / 128.0)),
        SampleFormat::I16 => {
            out.extend(chunks.map(|b| f32::from(i16::from_le_bytes([b[0], b[1]])) / 32_768.0))
        }
        SampleFormat::I24 => out.extend(chunks.map(|b| {
            // Place the 3 bytes in the top of an i32 so the sign extends.
            let v = i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8;
            v as f32 / 8_388_608.0
        })),
        SampleFormat::I32 => out.extend(
            chunks.map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32 / 2_147_483_648.0),
        ),
        SampleFormat::F32 => {
            out.extend(chunks.map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])))
        }
        SampleFormat::F64 => {
            out.extend(chunks.map(|b| {
                f64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]) as f32
            }))
        }
    }
}

/// Whole frames covered by `d` at `rate` Hz (floor).
pub fn frames_in(d: Duration, rate: u32) -> u64 {
    (d.as_nanos() * u128::from(rate) / 1_000_000_000) as u64
}

/// No packets for this long counts as a gap (packets normally arrive every
/// device period, ~10 ms, and the poll runs every ~10-16 ms).
pub const GAP_THRESHOLD: Duration = Duration::from_millis(60);
/// At stop, a trailing packet-less stretch at least this long is filled even
/// though it has not reached [`GAP_THRESHOLD`] yet (packets normally arrive
/// every ~10 ms, so this long without one means nothing is playing).
pub const FINISH_THRESHOLD: Duration = Duration::from_millis(25);
/// Longest silence backlog emitted at once. A longer stall (system sleep, a
/// descheduled thread) skips the excess instead of flooding the session.
pub const MAX_SILENCE_BACKLOG: Duration = Duration::from_secs(1);

/// WASAPI loopback delivers NO packets while nothing is playing. The session
/// needs a steady stream (Deepgram must see audio time pass), so the capture
/// thread fills such gaps with silence, timed on the wall clock.
///
/// Each gap is measured from the last real packet on its own, so device vs
/// wall clock drift never accumulates and silence is never inserted while
/// packets are flowing.
#[derive(Debug, Clone)]
pub struct GapFiller {
    rate: u32,
    /// Time (since capture start) of the last real packet, or of the start.
    anchor: Duration,
    /// Silence frames already emitted (or skipped) since `anchor`.
    filled: u64,
    max_backlog: u64,
}

impl GapFiller {
    /// `start` is the capture start time on the caller's clock.
    pub fn new(rate: u32, start: Duration) -> Self {
        Self {
            rate,
            anchor: start,
            filled: 0,
            max_backlog: frames_in(MAX_SILENCE_BACKLOG, rate),
        }
    }

    /// A real (or device-flagged silent) packet arrived at `now`.
    pub fn on_packet(&mut self, now: Duration) {
        self.anchor = now;
        self.filled = 0;
    }

    /// Frames of silence to emit at `now` (0 while packets are flowing).
    pub fn due(&mut self, now: Duration) -> u64 {
        let since = now.saturating_sub(self.anchor);
        if since < GAP_THRESHOLD {
            return 0;
        }
        let target = frames_in(since, self.rate);
        let mut deficit = target.saturating_sub(self.filled);
        if deficit > self.max_backlog {
            self.filled += deficit - self.max_backlog;
            deficit = self.max_backlog;
        }
        self.filled += deficit;
        deficit
    }

    /// Frames of silence to emit when capture stops at `now`: like
    /// [`GapFiller::due`], but a trailing gap only needs [`FINISH_THRESHOLD`],
    /// so the last stretch before a stop is not dropped.
    pub fn finish(&mut self, now: Duration) -> u64 {
        let since = now.saturating_sub(self.anchor);
        if since < FINISH_THRESHOLD {
            return 0;
        }
        // Same bookkeeping as `due`, with the lower threshold and the same
        // backlog cap (any excess is skipped).
        let target = frames_in(since, self.rate);
        let deficit = target.saturating_sub(self.filled).min(self.max_backlog);
        self.filled = target;
        deficit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(tag: u16, ch: u16, bits: u16, sub: Option<SubFormat>) -> RawFormat {
        RawFormat {
            tag,
            channels: ch,
            bits_per_sample: bits,
            block_align: ch * bits / 8,
            sub_format: sub,
        }
    }

    #[test]
    fn classify_extensible_float_and_pcm() {
        let f = raw(WAVE_FORMAT_EXTENSIBLE, 2, 32, Some(SubFormat::Float));
        assert_eq!(classify(f), Ok(SampleFormat::F32));
        let f = raw(WAVE_FORMAT_EXTENSIBLE, 2, 24, Some(SubFormat::Pcm));
        assert_eq!(classify(f), Ok(SampleFormat::I24));
        let f = raw(WAVE_FORMAT_EXTENSIBLE, 6, 32, Some(SubFormat::Pcm));
        assert_eq!(classify(f), Ok(SampleFormat::I32));
        let f = raw(WAVE_FORMAT_EXTENSIBLE, 2, 64, Some(SubFormat::Float));
        assert_eq!(classify(f), Ok(SampleFormat::F64));
    }

    #[test]
    fn classify_plain_tags() {
        assert_eq!(
            classify(raw(WAVE_FORMAT_PCM, 2, 16, None)),
            Ok(SampleFormat::I16)
        );
        assert_eq!(
            classify(raw(WAVE_FORMAT_PCM, 1, 8, None)),
            Ok(SampleFormat::U8)
        );
        assert_eq!(
            classify(raw(WAVE_FORMAT_IEEE_FLOAT, 2, 32, None)),
            Ok(SampleFormat::F32)
        );
    }

    #[test]
    fn classify_rejects_unsupported() {
        assert!(classify(raw(WAVE_FORMAT_EXTENSIBLE, 2, 32, Some(SubFormat::Other))).is_err());
        assert!(classify(raw(WAVE_FORMAT_EXTENSIBLE, 2, 32, None)).is_err());
        assert!(classify(raw(0x0055, 2, 16, None)).is_err()); // MP3
        assert!(classify(raw(WAVE_FORMAT_IEEE_FLOAT, 2, 16, None)).is_err());
        assert!(classify(raw(WAVE_FORMAT_PCM, 2, 12, None)).is_err());
        assert!(classify(raw(WAVE_FORMAT_PCM, 0, 16, None)).is_err());
        let mut bad_align = raw(WAVE_FORMAT_PCM, 2, 16, None);
        bad_align.block_align = 6;
        assert!(classify(bad_align).is_err());
    }

    #[test]
    fn convert_f32_and_f64() {
        let mut out = Vec::new();
        let bytes: Vec<u8> = [0.5f32, -1.0, 0.25]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        convert_to_f32(&bytes, SampleFormat::F32, &mut out);
        assert_eq!(out, vec![0.5, -1.0, 0.25]);
        let bytes: Vec<u8> = [0.75f64, -0.5]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        convert_to_f32(&bytes, SampleFormat::F64, &mut out);
        assert_eq!(out, vec![0.75, -0.5]);
    }

    #[test]
    fn convert_integer_formats_to_unit_range() {
        let mut out = Vec::new();
        convert_to_f32(&[0, 128, 192], SampleFormat::U8, &mut out);
        assert_eq!(out, vec![-1.0, 0.0, 0.5]);
        let bytes: Vec<u8> = [i16::MIN, 0, 16_384]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        convert_to_f32(&bytes, SampleFormat::I16, &mut out);
        assert_eq!(out, vec![-1.0, 0.0, 0.5]);
        let bytes: Vec<u8> = [i32::MIN, 0, 1 << 30]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        convert_to_f32(&bytes, SampleFormat::I32, &mut out);
        assert_eq!(out, vec![-1.0, 0.0, 0.5]);
    }

    #[test]
    fn convert_24bit_packed_sign_extends() {
        let mut out = Vec::new();
        // Little-endian 3-byte samples: -2^23, 0, +2^22, -1, max.
        let bytes = [
            0x00, 0x00, 0x80, // -8388608
            0x00, 0x00, 0x00, // 0
            0x00, 0x00, 0x40, // 4194304
            0xFF, 0xFF, 0xFF, // -1
            0xFF, 0xFF, 0x7F, // 8388607
        ];
        convert_to_f32(&bytes, SampleFormat::I24, &mut out);
        assert_eq!(out[0], -1.0);
        assert_eq!(out[1], 0.0);
        assert_eq!(out[2], 0.5);
        assert_eq!(out[3], -1.0 / 8_388_608.0);
        assert!((out[4] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn convert_ignores_trailing_partial_sample_and_replaces_output() {
        let mut out = vec![9.0; 10];
        convert_to_f32(&[0, 0x40, 0xAA], SampleFormat::I16, &mut out);
        assert_eq!(out, vec![0.5]);
        convert_to_f32(&[], SampleFormat::F32, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn frames_in_is_floor_and_exact() {
        assert_eq!(frames_in(Duration::from_secs(1), 48_000), 48_000);
        assert_eq!(frames_in(Duration::from_millis(10), 44_100), 441);
        assert_eq!(frames_in(Duration::from_micros(20), 48_000), 0);
        assert_eq!(frames_in(Duration::from_secs(3600), 192_000), 691_200_000);
    }

    #[test]
    fn gap_filler_is_silent_while_packets_flow() {
        let mut g = GapFiller::new(48_000, Duration::ZERO);
        for ms in (10..2_000).step_by(10) {
            let now = Duration::from_millis(ms);
            assert_eq!(g.due(now), 0, "at {ms} ms");
            g.on_packet(now);
        }
    }

    #[test]
    fn gap_filler_emits_wall_clock_silence_without_packets() {
        let mut g = GapFiller::new(48_000, Duration::from_secs(5));
        assert_eq!(g.due(Duration::from_millis(5_050)), 0, "below threshold");
        // Crossing the threshold fills the whole gap since the anchor.
        assert_eq!(g.due(Duration::from_millis(5_070)), 3_360);
        let mut total = 3_360;
        for ms in (5_080..=8_000).step_by(13) {
            total += g.due(Duration::from_millis(ms));
        }
        total += g.due(Duration::from_millis(8_000));
        assert_eq!(total, 3 * 48_000, "exactly 3 s of silence for 3 s of gap");
        // Asking twice at the same instant emits nothing more.
        assert_eq!(g.due(Duration::from_millis(8_000)), 0);
    }

    #[test]
    fn gap_filler_restarts_per_gap_after_packets_resume() {
        let mut g = GapFiller::new(16_000, Duration::ZERO);
        assert_eq!(g.due(Duration::from_millis(500)), 8_000);
        g.on_packet(Duration::from_millis(510));
        assert_eq!(g.due(Duration::from_millis(550)), 0);
        // A new gap is measured from the last packet only.
        assert_eq!(g.due(Duration::from_millis(610)), 1_600);
    }

    #[test]
    fn gap_filler_finish_fills_trailing_gap_below_the_threshold() {
        let mut g = GapFiller::new(48_000, Duration::ZERO);
        g.on_packet(Duration::from_millis(1_000));
        // 10 ms after a packet: still flowing, nothing to add.
        assert_eq!(g.finish(Duration::from_millis(1_010)), 0);
        // 40 ms without packets at stop: below GAP_THRESHOLD but filled.
        assert_eq!(g.finish(Duration::from_millis(1_040)), 1_920);
        // Mid-gap stop only adds what was not emitted yet.
        let mut g = GapFiller::new(16_000, Duration::ZERO);
        assert_eq!(g.due(Duration::from_millis(100)), 1_600);
        assert_eq!(g.finish(Duration::from_millis(112)), 192);
        assert_eq!(g.finish(Duration::from_millis(112)), 0);
    }

    #[test]
    fn gap_filler_caps_backlog_after_a_long_stall() {
        let mut g = GapFiller::new(48_000, Duration::ZERO);
        // e.g. the machine slept for an hour.
        assert_eq!(g.due(Duration::from_secs(3_600)), 48_000);
        // The skipped excess is not owed later.
        assert_eq!(g.due(Duration::from_millis(3_600_010)), 480);
    }
}
