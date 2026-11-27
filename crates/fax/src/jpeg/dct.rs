//! The 8 x 8 discrete cosine transform, forward and inverse (T.81 A.3.3).
//!
//! Done as the definition gives it, one dimension and then the other, in
//! floating point. A page is some hundred thousand blocks and that is soon
//! done, so there is no fast algorithm here to get wrong.

use std::sync::OnceLock;

/// `C(u) / 2 x cos((2x + 1) u pi / 16)`, by `[x][u]`: the kernel of both
/// transforms (T.81 A.3.3).
fn kernel() -> &'static [[f32; 8]; 8] {
    static KERNEL: OnceLock<[[f32; 8]; 8]> = OnceLock::new();
    KERNEL.get_or_init(|| {
        let mut table = [[0.0f32; 8]; 8];
        for (x, row) in table.iter_mut().enumerate() {
            for (u, value) in row.iter_mut().enumerate() {
                let c = if u == 0 { std::f64::consts::FRAC_1_SQRT_2 } else { 1.0 };
                let angle = (2 * x + 1) as f64 * u as f64 * std::f64::consts::PI / 16.0;
                *value = (c / 2.0 * angle.cos()) as f32;
            }
        }
        table
    })
}

/// Coefficients, row-major and already dequantised, to samples: the IDCT,
/// the level shift of A.3.1 put back, and the result held to eight bits.
pub fn inverse(coefficients: &[i32; 64], out: &mut [u8; 64]) {
    // A block with only its DC term is flat, and on a page most are.
    if coefficients[1..].iter().all(|&c| c == 0) {
        let flat = (coefficients[0] as f32 / 8.0 + 128.0).round().clamp(0.0, 255.0) as u8;
        out.fill(flat);
        return;
    }
    let k = kernel();
    let mut rows = [[0.0f32; 8]; 8];
    for (v, row) in rows.iter_mut().enumerate() {
        let input = &coefficients[v * 8..v * 8 + 8];
        for (x, value) in row.iter_mut().enumerate() {
            *value = (0..8).map(|u| k[x][u] * input[u] as f32).sum();
        }
    }
    for y in 0..8 {
        for x in 0..8 {
            let sum: f32 = (0..8).map(|v| k[y][v] * rows[v][x]).sum();
            out[y * 8 + x] = (sum + 128.0).round().clamp(0.0, 255.0) as u8;
        }
    }
}

/// Samples to coefficients, row-major: the level shift and the FDCT.
pub fn forward(samples: &[u8; 64], out: &mut [f32; 64]) {
    let k = kernel();
    let mut rows = [[0.0f32; 8]; 8];
    for (y, row) in rows.iter_mut().enumerate() {
        let input = &samples[y * 8..y * 8 + 8];
        for (u, value) in row.iter_mut().enumerate() {
            *value = (0..8).map(|x| k[x][u] * (f32::from(input[x]) - 128.0)).sum();
        }
    }
    for v in 0..8 {
        for u in 0..8 {
            out[v * 8 + u] = (0..8).map(|y| k[y][v] * rows[y][u]).sum();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flat_block_is_its_dc_term_alone() {
        let mut coefficients = [0.0f32; 64];
        forward(&[200u8; 64], &mut coefficients);
        // (200 - 128) x 8.
        assert!((coefficients[0] - 576.0).abs() < 0.01);
        assert!(coefficients[1..].iter().all(|c| c.abs() < 0.01));
    }

    #[test]
    fn the_inverse_undoes_the_forward() {
        let mut samples = [0u8; 64];
        let mut seed = 7u32;
        for s in &mut samples {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            *s = (seed >> 24) as u8;
        }
        let mut coefficients = [0.0f32; 64];
        forward(&samples, &mut coefficients);
        let whole: [i32; 64] = std::array::from_fn(|i| coefficients[i].round() as i32);
        let mut back = [0u8; 64];
        inverse(&whole, &mut back);
        for (a, b) in samples.iter().zip(&back) {
            assert!(a.abs_diff(*b) <= 1, "{a} came back {b}");
        }
    }
}
