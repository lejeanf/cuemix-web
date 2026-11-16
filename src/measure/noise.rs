//! Test noise: 90 s of pink noise as a 48 kHz / 16-bit / stereo WAV, generated
//! on the fly (a 17 MB file is never held in memory).
//!
//! White noise from a seeded xorshift64* (same bytes on every request) through
//! Paul Kellet's refined pink filter, scaled to about -20 dBFS RMS. Both
//! channels carry the same signal.

use std::convert::Infallible;

use axum::body::{Body, Bytes};
use axum::http::{header, HeaderValue};
use axum::response::Response;

pub const SAMPLE_RATE: u32 = 48_000;
pub const SECONDS: u32 = 90;
const CHANNELS: u16 = 2;
const BITS: u16 = 16;
const FRAMES: u32 = SAMPLE_RATE * SECONDS;
const BLOCK_ALIGN: u32 = CHANNELS as u32 * BITS as u32 / 8;
const DATA_BYTES: u32 = FRAMES * BLOCK_ALIGN;
const HEADER_BYTES: usize = 44;
/// Frames per streamed chunk (100 ms).
const CHUNK_FRAMES: u32 = SAMPLE_RATE / 10;
/// Scale applied to Kellet's output so that the RMS lands at -20 dBFS (checked
/// by the level test).
const GAIN: f64 = 0.0567;

/// Total size of the file in bytes (header + samples).
pub const fn total_bytes() -> u64 {
    HEADER_BYTES as u64 + DATA_BYTES as u64
}

/// RIFF/WAVE header of the whole file.
pub fn wav_header() -> [u8; HEADER_BYTES] {
    let mut h = [0u8; HEADER_BYTES];
    let mut put = |at: usize, bytes: &[u8]| h[at..at + bytes.len()].copy_from_slice(bytes);
    put(0, b"RIFF");
    put(4, &(36 + DATA_BYTES).to_le_bytes());
    put(8, b"WAVEfmt ");
    put(16, &16u32.to_le_bytes());
    put(20, &1u16.to_le_bytes()); // PCM
    put(22, &CHANNELS.to_le_bytes());
    put(24, &SAMPLE_RATE.to_le_bytes());
    put(28, &(SAMPLE_RATE * BLOCK_ALIGN).to_le_bytes());
    put(32, &(BLOCK_ALIGN as u16).to_le_bytes());
    put(34, &BITS.to_le_bytes());
    put(36, b"data");
    put(40, &DATA_BYTES.to_le_bytes());
    h
}

/// Pink noise source: sample by sample, deterministic.
#[derive(Debug, Clone)]
struct Pink {
    rng: u64,
    b: [f64; 7],
}

impl Pink {
    fn new() -> Self {
        Self {
            rng: 0x9E37_79B9_7F4A_7C15,
            b: [0.0; 7],
        }
    }

    /// Uniform white noise in `[-1, 1)` (xorshift64*).
    fn white(&mut self) -> f64 {
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        let x = self.rng.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11; // 53 bits
        x as f64 / (1u64 << 52) as f64 - 1.0
    }

    /// Next pink sample (Paul Kellet, refined method), scaled by [`GAIN`].
    fn next(&mut self) -> f64 {
        let w = self.white();
        let b = &mut self.b;
        b[0] = 0.99886 * b[0] + w * 0.055_517_9;
        b[1] = 0.99332 * b[1] + w * 0.075_075_9;
        b[2] = 0.96900 * b[2] + w * 0.153_852_0;
        b[3] = 0.86650 * b[3] + w * 0.310_485_6;
        b[4] = 0.55000 * b[4] + w * 0.532_952_2;
        b[5] = -0.7616 * b[5] - w * 0.016_898_0;
        let pink = b[0] + b[1] + b[2] + b[3] + b[4] + b[5] + b[6] + w * 0.5362;
        b[6] = w * 0.115_926;
        pink * GAIN
    }
}

/// The whole body, one chunk at a time.
#[derive(Debug)]
struct Chunks {
    pink: Pink,
    frames_left: u32,
    header_sent: bool,
}

impl Chunks {
    fn new() -> Self {
        Self {
            pink: Pink::new(),
            frames_left: FRAMES,
            header_sent: false,
        }
    }

