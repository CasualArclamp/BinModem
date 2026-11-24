//! The waterfall and the level strip of one channel.

use dsp::fft::Fft;

use crate::model::{Track, Waterfall};

/// Transform length at 8000 samples a second: 32 ms, 31 Hz a row. Long enough
/// to part the two tones of a V.21 channel, short enough that the 10 ms
/// phase reversals of phase 2 show as the gaps they are.
const SIZE_AT_8K: usize = 256;
/// Columns a second: one every 5 ms.
pub const COLUMNS_PER_SEC: f64 = 200.0;
/// Level points a second.
pub const LEVEL_PER_SEC: f64 = 100.0;

/// The waterfall of `samples`, from 0 Hz to `top_hz` or half the rate.
pub fn waterfall(samples: &[f32], fs: f64, top_hz: f64) -> Waterfall {
    let size = ((SIZE_AT_8K as f64 * fs / 8000.0).round() as usize).next_power_of_two().max(64);
    let fft = Fft::new(size);
    let hz_per_bin = fs / size as f64;
    let bins = ((top_hz.min(fs / 2.0) / hz_per_bin).floor() as usize).clamp(1, size / 2);
    let window: Vec<f64> =
        (0..size).map(|i| 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / size as f64).cos()).collect();
    let gain = window.iter().sum::<f64>() / size as f64;
    let scale = 2.0 / (size as f64 * gain);
    let hop = fs / COLUMNS_PER_SEC;
    let columns = (samples.len() as f64 / hop).floor() as usize;

    let mut cells = Vec::with_capacity(columns * bins);
    let mut re = vec![0.0; size];
    let mut im = vec![0.0; size];
    for c in 0..columns {
        // Centred on the column's own time, so that a box drawn from an
        // event's start to its end sits on what it names.
        let centre = (c as f64 * hop) as isize;
        let first = centre - (size / 2) as isize;
        for i in 0..size {
            let at = first + i as isize;
            let x = if at >= 0 && (at as usize) < samples.len() { samples[at as usize] as f64 } else { 0.0 };
            re[i] = x * window[i];
            im[i] = 0.0;
        }
        fft.process(&mut re, &mut im);
        for k in 0..bins {
            let mag = (re[k] * re[k] + im[k] * im[k]).sqrt() * scale;
            let db = 20.0 * (mag + 1e-12).log10();
            let unit = (db - Waterfall::FLOOR_DB) / -Waterfall::FLOOR_DB;
            cells.push((unit.clamp(0.0, 1.0) * 255.0).round() as u8);
        }
    }
    Waterfall { columns_per_sec: COLUMNS_PER_SEC, bins, hz_per_bin, cells }
}

/// The level of `samples` every 10 ms, in dB below a full-scale sine.
pub fn level(samples: &[f32], fs: f64) -> Track {
    let hop = (fs / LEVEL_PER_SEC).round().max(1.0) as usize;
    let db = samples
        .chunks(hop)
        .map(|block| {
            let power = block.iter().map(|&x| (x as f64) * (x as f64)).sum::<f64>() / block.len() as f64;
            (10.0 * (2.0 * power + 1e-12).log10()) as f32
        })
        .collect();
    Track { points_per_sec: fs / hop as f64, db }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tone_lands_on_its_own_row_and_at_its_own_level() {
        let fs = 8000.0;
        let tone: Vec<f32> =
            (0..8000).map(|n| (0.5 * (std::f64::consts::TAU * 2100.0 * n as f64 / fs).sin()) as f32).collect();
        let w = waterfall(&tone, fs, 4000.0);
        assert_eq!(w.columns(), 200);
        let column = w.column(100);
        let peak = (0..w.bins).max_by_key(|&k| column[k]).unwrap();
        assert!((peak as f64 * w.hz_per_bin - 2100.0).abs() <= w.hz_per_bin, "peak at row {peak}");
        // Half scale is 6 dB down.
        let db = column[peak] as f64 / 255.0 * -Waterfall::FLOOR_DB + Waterfall::FLOOR_DB;
        assert!((db + 6.0).abs() < 1.5, "{db} dB");

        let l = level(&tone, fs);
        assert!((l.db[50] + 6.0).abs() < 0.2, "{} dB", l.db[50]);
    }
}
