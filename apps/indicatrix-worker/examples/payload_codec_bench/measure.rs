//! Codec candidates and the per-candidate measurement (production codecs only).
//!
//! Radiance payloads go through `indicatrix_net::radiance::{PayloadEncoder, PayloadDecoder}`
//! exactly as the emitter and an accumulating client use them; 8-bit display frames go
//! through `indicatrix_net::display`. The encoder reuses its compression context across
//! calls (as an emitter does), and the timed decode is `decode_and_add`, the path that
//! sums a delta into the accumulator.

use indicatrix_net::{
    display::{decode_rgba8, encode_rgba8},
    messages::{DisplayEncoding, PayloadEncoding},
    radiance::{PayloadDecoder, PayloadEncoder, as_bytes},
};
use std::{error::Error, hint::black_box, time::Instant};

/// Which wire message a candidate encodes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A `FRAME`/`PREVIEW` radiance payload (`f32` x 3 per pixel).
    Payload,
    /// A `DISPLAY_FRAME` (8-bit RGBA).
    Display,
}

impl Kind {
    /// Short stable name used in the table and the CSV.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Payload => "frame",
            Self::Display => "display",
        }
    }
}

/// One codec setting to measure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Candidate {
    /// A radiance payload encoding.
    Payload(PayloadEncoding),
    /// A display-frame encoding.
    Display(DisplayEncoding),
}

impl Candidate {
    /// The radiance candidates for a sweep over `levels` of zstd, in report order.
    pub fn payload_sweep(levels: &[u8]) -> Vec<Self> {
        let mut out = vec![
            Self::Payload(PayloadEncoding::Raw),
            Self::Payload(PayloadEncoding::ShuffleLz4),
        ];
        out.extend(
            levels
                .iter()
                .map(|&level| Self::Payload(PayloadEncoding::ShuffleZstd { level })),
        );
        out
    }

    /// The display candidates (raw RGBA8 for scale, then PNG).
    pub const DISPLAY: [Self; 2] = [
        Self::Display(DisplayEncoding::Rgba8),
        Self::Display(DisplayEncoding::Png),
    ];

    /// The message kind.
    pub const fn kind(self) -> Kind {
        match self {
            Self::Payload(_) => Kind::Payload,
            Self::Display(_) => Kind::Display,
        }
    }

    /// Codec family name (levels excluded).
    pub const fn family(self) -> &'static str {
        match self {
            Self::Payload(PayloadEncoding::Raw) => "Raw",
            Self::Payload(PayloadEncoding::ShuffleLz4) => "ShuffleLz4",
            Self::Payload(PayloadEncoding::ShuffleZstd { .. }) => "ShuffleZstd",
            Self::Display(DisplayEncoding::Rgba8) => "Rgba8",
            Self::Display(DisplayEncoding::Png) => "Png",
        }
    }

    /// The zstd level, or 0 for codecs without one.
    pub const fn level(self) -> u32 {
        match self {
            Self::Payload(PayloadEncoding::ShuffleZstd { level }) => level as u32,
            _ => 0,
        }
    }
}

/// One measured (data set, size, codec, level) cell.
#[derive(Clone, Debug)]
pub struct Record {
    /// Data set name.
    pub set: &'static str,
    /// Frame width in pixels.
    pub width: usize,
    /// Frame height in pixels.
    pub height: usize,
    /// The message kind.
    pub kind: Kind,
    /// Codec family.
    pub family: &'static str,
    /// zstd level, 0 when not applicable.
    pub level: u32,
    /// What the encoder actually sent (it falls back to `Raw` when compressing does not
    /// shrink a payload).
    pub sent_as: String,
    /// Uncompressed input size in bytes.
    pub raw_bytes: usize,
    /// Encoded size in bytes.
    pub wire_bytes: usize,
    /// Median compress seconds per frame (single thread).
    pub compress_s: f64,
    /// Median decompress seconds per frame (single thread).
    pub decompress_s: f64,
    /// Repetitions behind the medians.
    pub reps: usize,
}

impl Record {
    /// Raw bytes over encoded bytes.
    pub fn ratio(&self) -> f64 {
        self.raw_bytes as f64 / self.wire_bytes.max(1) as f64
    }

    /// Compress throughput in MB/s (10^6 bytes) of raw input.
    pub fn compress_mbs(&self) -> f64 {
        self.raw_bytes as f64 / 1e6 / self.compress_s
    }

    /// Decompress throughput in MB/s (10^6 bytes) of raw output.
    pub fn decompress_mbs(&self) -> f64 {
        self.raw_bytes as f64 / 1e6 / self.decompress_s
    }

    /// Label such as `ShuffleZstd/9`.
    pub fn label(&self) -> String {
        if self.family == "ShuffleZstd" {
            format!("{}/{}", self.family, self.level)
        } else {
            self.family.to_string()
        }
    }
}

