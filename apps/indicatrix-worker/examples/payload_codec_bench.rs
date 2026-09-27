//! Measures lossless codecs on captured `FRAME` payloads.
//!
//! For every capture written by `capture_frame_payloads` this reports the compression
//! ratio and single-core encode/decode throughput (MB/s = 10^6 bytes of RAW payload per
//! second, on the calling thread only) for:
//!
//! - `Raw`: a plain copy (the emitter itself is zero-copy; shown for scale),
//! - `ShuffleLz4`: byte shuffle + `lz4_flex` block format,
//! - `ShuffleZstd1` / `ShuffleZstd3`: byte shuffle + zstd at level 1 / 3,
//! - `Lz4` / `Zstd1` without the shuffle, to show what the shuffle buys.
//!
//! Every codec's decode path is the bounded one production would use: the output buffer
//! is exactly `raw_len = w * h * 12` bytes and anything that does not decode to exactly
//! that length is rejected. Round trips are checked BIT-EXACT (byte equality) on every
//! capture, and synthetic checks cover NaN payloads, signed zeros, infinities and
//! subnormals plus a crafted decompression bomb per codec.
//!
//! ```text
//! cargo run -p indicatrix-worker --profile probe --example payload_codec_bench -- <dir>
//! ```

use std::{error::Error, fmt::Write as _, path::Path, time::Instant};

/// Bytes per f32 value, i.e. the number of byte planes the shuffle produces.
const PLANES: usize = 4;

/// Bytes per pixel on the wire (three f32 XYZ sums).
const BYTES_PER_PIXEL: usize = 12;

/// Minimum wall time spent timing one direction of one codec on one capture.
const MIN_TIMING_SECS: f64 = 0.4;

/// Upper bound on timing repetitions for tiny inputs.
const MAX_REPS: usize = 25;

/// Byte-shuffles `src` (a sequence of little-endian f32 values) into `dst`: all byte 0s,
/// then all byte 1s, then all byte 2s, then all byte 3s.
///
/// # Panics
///
/// Panics if `src.len()` is not a multiple of 4 or `dst.len() != src.len()`.
fn shuffle_into(src: &[u8], dst: &mut [u8]) {
    assert_eq!(
        src.len() % PLANES,
        0,
        "payload is not a whole number of f32 values"
    );
    assert_eq!(
        src.len(),
        dst.len(),
        "shuffle output must match the input length"
    );
    let n = src.len() / PLANES;
    let (p0, rest) = dst.split_at_mut(n);
    let (p1, rest) = rest.split_at_mut(n);
    let (p2, p3) = rest.split_at_mut(n);
    for (i, v) in src.as_chunks::<PLANES>().0.iter().enumerate() {
        p0[i] = v[0];
        p1[i] = v[1];
        p2[i] = v[2];
        p3[i] = v[3];
    }
}

/// The inverse of [`shuffle_into`]: re-interleaves 4 byte planes into f32 values.
///
/// # Panics
///
/// Panics if `src.len()` is not a multiple of 4 or `dst.len() != src.len()`.
fn unshuffle_into(src: &[u8], dst: &mut [u8]) {
    assert_eq!(
        src.len() % PLANES,
        0,
        "payload is not a whole number of f32 values"
    );
    assert_eq!(
        src.len(),
        dst.len(),
        "unshuffle output must match the input length"
    );
    let n = src.len() / PLANES;
    let (p0, rest) = src.split_at(n);
    let (p1, rest) = rest.split_at(n);
    let (p2, p3) = rest.split_at(n);
    for (i, v) in dst.as_chunks_mut::<PLANES>().0.iter_mut().enumerate() {
        v[0] = p0[i];
        v[1] = p1[i];
        v[2] = p2[i];
        v[3] = p3[i];
    }
}

/// The candidate `PayloadEncoding`s (plus two unshuffled references).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Codec {
    /// Plain copy.
    Raw,
    /// Byte shuffle + LZ4 block.
    ShuffleLz4,
    /// Byte shuffle + zstd at the given level.
    ShuffleZstd(i32),
    /// LZ4 block without the shuffle.
    Lz4,
    /// zstd at the given level without the shuffle.
    Zstd(i32),
}

impl Codec {
    /// Every codec the bench reports, in table order.
    const ALL: [Self; 6] = [
        Self::Raw,
        Self::ShuffleLz4,
        Self::ShuffleZstd(1),
        Self::ShuffleZstd(3),
        Self::Lz4,
        Self::Zstd(1),
    ];

