//! The other end's echo, taken out of a channel.
//!
//! A recording with an end a channel is rarely an end alone on a channel.
//! Where it was made on the telephone side of a hybrid, each channel is one
//! modem's transmission with the other's coming back under it, twenty or
//! thirty decibels down: nothing to a tone or to four points, and the end of
//! any hope of reading a thousand.
//!
//! A modem cancels its own echo knowing only what it sent, and has to learn
//! the path while the far end talks over it. A reader of a recording is
//! better placed than either modem. It has what the other end sent, exactly,
//! on the other channel; and it has the whole call, so it can go to the
//! places where one end was sending alone -- its training signal, which is
//! as good a probe as there is -- and measure the path there with nothing in
//! the way. That one measurement is then taken off the whole channel.
//!
//! What can move is the delay. The other end's signal is played out by a
//! jitter buffer, which may drop or repeat a sample to keep step, and every
//! such slip moves the echo by one. That is one number to follow, not a path
//! to learn again; but it can only be followed where the other end is alone
//! again, since under this end's own signal the echo is too small a part of
//! what is there for a delay to be told from its neighbour. A slip in the
//! middle of data goes unfollowed until the next such place.

/// What was found and done on one channel.
#[derive(Debug, Clone)]
pub struct Cancelled {
    /// The channel with the echo taken out.
    pub samples: Vec<f32>,
    /// How late the other end's signal comes back, in milliseconds.
    pub delay_ms: f64,
    /// The echo's level against the signal it is an echo of, in dB.
    pub echo_db: f64,
    /// How much of it went, in dB, where the other end was alone.
    pub removed_db: f64,
    /// Times the delay moved.
    pub slips: usize,
}

/// A stretch of this length is taken at a time, for levels and for the delay.
const FRAME: f64 = 0.020;
/// The furthest back an echo is looked for.
const FURTHEST: f64 = 0.300;
/// The length of the path that is measured.
const SPAN: f64 = 0.016;
/// How much of that span is before the main arrival.
const LEAD: f64 = 0.003;

fn power(x: &[f32]) -> f64 {
    x.iter().map(|v| f64::from(*v) * f64::from(*v)).sum::<f64>() / x.len().max(1) as f64
}

/// Solve the symmetric positive system `a x = b` in place of nothing: the
/// normal equations of a least squares fit, by Cholesky.
fn solve(mut a: Vec<f64>, mut b: Vec<f64>, n: usize) -> Option<Vec<f64>> {
    for j in 0..n {
        let mut d = a[j * n + j];
        for k in 0..j {
            d -= a[j * n + k] * a[j * n + k];
        }
        if d <= 0.0 {
            return None;
        }
        let d = d.sqrt();
        a[j * n + j] = d;
        for i in j + 1..n {
            let mut v = a[i * n + j];
            for k in 0..j {
                v -= a[i * n + k] * a[j * n + k];
            }
            a[i * n + j] = v / d;
        }
    }
    for i in 0..n {
        let mut v = b[i];
        for k in 0..i {
            v -= a[i * n + k] * b[k];
        }
        b[i] = v / a[i * n + i];
    }
    for i in (0..n).rev() {
        let mut v = b[i];
        for k in i + 1..n {
            v -= a[k * n + i] * b[k];
        }
        b[i] = v / a[i * n + i];
    }
    Some(b)
}