/// Smallest time reported, so a zero-copy path never divides by zero.
const MIN_SECS: f64 = 1.0e-9;

/// Runs `f` once as a warm-up, then `reps` times, and returns the median seconds.
fn median_secs(
    reps: usize,
    mut f: impl FnMut() -> Result<(), Box<dyn Error>>,
) -> Result<f64, Box<dyn Error>> {
    f()?;
    let mut times = Vec::with_capacity(reps);
    for _ in 0..reps.max(1) {
        let t = Instant::now();
        f()?;
        times.push(t.elapsed().as_secs_f64());
    }
    times.sort_by(f64::total_cmp);
    Ok(times[times.len() / 2].max(MIN_SECS))
}

/// Identity of the frame being measured.
#[derive(Clone, Copy)]
pub struct Frame<'a> {
    /// Data set name.
    pub set: &'static str,
    /// Width in pixels.
    pub width: usize,
    /// Height in pixels.
    pub height: usize,
    /// The bytes to encode (radiance `f32` bytes, or RGBA8 for display candidates).
    pub raw: &'a [u8],
}

/// Measures one candidate on one frame; fails loudly unless decode(encode(x)) == x bit for bit.
///
/// # Errors
///
/// Any codec error, or a round trip that is not bit-identical.
pub fn measure(
    candidate: Candidate,
    frame: Frame<'_>,
    reps: usize,
) -> Result<Record, Box<dyn Error>> {
    let (wire_bytes, sent_as, compress_s, decompress_s) = match candidate {
        Candidate::Payload(enc) => measure_payload(enc, frame, reps)?,
        Candidate::Display(enc) => measure_display(enc, frame, reps)?,
    };
    Ok(Record {
        set: frame.set,
        width: frame.width,
        height: frame.height,
        kind: candidate.kind(),
        family: candidate.family(),
        level: candidate.level(),
        sent_as,
        raw_bytes: frame.raw.len(),
        wire_bytes,
        compress_s,
        decompress_s,
        reps,
    })
}

/// `(wire bytes, sent-as label, compress s, decompress s)`.
type Timings = (usize, String, f64, f64);

/// Measures a radiance payload encoding with the production encoder and decoder.
fn measure_payload(
    enc: PayloadEncoding,
    frame: Frame<'_>,
    reps: usize,
) -> Result<Timings, Box<dyn Error>> {
    let (w, h) = (frame.width as u32, frame.height as u32);
    let raw = frame.raw;
    let mut encoder = PayloadEncoder::new(enc);
    let compress_s = median_secs(reps, || {
        black_box(encoder.encode(black_box(raw)).bytes.len());
        Ok(())
    })?;
    let sent = encoder.encode(raw);
    let (sent_enc, raw_len, wire) = (sent.encoding, sent.raw_len, sent.bytes.to_vec());

    let mut decoder = PayloadDecoder::new();
    let decoded = decoder
        .decode_to_vec(sent_enc, raw_len, &wire, w, h)
        .map_err(|e| e.to_string())?;
    if as_bytes(&decoded) != raw {
        return Err(format!(
            "{enc:?} on {}x{} {}: round trip is NOT bit-identical",
            frame.width, frame.height, frame.set
        )
        .into());
    }
    let mut acc = decoded;
    let decompress_s = median_secs(reps, || {
        let dropped = decoder
            .decode_and_add(sent_enc, raw_len, black_box(&wire), w, h, &mut acc)
            .map_err(|e| e.to_string())?;
        black_box(dropped);
        Ok(())
    })?;
    Ok((
        wire.len(),
        format!("{sent_enc:?}"),
        compress_s,
        decompress_s,
    ))
}

/// Measures a display-frame encoding with the production display codec.
fn measure_display(
    enc: DisplayEncoding,
    frame: Frame<'_>,
    reps: usize,
) -> Result<Timings, Box<dyn Error>> {
    let (w, h) = (frame.width as u32, frame.height as u32);
    let rgba = frame.raw;
    let mut wire = Vec::new();
    let compress_s = median_secs(reps, || {
        wire = encode_rgba8(enc, w, h, black_box(rgba)).map_err(|e| e.to_string())?;
        Ok(())
    })?;
    let back = decode_rgba8(enc, w, h, &wire).map_err(|e| e.to_string())?;
    if back != rgba {
        return Err(format!(
            "{enc:?} on {}x{} {}: display round trip is NOT bit-identical",
            frame.width, frame.height, frame.set
        )
        .into());
    }
    let decompress_s = median_secs(reps, || {
        black_box(decode_rgba8(enc, w, h, black_box(&wire)).map_err(|e| e.to_string())?);
        Ok(())
    })?;
    Ok((wire.len(), format!("{enc:?}"), compress_s, decompress_s))
}
