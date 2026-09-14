//! Complex numbers, and the one piece of linear algebra a trained receiver
//! needs.
//!
//! Everything before V.34 got by on `(f64, f64)` pairs, because a receiver
//! that slices a handful of points does little arithmetic on them. A receiver
//! that solves for its equaliser from a training sequence does a great deal,
//! and reads better with the operators written as operators.

use std::ops::{Add, AddAssign, Div, Mul, MulAssign, Neg, Sub, SubAssign};

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Complex {
    pub re: f64,
    pub im: f64,
}

impl Complex {
    pub const ZERO: Self = Self { re: 0.0, im: 0.0 };
    pub const ONE: Self = Self { re: 1.0, im: 0.0 };
    pub const I: Self = Self { re: 0.0, im: 1.0 };

    pub const fn new(re: f64, im: f64) -> Self {
        Self { re, im }
    }

    /// `r` at an angle of `theta` radians.
    pub fn from_polar(r: f64, theta: f64) -> Self {
        Self { re: r * theta.cos(), im: r * theta.sin() }
    }

    pub fn conj(self) -> Self {
        Self { re: self.re, im: -self.im }
    }

    pub fn norm_sqr(self) -> f64 {
        self.re * self.re + self.im * self.im
    }

    pub fn abs(self) -> f64 {
        self.norm_sqr().sqrt()
    }

    /// The angle, in radians.
    pub fn arg(self) -> f64 {
        self.im.atan2(self.re)
    }

    pub fn scale(self, k: f64) -> Self {
        Self { re: self.re * k, im: self.im * k }
    }
}

impl From<(f64, f64)> for Complex {
    fn from((re, im): (f64, f64)) -> Self {
        Self { re, im }
    }
}

impl From<Complex> for (f64, f64) {
    fn from(c: Complex) -> Self {
        (c.re, c.im)
    }
}

impl Add for Complex {
    type Output = Self;
    fn add(self, o: Self) -> Self {
        Self { re: self.re + o.re, im: self.im + o.im }
    }
}

impl Sub for Complex {
    type Output = Self;
    fn sub(self, o: Self) -> Self {
        Self { re: self.re - o.re, im: self.im - o.im }
    }
}

impl Mul for Complex {
    type Output = Self;
    fn mul(self, o: Self) -> Self {
        Self { re: self.re * o.re - self.im * o.im, im: self.re * o.im + self.im * o.re }
    }
}

impl Mul<f64> for Complex {
    type Output = Self;
    fn mul(self, k: f64) -> Self {
        self.scale(k)
    }
}

impl Div<f64> for Complex {
    type Output = Self;
    fn div(self, k: f64) -> Self {
        self.scale(1.0 / k)
    }
}

impl Div for Complex {
    type Output = Self;
    fn div(self, o: Self) -> Self {
        (self * o.conj()) / o.norm_sqr()
    }
}

impl Neg for Complex {
    type Output = Self;
    fn neg(self) -> Self {
        Self { re: -self.re, im: -self.im }
    }
}

impl AddAssign for Complex {
    fn add_assign(&mut self, o: Self) {
        *self = *self + o;
    }
}

impl SubAssign for Complex {
    fn sub_assign(&mut self, o: Self) {
        *self = *self - o;
    }
}

impl MulAssign<f64> for Complex {
    fn mul_assign(&mut self, k: f64) {
        *self = self.scale(k);
    }
}

/// The weights `w` that make `sum(w[i] * row[i])` come closest to each
/// target, in the least-squares sense, with `ridge` added to the diagonal of
/// the normal equations.
///
/// The ridge is what keeps an equaliser sampling at twice the symbol rate
/// solvable. Half of what such an equaliser sees is the band beyond the
/// signal, where there is nothing to fit, and without something to hold them
/// the weights there are free to be anything -- which is noise, amplified.
///
/// None if the rows are too few or too alike to settle every weight.
pub fn least_squares(rows: &[&[Complex]], targets: &[Complex], ridge: f64) -> Option<Vec<Complex>> {
    let n = rows.first()?.len();
    // Normal equations: A[j][i] = sum conj(x[j]) x[i], b[j] = sum conj(x[j]) d.
    let mut a = vec![Complex::ZERO; n * n];
    let mut b = vec![Complex::ZERO; n];
    for (row, &target) in rows.iter().zip(targets) {
        for j in 0..n {
            let xj = row[j].conj();
            b[j] += xj * target;
            for i in j..n {
                a[j * n + i] += xj * row[i];
            }
        }
    }
    for j in 0..n {
        a[j * n + j].re += ridge;
        for i in 0..j {
            a[j * n + i] = a[i * n + j].conj();
        }
    }
    solve_hermitian(&a, &b)
}