/// Take `other`'s echo out of `own`. None if there is none to speak of, or
/// nowhere on the recording to measure it.
pub fn cancel(own: &[f32], other: &[f32], fs: f64) -> Option<Cancelled> {
    let n = own.len().min(other.len());
    let frame = (FRAME * fs) as usize;
    let furthest = (FURTHEST * fs) as usize;
    if n < 4 * furthest || frame == 0 {
        return None;
    }
    // Where the other end is sending and this one is not: this channel is
    // a good deal quieter than the other, and the other is not silent.
    let frames = n / frame;
    let levels: Vec<(f64, f64)> = (0..frames)
        .map(|k| {
            (
                power(&own[k * frame..(k + 1) * frame]),
                power(&other[k * frame..(k + 1) * frame]),
            )
        })
        .collect();
    let loud = levels.iter().map(|l| l.1).fold(0.0, f64::max);
    if loud <= 0.0 {
        return None;
    }
    // The other end has to have been loud for the whole of the furthest an
    // echo could have come from, or what is on this channel is not all echo.
    let back = furthest.div_ceil(frame);
    let alone: Vec<usize> = (back..frames)
        .filter(|&k| {
            (k - back..=k).all(|j| levels[j].1 > 0.02 * loud)
                && levels[k].0 < 0.05 * levels[k].1
                && levels[k].0 > 0.0
        })
        .collect();
    if alone.len() * frame < (0.3 * fs) as usize {
        return None;
    }
    // The path is measured on one unbroken stretch, the longest there is:
    // between two stretches the buffer may have slipped, and a path fitted
    // across a slip is two paths a sample apart, added together.
    let mut runs: Vec<(usize, usize)> = Vec::new();
    for &k in &alone {
        match runs.last_mut() {
            Some((_, last)) if *last + 1 == k => *last = k,
            _ => runs.push((k, k)),
        }
    }
    let (run_from, run_to) = runs.iter().copied().max_by_key(|(a, b)| b - a)?;
    if (run_to + 1 - run_from) * frame < (0.25 * fs) as usize {
        return None;
    }
    // Not more of it than a slip is likely to leave whole.
    let stretch: Vec<usize> = (run_from * frame..(run_to + 1) * frame)
        .take((0.8 * fs) as usize)
        .collect();

    // The delay: where this channel, in those stretches, is most like the
    // other one earlier.
    let picked: Vec<usize> = stretch.iter().copied().step_by(2).collect();
    let (mut best, mut delay) = (0.0f64, 0usize);
    for lag in 0..furthest {
        let sum: f64 = picked
            .iter()
            .map(|&i| f64::from(own[i]) * f64::from(other[i - lag]))
            .sum();
        if sum.abs() > best {
            best = sum.abs();
            delay = lag;
        }
    }

    // The path, by least squares over the same stretches: this channel as a
    // sum of the other's samples around that delay.
    let taps = (SPAN * fs) as usize;
    let lead = ((LEAD * fs) as usize).min(delay);
    let first = delay - lead;
    let fit: &[usize] = &stretch;
    let mut a = vec![0.0f64; taps * taps];
    let mut b = vec![0.0f64; taps];
    let mut row = vec![0.0f64; taps];
    for &i in fit {
        if i < first + taps {
            continue;
        }
        for (j, slot) in row.iter_mut().enumerate() {
            *slot = f64::from(other[i - first - j]);
        }
        let y = f64::from(own[i]);
        for j in 0..taps {
            b[j] += row[j] * y;
            let rj = row[j];
            for k in 0..=j {
                a[j * taps + k] += rj * row[k];
            }
        }
    }
    // A little on the diagonal: the other end's signal has no energy at the
    // edges of the band, and the fit must not invent a path there.
    let ridge = 1e-4 * (0..taps).map(|j| a[j * taps + j]).sum::<f64>() / taps as f64;
    for j in 0..taps {
        a[j * taps + j] += ridge;
        for k in j + 1..taps {
            a[j * taps + k] = a[k * taps + j];
        }
    }
    let path = solve(a, b, taps)?;

    // The echo as the path makes it at a shift of `shift` samples, at `i`.
    let echo_at = |i: usize, shift: isize| -> f64 {
        let base = i as isize - first as isize - shift;
        let mut sum = 0.0;
        for (j, h) in path.iter().enumerate() {
            let at = base - j as isize;
            if at >= 0 && (at as usize) < n {
                sum += h * f64::from(other[at as usize]);
            }
        }
        sum
    };

    // The echo itself, at the delay measured, once: every other delay is
    // this moved along.
    let made: Vec<f64> = (0..n).map(|i| echo_at(i, 0)).collect();
    let moved = |i: usize, shift: isize| -> f64 {
        let at = i as isize - shift;
        if at >= 0 && (at as usize) < n {
            made[at as usize]
        } else {
            0.0
        }
    };
    // What a stretch has left in it with the echo taken off at a shift.
    let left = |from: usize, to: usize, shift: isize| -> f64 {
        (from..to)
            .map(|i| (f64::from(own[i]) - moved(i, shift)).powi(2))
            .sum::<f64>()
    };

    // Which delay. It is settled wherever the other end is alone, where
    // the right one takes twenty decibels off and its neighbours take none:
    // there is no mistaking it. Under this end's own signal the echo is a
    // few thousandths of what is there, and the right delay leaves so little
    // less than a wrong one that a second of samples cannot tell them
    // apart. So the delay found at one such stretch stands until the next.
    let mut anchors: Vec<(usize, isize)> = Vec::new();
    // How much went at each such stretch.
    let mut took: Vec<f64> = Vec::new();
    let mut slips = 0usize;
    let mut held = 0isize;
    for &(from, to) in &runs {
        let (from, to) = (from * frame, (to + 1) * frame);
        if to - from < (0.1 * fs) as usize {
            continue;
        }
        let bare: f64 = (from..to).map(|i| f64::from(own[i]).powi(2)).sum();
        let (least, shift) = (held - 8..=held + 8)
            .map(|shift| (left(from, to, shift), shift))
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .unwrap_or((bare, held));
        // Only where it plainly is the echo that went.
        if least < 0.1 * bare {
            took.push(10.0 * (bare / least.max(1e-30)).log10());
            if shift != held && !anchors.is_empty() {
                slips += 1;
            }
            held = shift;
            anchors.push((from, shift));
        }
    }
    let shift_at = |i: usize| -> isize {
        match anchors.partition_point(|a| a.0 <= i) {
            0 => anchors.first().map_or(0, |a| a.1),
            k => anchors[k - 1].1,
        }
    };

    let mut samples = own[..n].to_vec();
    let (mut before, mut after, mut sent) = (0.0f64, 0.0f64, 0.0f64);
    let alone_at: Vec<bool> = {
        let mut flags = vec![false; frames];
        for &k in &alone {
            flags[k] = true;
        }
        flags
    };
    for (i, slot) in samples.iter_mut().enumerate() {
        let clean = f64::from(own[i]) - moved(i, shift_at(i));
        if alone_at.get(i / frame).copied().unwrap_or(false) {
            before += f64::from(own[i]).powi(2);
            after += clean * clean;
            sent += f64::from(other[i.saturating_sub(delay)]).powi(2);
        }
        *slot = clean as f32;
    }
    if before <= 0.0 || sent <= 0.0 {
        return None;
    }
    let echo_db = 10.0 * (before / sent).log10();
    let _ = after;
    // The middle one of the stretches where the other end was plainly
    // alone: some of what passes for such a stretch has something of this
    // end's own in it, and says nothing about the echo.
    took.sort_by(f64::total_cmp);
    let removed_db = took.get(took.len() / 2).copied().unwrap_or(0.0);
    // An echo that is not there cannot be taken out, and the fit of one is
    // only noise to add.
    if echo_db < -55.0 || removed_db < 3.0 {
        return None;
    }
    Some(Cancelled {
        samples,
        delay_ms: 1000.0 * delay as f64 / fs,
        echo_db,
        removed_db,
        slips,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Noise that is the same every time.
    fn noise(seed: u64, n: usize) -> Vec<f32> {
        let mut state = seed;
        (0..n)
            .map(|_| {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                ((state >> 33) as f64 / (1u64 << 31) as f64 - 1.0) as f32 * 0.2
            })
            .collect()
    }

    #[test]
    fn an_echo_measured_where_one_end_is_alone_comes_out_from_under_the_other() {
        let fs = 8000.0;
        let n = 96_000;
        let far = noise(1, n);
        // This end is silent for the first three seconds, sends for four,
        // is silent for one more, and sends again.
        let sending = |i: usize| (24_000..56_000).contains(&i) || i >= 64_000;
        let near: Vec<f32> = noise(2, n)
            .into_iter()
            .enumerate()
            .map(|(i, v)| if sending(i) { v } else { 0.0 })
            .collect();
        // The far end comes back 37 ms late through a short path, 26 dB down,
        // and the buffer slips a sample at six seconds, in the middle of this
        // end's sending.
        let path = [0.03f32, 0.04, -0.02, 0.01];
        let own: Vec<f32> = (0..n)
            .map(|i| {
                let slip = usize::from(i >= 48_000);
                let echo: f32 = path
                    .iter()
                    .enumerate()
                    .map(|(j, h)| {
                        if i >= 296 + j + slip {
                            h * far[i - 296 - j - slip]
                        } else {
                            0.0
                        }
                    })
                    .sum();
                near[i] + echo
            })
            .collect();
        let done = cancel(&own, &far, fs).expect("no echo found");
        assert!(
            (done.delay_ms - 37.0).abs() < 1.0,
            "delay {} ms",
            done.delay_ms
        );
        assert!(done.removed_db > 30.0, "only {} dB went", done.removed_db);
        assert_eq!(done.slips, 1, "the slip was not followed");
        // Under this end's own signal, before the slip and once the far end
        // has been alone again after it, what is left is this end's signal.
        for range in [30_000..40_000, 70_000..90_000] {
            let left = power(
                &done.samples[range.clone()]
                    .iter()
                    .zip(&near[range.clone()])
                    .map(|(a, b)| a - b)
                    .collect::<Vec<_>>(),
            );
            let echo = power(
                &own[range.clone()]
                    .iter()
                    .zip(&near[range])
                    .map(|(a, b)| a - b)
                    .collect::<Vec<_>>(),
            );
            assert!(left < 0.01 * echo, "left {left:e} of {echo:e}");
        }
    }

    #[test]
    fn a_channel_with_no_echo_on_it_is_left_alone() {
        let fs = 8000.0;
        let far = noise(3, 40_000);
        let near: Vec<f32> = noise(4, 40_000)
            .into_iter()
            .enumerate()
            .map(|(i, v)| if i < 16_000 { 0.0 } else { v })
            .collect();
        assert!(cancel(&near, &far, fs).is_none());
    }
}