    /// Column label.
    fn label(self) -> String {
        match self {
            Self::Raw => "Raw".to_string(),
            Self::ShuffleLz4 => "ShuffleLz4".to_string(),
            Self::ShuffleZstd(l) => format!("ShuffleZstd{l}"),
            Self::Lz4 => "Lz4".to_string(),
            Self::Zstd(l) => format!("Zstd{l}"),
        }
    }

    /// Whether the byte shuffle runs before compression.
    const fn shuffled(self) -> bool {
        matches!(self, Self::ShuffleLz4 | Self::ShuffleZstd(_))
    }
}

/// Why a bounded decode was refused.
#[derive(Debug)]
enum DecodeError {
    /// The header's `raw_len` disagrees with `width * height * 12`.
    RawLenMismatch { raw_len: usize, expected: usize },
    /// The codec reported an error (includes output overflow).
    Codec(String),
    /// The payload decoded to fewer bytes than `raw_len`.
    Short { got: usize, raw_len: usize },
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RawLenMismatch { raw_len, expected } => {
                write!(f, "raw_len {raw_len} != w*h*12 = {expected}")
            }
            Self::Codec(msg) => write!(f, "codec error: {msg}"),
            Self::Short { got, raw_len } => write!(f, "decoded {got} B, raw_len {raw_len}"),
        }
    }
}

/// Reusable buffers so the timed loops measure the codec, not the allocator.
struct Scratch {
    /// Shuffled planes (encode) or decompressed planes (decode).
    planes: Vec<u8>,
    /// Compressed bytes.
    wire: Vec<u8>,
    /// Length of the valid prefix of `wire`.
    wire_len: usize,
    /// Decoded output, exactly `raw_len` bytes.
    out: Vec<u8>,
}

impl Scratch {
    /// Buffers sized for a `raw_len`-byte payload under any codec.
    fn new(raw_len: usize) -> Self {
        let bound = lz4_flex::block::get_maximum_output_size(raw_len)
            .max(zstd::zstd_safe::compress_bound(raw_len));
        Self {
            planes: vec![0; raw_len],
            wire: vec![0; bound],
            wire_len: 0,
            out: vec![0; raw_len],
        }
    }
}

/// Compresses `raw` into `s.wire` (setting `s.wire_len`).
fn encode(codec: Codec, raw: &[u8], s: &mut Scratch) -> Result<(), Box<dyn Error>> {
    let input: &[u8] = if codec.shuffled() {
        shuffle_into(raw, &mut s.planes);
        &s.planes
    } else {
        raw
    };
    s.wire_len = match codec {
        Codec::Raw => {
            s.wire[..raw.len()].copy_from_slice(raw);
            raw.len()
        }
        Codec::ShuffleLz4 | Codec::Lz4 => lz4_flex::block::compress_into(input, &mut s.wire)?,
        Codec::ShuffleZstd(level) | Codec::Zstd(level) => {
            zstd::bulk::Compressor::new(level)?.compress_to_buffer(input, s.wire.as_mut_slice())?
        }
    };
    Ok(())
}

/// Decompresses `wire` into exactly `raw_len` bytes of `out`, never more.
fn decompress_bounded(codec: Codec, wire: &[u8], out: &mut [u8]) -> Result<usize, DecodeError> {
    match codec {
        Codec::Raw => {
            if wire.len() > out.len() {
                return Err(DecodeError::Codec("raw payload longer than raw_len".into()));
            }
            out[..wire.len()].copy_from_slice(wire);
            Ok(wire.len())
        }
        Codec::ShuffleLz4 | Codec::Lz4 => lz4_flex::block::decompress_into(wire, out)
            .map_err(|e| DecodeError::Codec(e.to_string())),
        Codec::ShuffleZstd(_) | Codec::Zstd(_) => zstd::bulk::Decompressor::new()
            .and_then(|mut d| d.decompress_to_buffer(wire, out))
            .map_err(|e| DecodeError::Codec(e.to_string())),
    }
}

