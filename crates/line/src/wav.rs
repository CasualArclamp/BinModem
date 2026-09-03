//! Minimal RIFF/WAVE reader for capture files and test vectors.

use std::fs;
use std::io;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct Wav {
    pub sample_rate: u32,
    pub channels: u16,
    /// Interleaved samples, normalised to `[-1, 1)`.
    pub samples: Vec<f32>,
}

impl Wav {
    /// Average all channels into a single stream.
    ///
    /// A 2-wire line tap is inherently mono; stereo capture files duplicate it.
    pub fn mono(&self) -> Vec<f32> {
        if self.channels <= 1 {
            return self.samples.clone();
        }
        let ch = self.channels as usize;
        self.samples
            .chunks_exact(ch)
            .map(|f| f.iter().sum::<f32>() / ch as f32)
            .collect()
    }

    pub fn duration_secs(&self) -> f64 {
        self.samples.len() as f64 / (self.sample_rate as f64 * self.channels as f64)
    }
}

fn u16le(b: &[u8]) -> u16 {
    u16::from_le_bytes([b[0], b[1]])
}

fn u32le(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

/// Read a 16-bit PCM WAVE file.
///
/// Tolerates a bogus `data` chunk size. Streaming writers commonly stamp
/// `0x3FFFFFFF` there and never go back to fix it, which is exactly what the
/// reference capture in this repo does; in that case the chunk is taken to run
/// to the end of the file.
pub fn read<P: AsRef<Path>>(path: P) -> io::Result<Wav> {
    let blob = fs::read(path)?;
    let bad = |m: &str| io::Error::new(io::ErrorKind::InvalidData, m.to_string());

    if blob.len() < 12 || &blob[0..4] != b"RIFF" || &blob[8..12] != b"WAVE" {
        return Err(bad("not a RIFF/WAVE file"));
    }

    let mut pos = 12usize;
    let mut rate = 0u32;
    let mut channels = 0u16;
    let mut bits = 0u16;

    while pos + 8 <= blob.len() {
        let id = &blob[pos..pos + 4];
        let declared = u32le(&blob[pos + 4..pos + 8]) as usize;
        let body = pos + 8;

        if id == b"fmt " {
            if body + 16 > blob.len() {
                return Err(bad("truncated fmt chunk"));
            }
            channels = u16le(&blob[body + 2..]);
            rate = u32le(&blob[body + 4..]);
            bits = u16le(&blob[body + 14..]);
        } else if id == b"data" {
            if bits != 16 {
                return Err(bad("only 16-bit PCM is supported"));
            }
            let avail = blob.len() - body;
            let n = if declared == 0 || declared > avail { avail } else { declared };
            let frame = 2 * channels.max(1) as usize;
            let n = n - (n % frame);
            let samples = blob[body..body + n]
                .as_chunks::<2>().0.iter()
                .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
                .collect();
            return Ok(Wav { sample_rate: rate, channels, samples });
        }

        // Chunks are word-aligned; a bogus size would otherwise walk off the end.
        pos = body + declared.min(blob.len() - body) + (declared & 1);
    }
    Err(bad("no data chunk"))
}
