//! How fast a file transfer is going.
//!
//! What the window used to show was the bytes so far over the seconds since
//! the transfer was asked for, and that is wrong three ways at once. The
//! seconds before the first byte of the file -- while the two ends say who
//! they are and agree where to start -- count against it, so a transfer
//! reads slow for its whole first minute. Going back over ground after an
//! error moves the position backwards, so a rewind reads as the transfer
//! running in reverse. And a rate averaged over everything says nothing about
//! the line now, which is the one thing worth watching while it runs.
//!
//! So: the clock starts when the file does, only new ground counts, and there
//! are two rates -- the last few seconds' and the whole file's -- from which
//! the time left is worked out.

use std::collections::VecDeque;

/// Seconds the recent rate is taken over.
const RECENT: f64 = 4.0;

/// Seconds between the readings kept for it.
const EVERY: f64 = 0.1;

/// Follows one transfer's progress, from readings of where it has got to.
#[derive(Debug, Clone, Default)]
pub struct Speedometer {
    /// The last time the transfer was seen with nothing of the file moved.
    idle_since: Option<f64>,
    /// When the file itself started moving.
    began: Option<f64>,
    /// The furthest into the file the transfer has got, and when.
    furthest: u64,
    latest: f64,
    /// (seconds, furthest) over the last few seconds, oldest first.
    recent: VecDeque<(f64, u64)>,
}

impl Speedometer {
    pub fn new() -> Self {
        Self::default()
    }

    /// The transfer is at byte `position` of the file, `now` seconds after it
    /// was asked for.
    pub fn update(&mut self, now: f64, position: u64) {
        self.latest = now;
        if self.began.is_none() {
            if position == 0 {
                self.idle_since = Some(now);
                return;
            }
            // The bytes that arrived since the last reading of nothing began
            // at that reading.
            let start = self.idle_since.unwrap_or(now);
            self.began = Some(start);
            self.recent.push_back((start, 0));
        }
        self.furthest = self.furthest.max(position);
        if self.recent.back().is_none_or(|(t, _)| now - t >= EVERY) {
            self.recent.push_back((now, self.furthest));
        }
        // Keep one reading from before the window, to measure from.
        while self.recent.len() > 2 && self.recent[1].0 <= now - RECENT {
            self.recent.pop_front();
        }
    }

    /// Bytes of new ground covered.
    #[cfg(test)]
    pub fn bytes(&self) -> u64 {
        self.furthest
    }

    /// Seconds since the file began moving.
    pub fn elapsed(&self) -> f64 {
        self.began.map_or(0.0, |b| (self.latest - b).max(0.0))
    }

    /// Bytes a second over the whole file so far, once there has been long
    /// enough to say.
    pub fn average(&self) -> Option<f64> {
        let elapsed = self.elapsed();
        (elapsed >= 0.5).then(|| self.furthest as f64 / elapsed)
    }

    /// Bytes a second over the last few seconds.
    pub fn recent(&self) -> Option<f64> {
        let &(from, at) = self.recent.front()?;
        let span = self.latest - from;
        (span >= 0.5).then(|| (self.furthest - at) as f64 / span)
    }

    /// Seconds left of a `total`-byte file at the recent rate, or the
    /// average's where there is no recent one.
    pub fn remaining(&self, total: u64) -> Option<f64> {
        let rate = self.recent().or_else(|| self.average()).filter(|r| *r > 0.0)?;
        Some(total.saturating_sub(self.furthest) as f64 / rate)
    }
}

/// A count of bytes as a person reads it: 812 B, 3.4 KB, 1.2 MB.
pub fn bytes(n: f64) -> String {
    if n < 1024.0 {
        format!("{n:.0} B")
    } else if n < 1024.0 * 1024.0 {
        format!("{:.1} KB", n / 1024.0)
    } else {
        format!("{:.2} MB", n / (1024.0 * 1024.0))
    }
}

/// Seconds as minutes and seconds, or hours when it comes to that.
pub fn clock(seconds: f64) -> String {
    let s = seconds.max(0.0).round() as u64;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_handshake_before_the_file_does_not_count() {
        let mut meter = Speedometer::new();
        // Ten seconds of the two ends greeting each other, then 1000 bytes a
        // second.
        for tick in 0..100 {
            meter.update(tick as f64 * 0.1, 0);
        }
        for tick in 0..=50 {
            meter.update(10.0 + tick as f64 * 0.1, tick * 100);
        }
        let average = meter.average().unwrap();
        assert!((average - 1000.0).abs() < 25.0, "{average}");
        assert!((meter.elapsed() - 5.0).abs() < 0.15, "{}", meter.elapsed());
    }

    #[test]
    fn going_back_over_ground_is_not_going_backwards() {
        let mut meter = Speedometer::new();
        for tick in 0..=40 {
            meter.update(tick as f64 * 0.1, tick * 100);
        }
        // A rewind to byte 2000, and on at the same rate.
        for tick in 0..=40 {
            meter.update(4.0 + tick as f64 * 0.1, 2000 + tick * 100);
        }
        assert_eq!(meter.bytes(), 6000);
        let recent = meter.recent().unwrap();
        // Nothing new for the twenty ticks it took to get back: the recent
        // rate reads the lull, and never less than nothing.
        assert!(recent > 0.0 && recent < 1000.0, "{recent}");
    }

    #[test]
    fn the_recent_rate_follows_a_change_the_average_does_not() {
        let mut meter = Speedometer::new();
        let mut at = 0u64;
        for tick in 0..=300 {
            // 3000 bytes a second for twenty seconds, then 1000.
            at += if tick < 200 { 300 } else { 100 };
            meter.update(tick as f64 * 0.1, at);
        }
        let recent = meter.recent().unwrap();
        let average = meter.average().unwrap();
        assert!((recent - 1000.0).abs() < 60.0, "recent {recent}");
        assert!(average > 2000.0, "average {average}");
        // And the time left goes by the recent one.
        let left = meter.remaining(at + 10_000).unwrap();
        assert!((left - 10.0).abs() < 1.0, "{left}");
    }

    #[test]
    fn nothing_is_said_before_there_is_anything_to_say() {
        let mut meter = Speedometer::new();
        meter.update(0.0, 0);
        assert_eq!((meter.average(), meter.recent(), meter.remaining(1000)), (None, None, None));
        meter.update(0.1, 50);
        assert_eq!(meter.average(), None);
    }

    #[test]
    fn sizes_and_times_read_as_people_write_them() {
        assert_eq!(bytes(812.0), "812 B");
        assert_eq!(bytes(3500.0), "3.4 KB");
        assert_eq!(bytes(1_300_000.0), "1.24 MB");
        assert_eq!(clock(83.0), "1:23");
        assert_eq!(clock(3725.0), "1:02:05");
    }
}