/// The production-shaped bounded decode: validates `raw_len` against the frame size,
/// decodes into a buffer of exactly `raw_len`, rejects short output, then unshuffles.
fn bounded_decode(
    codec: Codec,
    wire: &[u8],
    pixels: usize,
    raw_len: usize,
    s: &mut Scratch,
) -> Result<(), DecodeError> {
    let expected = pixels * BYTES_PER_PIXEL;
    if raw_len != expected {
        return Err(DecodeError::RawLenMismatch { raw_len, expected });
    }
    let target = if codec.shuffled() {
        &mut s.planes[..raw_len]
    } else {
        &mut s.out[..raw_len]
    };
    let got = decompress_bounded(codec, wire, target)?;
    if got != raw_len {
        return Err(DecodeError::Short { got, raw_len });
    }
    if codec.shuffled() {
        unshuffle_into(&s.planes[..raw_len], &mut s.out[..raw_len]);
    }
    Ok(())
}

/// Runs `f` repeatedly (at least 3 times, until [`MIN_TIMING_SECS`] or [`MAX_REPS`])
/// and returns the median seconds per run.
fn median_secs(mut f: impl FnMut() -> Result<(), Box<dyn Error>>) -> Result<f64, Box<dyn Error>> {
    let mut times = Vec::new();
    let budget = Instant::now();
    while times.len() < 3
        || (times.len() < MAX_REPS && budget.elapsed().as_secs_f64() < MIN_TIMING_SECS)
    {
        let t = Instant::now();
        f()?;
        times.push(t.elapsed().as_secs_f64());
    }
    times.sort_by(f64::total_cmp);
    Ok(times[times.len() / 2])
}

/// One codec's result on one capture.
struct Measurement {
    /// raw bytes / wire bytes.
    ratio: f64,
    /// Encode throughput in MB/s of raw payload.
    encode_mbs: f64,
    /// Decode throughput in MB/s of raw payload.
    decode_mbs: f64,
}

/// Measures one codec on one capture and checks the round trip is bit-exact.
fn measure(
    codec: Codec,
    raw: &[u8],
    pixels: usize,
    s: &mut Scratch,
) -> Result<Measurement, Box<dyn Error>> {
    let mb = raw.len() as f64 / 1e6;
    let enc = median_secs(|| encode(codec, raw, s))?;
    let wire = s.wire[..s.wire_len].to_vec();
    let dec = median_secs(|| {
        bounded_decode(codec, &wire, pixels, raw.len(), s).map_err(|e| e.to_string().into())
    })?;
    if s.out[..raw.len()] != *raw {
        return Err(format!("{} round trip is NOT bit-exact", codec.label()).into());
    }
    Ok(Measurement {
        ratio: raw.len() as f64 / wire.len() as f64,
        encode_mbs: mb / enc,
        decode_mbs: mb / dec,
    })
}

/// Parses `<scene>-<material>-<w>x<h>-spp<n>.xyz` into its parts.
fn parse_name(name: &str) -> Option<(String, String, usize, usize, u32)> {
    let stem = name.strip_suffix(".xyz")?;
    let mut parts = stem.split('-');
    let scene = parts.next()?.to_string();
    let material = parts.next()?.to_string();
    let (w, h) = parts.next()?.split_once('x')?;
    let spp = parts.next()?.strip_prefix("spp")?.parse().ok()?;
    Some((scene, material, w.parse().ok()?, h.parse().ok()?, spp))
}

/// Fraction of f32 values in `raw` that are exactly +0.0.
fn zero_fraction(raw: &[u8]) -> f64 {
    let zeros = raw
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|v| **v == [0, 0, 0, 0])
        .count();
    zeros as f64 / (raw.len() / 4) as f64
}

/// Benchmarks one capture file and prints its table row(s).
fn bench_file(path: &Path) -> Result<(), Box<dyn Error>> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let Some((scene, material, w, h, spp)) = parse_name(name) else {
        return Ok(());
    };
    let raw = std::fs::read(path)?;
    let pixels = w * h;
    if raw.len() != pixels * BYTES_PER_PIXEL {
        return Err(format!(
            "{name}: {} bytes, expected {}",
            raw.len(),
            pixels * BYTES_PER_PIXEL
        )
        .into());
    }
    let mut s = Scratch::new(raw.len());
    let mut row = format!(
        "| {scene} | {material} | {h}p | {spp} | {:.1}% |",
        100.0 * zero_fraction(&raw)
    );
    for codec in Codec::ALL {
        let m = measure(codec, &raw, pixels, &mut s)?;
        write!(
            row,
            " {:.3} / {:.0} / {:.0} |",
            m.ratio, m.encode_mbs, m.decode_mbs
        )?;
    }
    println!("{row}");
    Ok(())
}