/// Solve `A x = b` for a Hermitian positive definite `A`, given row by row,
/// by Cholesky factorisation.
pub fn solve_hermitian(a: &[Complex], b: &[Complex]) -> Option<Vec<Complex>> {
    let n = b.len();
    if a.len() != n * n {
        return None;
    }
    // A = L L*, with L lower triangular.
    let mut l = vec![Complex::ZERO; n * n];
    for j in 0..n {
        let mut diagonal = a[j * n + j].re;
        for k in 0..j {
            diagonal -= l[j * n + k].norm_sqr();
        }
        // What is left of the diagonal once the columns before it have
        // taken their share. Next to nothing means this column is one of
        // them over again.
        if diagonal <= 1e-12 * a[j * n + j].re.abs() || diagonal <= 0.0 || !diagonal.is_finite() {
            return None;
        }
        let root = diagonal.sqrt();
        l[j * n + j] = Complex::new(root, 0.0);
        for i in j + 1..n {
            let mut sum = a[i * n + j];
            for k in 0..j {
                sum -= l[i * n + k] * l[j * n + k].conj();
            }
            l[i * n + j] = sum / root;
        }
    }
    // L y = b, then L* x = y.
    let mut y = vec![Complex::ZERO; n];
    for i in 0..n {
        let mut sum = b[i];
        for k in 0..i {
            sum -= l[i * n + k] * y[k];
        }
        y[i] = sum / l[i * n + i].re;
    }
    let mut x = vec![Complex::ZERO; n];
    for i in (0..n).rev() {
        let mut sum = y[i];
        for k in i + 1..n {
            sum -= l[k * n + i].conj() * x[k];
        }
        x[i] = sum / l[i * n + i].re;
    }
    Some(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Complex, b: Complex) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn the_operators_are_complex_arithmetic() {
        let a = Complex::new(1.0, 2.0);
        let b = Complex::new(-3.0, 0.5);
        assert!(close(a * b, Complex::new(-4.0, -5.5)));
        assert!(close(a / b * b, a));
        assert!(close(a * a.conj(), Complex::new(5.0, 0.0)));
        assert!(close(Complex::I * Complex::I, -Complex::ONE));
        assert!((Complex::from_polar(2.0, 1.0).arg() - 1.0).abs() < 1e-12);
    }

    #[test]
    fn least_squares_finds_the_filter_that_made_the_targets() {
        // Targets made by a known three-tap filter from pseudo-random inputs,
        // which the solver should get back exactly.
        let truth = [Complex::new(0.2, -0.1), Complex::new(1.0, 0.3), Complex::new(-0.4, 0.05)];
        let mut seed = 12345u32;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            f64::from(seed) / f64::from(u32::MAX) - 0.5
        };
        let rows: Vec<Vec<Complex>> = (0..50).map(|_| (0..3).map(|_| Complex::new(next(), next())).collect()).collect();
        let targets: Vec<Complex> =
            rows.iter().map(|r| r.iter().zip(&truth).fold(Complex::ZERO, |s, (x, w)| s + *x * *w)).collect();
        let refs: Vec<&[Complex]> = rows.iter().map(Vec::as_slice).collect();
        let w = least_squares(&refs, &targets, 0.0).expect("solvable");
        for (got, want) in w.iter().zip(&truth) {
            assert!(close(*got, *want), "{got:?} against {want:?}");
        }
    }

    #[test]
    fn a_system_with_no_unique_answer_is_refused_unless_ridged() {
        // Two identical columns: without a ridge, nothing picks between them.
        let rows: Vec<Vec<Complex>> = (0..10).map(|i| vec![Complex::new(f64::from(i), 0.0); 2]).collect();
        let refs: Vec<&[Complex]> = rows.iter().map(Vec::as_slice).collect();
        let targets: Vec<Complex> = (0..10).map(|i| Complex::new(f64::from(i), 0.0)).collect();
        assert!(least_squares(&refs, &targets, 0.0).is_none());
        let w = least_squares(&refs, &targets, 1e-3).expect("ridged");
        assert!((w[0] - w[1]).abs() < 1e-6, "the ridge splits it evenly");
        assert!((w[0].re + w[1].re - 1.0).abs() < 1e-3);
    }
}
