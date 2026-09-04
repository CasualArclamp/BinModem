//! Minimal RIFF/WAVE reader and writer for capture files and test vectors.

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

/// Write mono 16-bit PCM.
///
/// Sixteen bits because that is what every capture in this project is and what
/// every tool that might be pointed at the result expects. A modem's line
/// signal has nothing like the dynamic range to need more: the whole of it
/// lives within about 40 dB, and the thing at the far end is a telephone
/// network that will do far worse to it than quantising ever could.
pub fn write<P: AsRef<Path>>(path: P, samples: &[f32], sample_rate: u32) -> io::Result<()> {
    let data_bytes = samples.len() * 2;
    let mut out = Vec::with_capacity(44 + data_bytes);

    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&u32::try_from(36 + data_bytes).unwrap_or(u32::MAX).to_le_bytes());
    out.extend_from_slice(b"WAVE");

    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // chunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // one channel
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * 2).to_le_bytes()); // bytes per second
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample

    out.extend_from_slice(b"data");
    out.extend_from_slice(&u32::try_from(data_bytes).unwrap_or(u32::MAX).to_le_bytes());
    for &s in samples {
        // Clamp rather than wrap. A sample that has gone past full scale is
        // already a fault, and letting it come out the other side as a loud
        // click of the opposite sign turns a small one into an obvious one.
        let v = (s.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    fs::write(path, out)
}

#[cfg(test)]
mod write_tests {
    use super::*;

    #[test]
    fn what_is_written_reads_back() {
        let dir = std::env::temp_dir().join("dialupmodem2-wav-test");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("roundtrip.wav");

        let samples: Vec<f32> = (0..8000)
            .map(|i| (i as f32 * 0.01).sin() * 0.5)
            .collect();
        write(&path, &samples, 16_000).unwrap();

        let back = read(&path).unwrap();
        assert_eq!(back.sample_rate, 16_000);
        assert_eq!(back.channels, 1);
        assert_eq!(back.samples.len(), samples.len());
        for (a, b) in samples.iter().zip(back.samples.iter()) {
            // Sixteen bits, so a thirty-thousandth either way.
            assert!((a - b).abs() < 1.0 / 16_000.0, "{a} came back as {b}");
        }
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn samples_past_full_scale_are_clamped_rather_than_wrapped() {
        let dir = std::env::temp_dir().join("dialupmodem2-wav-test");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("clipped.wav");
        write(&path, &[2.0, -2.0, 0.0], 16_000).unwrap();
        let back = read(&path).unwrap();
        assert!(back.samples[0] > 0.9, "wrapped to {}", back.samples[0]);
        assert!(back.samples[1] < -0.9, "wrapped to {}", back.samples[1]);
        let _ = fs::remove_file(&path);
    }
}