/// Synthetic bit-exactness check over special f32 values, for every codec.
fn synthetic_round_trip() -> Result<(), Box<dyn Error>> {
    let specials: [u32; 12] = [
        0x0000_0000, // +0
        0x8000_0000, // -0
        0x7fc0_0000, // canonical quiet NaN
        0x7fc0_1234, // quiet NaN with payload
        0xffc5_a5a5, // negative quiet NaN with payload
        0x7f80_0001, // signalling NaN
        0xff80_0001, // negative signalling NaN
        0x7f80_0000, // +inf
        0xff80_0000, // -inf
        0x0000_0001, // smallest subnormal
        0x807f_ffff, // largest negative subnormal
        0x3f80_0000, // 1.0
    ];
    let mut state = 0x9e37_79b9_u32;
    let values: Vec<u32> = (0..3 * 997)
        .map(|i| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            if i % 3 == 0 {
                specials[i % specials.len()]
            } else {
                state
            }
        })
        .collect();
    let raw: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();

    let mut shuffled = vec![0; raw.len()];
    let mut back = vec![0; raw.len()];
    shuffle_into(&raw, &mut shuffled);
    unshuffle_into(&shuffled, &mut back);
    if back != raw {
        return Err("synthetic shuffle/unshuffle is NOT bit-exact".into());
    }
    let mut s = Scratch::new(raw.len());
    for codec in Codec::ALL {
        measure(codec, &raw, raw.len() / BYTES_PER_PIXEL, &mut s)?;
    }
    println!(
        "synthetic: {} values incl. +-0, NaN payloads (quiet/signalling, both signs), +-inf, \
         subnormals: shuffle and all {} codecs round-trip bit-exact",
        values.len(),
        Codec::ALL.len()
    );
    Ok(())
}

/// Crafted bombs: a tiny payload that expands to 64x `raw_len`, a short payload, and a
/// lying `raw_len`. Every one must be rejected by [`bounded_decode`].
fn bomb_checks() -> Result<(), Box<dyn Error>> {
    let pixels = 256 * 256;
    let raw_len = pixels * BYTES_PER_PIXEL;
    let huge = vec![0_u8; raw_len * 64];
    let short = vec![0_u8; raw_len / 2];
    let mut s = Scratch::new(raw_len);
    for codec in Codec::ALL {
        let mut cases = Vec::new();
        for (what, input) in [("64x expansion", &huge), ("half-length", &short)] {
            let mut big = Scratch::new(input.len());
            encode(codec, input, &mut big)?;
            let wire = &big.wire[..big.wire_len];
            let result = bounded_decode(codec, wire, pixels, raw_len, &mut s);
            let Err(e) = result else {
                return Err(format!("{}: {what} bomb was ACCEPTED", codec.label()).into());
            };
            cases.push(format!("{what} ({} B wire) -> {e}", wire.len()));
        }
        let lying = bounded_decode(codec, &[], pixels, raw_len * 64, &mut s);
        let Err(e) = lying else {
            return Err(format!("{}: lying raw_len was ACCEPTED", codec.label()).into());
        };
        cases.push(format!("lying raw_len -> {e}"));
        println!("bomb {}: rejected: {}", codec.label(), cases.join("; "));
    }
    Ok(())
}

/// Entry point: `payload_codec_bench <capture-dir>`.
fn main() -> Result<(), Box<dyn Error>> {
    let dir = std::env::args()
        .nth(1)
        .ok_or("usage: payload_codec_bench <capture-dir>")?;
    synthetic_round_trip()?;
    bomb_checks()?;

    let mut files: Vec<_> = std::fs::read_dir(&dir)?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "xyz"))
        .collect();
    files.sort();

    let header: Vec<String> = Codec::ALL.into_iter().map(Codec::label).collect();
    println!("cells: ratio / encode MB/s / decode MB/s (single core, MB = 1e6 B of raw payload)");
    println!(
        "| scene | material | size | spp | zero f32 | {} |",
        header.join(" | ")
    );
    println!("|---|---|---|---|---|{}", "---|".repeat(header.len()));
    for path in &files {
        bench_file(path)?;
    }
    println!(
        "all {} captures round-tripped bit-exact under every codec",
        files.len()
    );
    Ok(())
}