    fn next_chunk(&mut self) -> Option<Vec<u8>> {
        if !self.header_sent {
            self.header_sent = true;
            return Some(wav_header().to_vec());
        }
        if self.frames_left == 0 {
            return None;
        }
        let frames = self.frames_left.min(CHUNK_FRAMES);
        self.frames_left -= frames;
        let mut out = Vec::with_capacity(frames as usize * BLOCK_ALIGN as usize);
        for _ in 0..frames {
            // Clamped, then an exact i16 in range.
            let s = (self.pink.next() * 32768.0).clamp(-32768.0, 32767.0) as i16;
            let le = s.to_le_bytes();
            out.extend_from_slice(&le);
            out.extend_from_slice(&le);
        }
        Some(out)
    }
}

/// `GET /api/measure/noise.wav`
pub async fn handler() -> Response {
    let stream = futures_util::stream::unfold(Chunks::new(), |mut c| async move {
        c.next_chunk()
            .map(|chunk| (Ok::<_, Infallible>(Bytes::from(chunk)), c))
    });
    let mut resp = Response::new(Body::from_stream(stream));
    let h = resp.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("audio/wav"));
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(total_bytes()));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("none"));
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_samples() -> Vec<i16> {
        let mut c = Chunks::new();
        let mut bytes = Vec::new();
        let mut chunks = 0;
        while let Some(chunk) = c.next_chunk() {
            assert!(
                chunk.len() <= CHUNK_FRAMES as usize * 4 + HEADER_BYTES,
                "bounded chunks"
            );
            bytes.extend(chunk);
            chunks += 1;
        }
        assert!(chunks > 100, "streamed in many chunks");
        assert_eq!(bytes.len() as u64, total_bytes());
        bytes[HEADER_BYTES..]
            .chunks_exact(4)
            .map(|f| i16::from_le_bytes([f[0], f[1]]))
            .collect()
    }

    #[test]
    fn header_is_a_valid_wav() {
        let h = wav_header();
        let u32_at = |i: usize| u32::from_le_bytes([h[i], h[i + 1], h[i + 2], h[i + 3]]);
        let u16_at = |i: usize| u16::from_le_bytes([h[i], h[i + 1]]);
        assert_eq!(&h[0..4], b"RIFF");
        assert_eq!(&h[8..16], b"WAVEfmt ");
        assert_eq!(&h[36..40], b"data");
        assert_eq!(
            u64::from(u32_at(4)) + 8,
            total_bytes(),
            "RIFF size covers the file"
        );
        assert_eq!(u32_at(16), 16);
        assert_eq!(u16_at(20), 1, "PCM");
        assert_eq!(u16_at(22), 2, "stereo");
        assert_eq!(u32_at(24), 48_000);
        assert_eq!(u32_at(28), 48_000 * 4, "byte rate");
        assert_eq!(u16_at(32), 4, "block align");
        assert_eq!(u16_at(34), 16);
        assert_eq!(u32_at(40), 90 * 48_000 * 4);
        assert_eq!(total_bytes(), 17_280_044);
    }

    #[test]
    fn level_is_about_minus_20_dbfs_without_clipping() {
        let s = all_samples();
        assert_eq!(s.len(), 90 * 48_000);
        let sum: f64 = s.iter().map(|&v| (f64::from(v) / 32768.0).powi(2)).sum();
        let rms_db = 10.0 * (sum / s.len() as f64).log10();
        let peak = s.iter().map(|&v| i32::from(v).abs()).max().unwrap_or(0);
        let peak_db = 20.0 * (f64::from(peak) / 32768.0).log10();
        assert!((rms_db + 20.0).abs() < 1.0, "rms {rms_db:.2} dBFS");
        assert!(peak_db < -6.0, "peak {peak_db:.2} dBFS");
        assert!(peak < 32767, "no clipping");
    }

    #[test]
    fn noise_is_pink_and_deterministic() {
        let (a, b) = (all_samples(), all_samples());
        assert_eq!(a, b, "same bytes on every request");
        // Pink: the part below ~380 Hz carries more power than white noise
        // would give it (white: ~0.016 of the total with this 1-pole split).
        let mut lp = 0.0f64;
        let (mut low, mut total) = (0.0, 0.0);
        for &v in a.iter().take(480_000) {
            let x = f64::from(v);
            lp += 0.05 * (x - lp);
            low += lp * lp;
            total += x * x;
        }
        assert!(low / total > 0.1, "low share {}", low / total);
    }

    #[test]
    fn white_noise_is_in_range() {
        let mut p = Pink::new();
        for _ in 0..100_000 {
            let w = p.white();
            assert!((-1.0..1.0).contains(&w));
        }
    }
}
