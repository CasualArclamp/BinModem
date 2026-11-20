//! What the three modulations have in common, as seen from above.
//!
//! Each of them is a transmitter that offers room for octets every so often
//! and a receiver that hands back what arrived in that room; each
//! measures the line it is listening to and says how many bits it thinks the
//! line will stand; and each can be told, by the far end, how many bits to
//! send. The traits here are that and nothing else, so that what sits above
//! -- the exchange of tables, the frames, the retransmission -- is written
//! once.

use crate::frame::Layout;

/// How many bits go where, and how well they are protected.
///
/// For the multicarrier mode there is an entry a carrier. The single-carrier
/// modes have one entry, the bits a symbol carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    /// Check octets in each Reed-Solomon codeword.
    pub parity: u8,
    pub bits: Vec<u8>,
}

impl Table {
    /// Every bit the table places in one symbol.
    pub fn total(&self) -> usize {
        self.bits.iter().map(|&b| usize::from(b)).sum()
    }

    /// The table as the body of a frame: the check octets, then runs of
    /// equal entries as a count and a value. A loaded line is mostly runs.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = vec![self.parity];
        out.extend_from_slice(&(self.bits.len() as u16).to_le_bytes());
        let mut at = 0;
        while at < self.bits.len() {
            let value = self.bits[at];
            let run = self.bits[at..].iter().take(255).take_while(|&&b| b == value).count();
            out.push(run as u8);
            out.push(value);
            at += run;
        }
        out
    }

    pub fn decode(body: &[u8]) -> Option<Self> {
        let (&parity, rest) = body.split_first()?;
        if rest.len() < 2 {
            return None;
        }
        let len = usize::from(u16::from_le_bytes([rest[0], rest[1]]));
        let mut bits = Vec::with_capacity(len);
        for pair in rest[2..].chunks(2) {
            let &[run, value] = pair else { return None };
            if run == 0 {
                return None;
            }
            bits.extend(std::iter::repeat_n(value, usize::from(run)));
        }
        (bits.len() == len).then_some(Self { parity, bits })
    }
}

/// What a receiver has to show for one block of octets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Arrived {
    /// The octets as sliced, errors and all, and how they are laid out;
    /// and, from a receiver that can tell, which of them never arrived at
    /// all and are only there to keep the rest in their places.
    Octets { line: Vec<u8>, layout: Layout, erased: Option<Vec<bool>> },
    /// A block went by that could not be read: sent with a table this end
    /// does not have, or missed.
    Lost,
}

/// How a receiver is getting on, in terms any of them can answer in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Health {
    /// In step with the far end's signal.
    pub locked: bool,
    /// And finished measuring it.
    pub steady: bool,
    /// Signal to noise over the part of the band in use, in decibels.
    pub snr_db: f64,
    /// How far the far end's sample clock is from this one.
    pub clock_ppm: f64,
}

/// One slice of the band as a receiver sees it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bin {
    pub hz: f64,
    pub snr_db: f64,
    /// Bits the far end is putting here.
    pub bits: u8,
}

/// The sending half of a modulation.
pub trait Sender {
    /// Room that needs filling for the block about to start, if any is
    /// wanted and it has not been given yet.
    fn wants(&self) -> Option<Layout>;

    /// The octets for that room.
    fn give(&mut self, line: Vec<u8>);

    /// The next sample for the line.
    fn next_sample(&mut self) -> f64;

    /// Send with this table from the next superframe on, marked `id`.
    fn adopt(&mut self, id: u8, table: Table);

    /// The table in use, or about to be.
    fn id(&self) -> u8;

    /// Go back to the table both ends start with, at the next superframe.
    fn reset(&mut self);

    /// Bits a second with the table in use: on the line, and of frames.
    fn rates(&self) -> (f64, f64);

    /// Samples that reached full scale and were held there.
    fn clipped(&self) -> u64;

    /// Send this many decibels below the level the transmitter was made
    /// for; nought is that level. Nothing for a sender whose samples are
    /// the data and have no level to choose.
    fn trim(&mut self, _db: f64) {}
}

/// The codes a table may be given, strongest last: the share of each
/// codeword that is data, and how many decibels a point of the grid has to
/// stand clear of the noise for that code to carry it.
///
/// The first is the one a good line gets: sixteen check octets a word,
/// whatever its length, and eight decibels in hand. The rest are for lines
/// that have not got eight decibels to spare. A code that is half checks
/// mends a quarter of a word's octets, and a grid of four points can then be
/// read at a ratio where one in fourteen of its points comes out wrong; so
/// it needs three decibels in hand and not eight, and a line at 8 dB that
/// could carry nothing carries a bit a symbol. Below a quarter there is
/// little more to be had from a code that is told only right or wrong.
///
/// A receiver works out the table for each and asks for whichever carries
/// most.
pub const CODES: [(Option<f64>, f64); 5] = [(None, 8.0), (Some(0.75), 5.0), (Some(0.5), 3.0), (Some(0.33), 2.0), (Some(0.25), 1.5)];

/// Check octets for a code of `rate` on blocks of `capacity` octets, which
/// are cut into codewords of 255 at most.
pub fn checks_for(rate: f64, capacity: usize) -> u8 {
    let words = capacity.div_ceil(255).max(1);
    let n = capacity / words;
    (((1.0 - rate) * n as f64).round() as usize).clamp(16, n.saturating_sub(8).max(16)).min(246) as u8
}

/// What one end hears, as it tells the other.
///
/// A transmitter cannot hear its own line. It does not know whether what it
/// sends arrives faint, or so loud that the far end's converter is flat
/// against its stops, and those are the two things it could do something
/// about. So each end measures what arrives on each channel, a second at a
/// time, and sends it back.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Report {
    /// The level arriving, RMS, in decibels against full scale.
    pub level_db: f64,
    /// The largest sample in the second, the same way.
    pub peak_db: f64,
    /// Samples in the second that were at full scale: the converter had
    /// nowhere further to go.
    pub clipped: u32,
    /// Signal to noise at the slicer over the band in use, in decibels:
    /// a symbol's energy against the noise under it. Nought until the
    /// receiver has measured anything.
    pub snr_db: f64,
    /// Decibels below its made-for level the reporting end is itself
    /// sending at.
    pub trim_db: f64,
}

impl Report {
    /// Eight octets: a version, then half decibels below full scale for the
    /// level and the peak, the clipped count, tenths of a decibel of signal
    /// to noise, and half decibels of trim.
    pub fn encode(&self) -> Vec<u8> {
        let below = |db: f64| (-2.0 * db).round().clamp(0.0, 255.0) as u8;
        let snr = (10.0 * self.snr_db).round().clamp(0.0, 65_535.0) as u16;
        let clipped = self.clipped.min(65_535) as u16;
        let mut out = vec![1, below(self.level_db), below(self.peak_db)];
        out.extend_from_slice(&clipped.to_le_bytes());
        out.extend_from_slice(&snr.to_le_bytes());
        out.push(below(-self.trim_db));
        out
    }

    pub fn decode(body: &[u8]) -> Option<Self> {
        let &[1, level, peak, c0, c1, s0, s1, trim, ..] = body else { return None };
        Some(Self {
            level_db: -f64::from(level) / 2.0,
            peak_db: -f64::from(peak) / 2.0,
            clipped: u32::from(u16::from_le_bytes([c0, c1])),
            snr_db: f64::from(u16::from_le_bytes([s0, s1])) / 10.0,
            trim_db: f64::from(trim) / 2.0,
        })
    }
}

// A boxed half is a half, so that a program can choose its modulation when
// it is run rather than when it is built.
impl<T: Sender + ?Sized> Sender for Box<T> {
    fn wants(&self) -> Option<Layout> {
        (**self).wants()
    }
    fn give(&mut self, line: Vec<u8>) {
        (**self).give(line)
    }
    fn next_sample(&mut self) -> f64 {
        (**self).next_sample()
    }
    fn adopt(&mut self, id: u8, table: Table) {
        (**self).adopt(id, table)
    }
    fn id(&self) -> u8 {
        (**self).id()
    }
    fn reset(&mut self) {
        (**self).reset()
    }
    fn rates(&self) -> (f64, f64) {
        (**self).rates()
    }
    fn clipped(&self) -> u64 {
        (**self).clipped()
    }
    fn trim(&mut self, db: f64) {
        (**self).trim(db)
    }
}

impl<T: Receiver + ?Sized> Receiver for Box<T> {
    fn feed(&mut self, sample: f64) {
        (**self).feed(sample)
    }
    fn take(&mut self) -> Option<Arrived> {
        (**self).take()
    }
    fn proposal(&mut self) -> Option<Table> {
        (**self).proposal()
    }
    fn expect(&mut self, id: u8, table: Table) {
        (**self).expect(id, table)
    }
    fn id(&self) -> Option<u8> {
        (**self).id()
    }
    fn back_off(&mut self, db: f64) {
        (**self).back_off(db)
    }
    fn remeasure(&mut self, db: f64) {
        (**self).remeasure(db)
    }
    fn health(&self) -> Health {
        (**self).health()
    }
    fn rates(&self) -> (f64, f64) {
        Receiver::rates(&**self)
    }
    fn spectrum(&self) -> Vec<Bin> {
        (**self).spectrum()
    }
    fn points(&mut self) -> Vec<(f64, f64)> {
        (**self).points()
    }
    fn shown_bits(&self) -> u8 {
        (**self).shown_bits()
    }
    fn code(&self) -> Option<(usize, usize)> {
        (**self).code()
    }
    fn echo_under(&mut self, sample: f64) {
        (**self).echo_under(sample)
    }
    fn echo_again(&mut self) {
        (**self).echo_again()
    }
    fn echo_left_db(&self) -> Option<f64> {
        (**self).echo_left_db()
    }
}

/// The receiving half.
pub trait Receiver {
    /// One sample off the line.
    fn feed(&mut self, sample: f64);

    /// The next block that has finished arriving.
    fn take(&mut self) -> Option<Arrived>;

    /// A table the receiver would like the far end to use, when it has
    /// measured the line and has one to suggest. Yields each suggestion once.
    fn proposal(&mut self) -> Option<Table>;

    /// What a superframe marked `id` will have been sent with.
    fn expect(&mut self, id: u8, table: Table);

    /// The mark on the superframe now arriving, once anything is.
    fn id(&self) -> Option<u8>;

    /// Ask for a fresh proposal with this many more decibels in hand than
    /// the last, because the last was not enough.
    fn back_off(&mut self, db: f64);

    /// Measure the line afresh once the far end is back on the starting
    /// table, and propose from that with this many more decibels in hand.
    /// For when what arrives is so wrong that nothing learned from it can
    /// be trusted.
    fn remeasure(&mut self, db: f64);

    fn health(&self) -> Health;

    /// Bits a second arriving with the table the far end is using: on the
    /// line, and of frames.
    fn rates(&self) -> (f64, f64);

    /// The band, slice by slice: a carrier at a time for the multicarrier
    /// mode, and all in one for the others.
    fn spectrum(&self) -> Vec<Bin>;

    /// Points for a symbol diagram: what arrived since this was last asked,
    /// as it stood when it was sliced, scaled so the outermost point of the
    /// grid is at one. Up to [`POINTS_KEPT`]; nothing for a receiver that
    /// slices nothing.
    fn points(&mut self) -> Vec<(f64, f64)> {
        Vec::new()
    }

    /// Bits a symbol of the grid those points are on, nought when there is
    /// none.
    fn shown_bits(&self) -> u8 {
        0
    }

    /// The Reed-Solomon code on what is arriving now, (n, k): octets a
    /// codeword and how many of them are data. The table says how many are
    /// check octets, so it changes when the table does.
    fn code(&self) -> Option<(usize, usize)> {
        None
    }

    /// On a line that is one pair of wires: the sample of this end's own
    /// signal that the echo in the next sample fed is the echo of, as the
    /// canceller in front of this receiver has placed it. Given before each
    /// [`Receiver::feed`]. A receiver that knows what the far end sent can
    /// take the echo further down than a canceller that does not; one that
    /// cannot ignores this.
    fn echo_under(&mut self, _sample: f64) {}

    /// The echo has moved: whatever was learned of it is to be forgotten.
    fn echo_again(&mut self) {}

    /// How much this receiver's own stage has taken off the echo it was
    /// handed, in decibels, if it has one and it has learned anything.
    fn echo_left_db(&self) -> Option<f64> {
        None
    }
}

/// Points a receiver keeps for whoever draws them. Past this it keeps no
/// more until they are taken, so one nobody draws costs nothing.
pub const POINTS_KEPT: usize = 2048;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_table_survives_being_written_down() {
        let mut bits = vec![0u8; 512];
        for (k, b) in bits.iter_mut().enumerate() {
            *b = match k {
                0..=3 => 0,
                4..=300 => 14,
                301..=420 => (14 - (k - 300) / 10) as u8,
                _ => 0,
            };
        }
        let table = Table { parity: 16, bits };
        let body = table.encode();
        assert!(body.len() < 80, "{} octets", body.len());
        assert_eq!(Table::decode(&body), Some(table));
        assert_eq!(Table::decode(&body[..body.len() - 1]), None);
        let long = Table { parity: 8, bits: vec![9; 2047] };
        assert_eq!(Table::decode(&long.encode()), Some(long));
    }
}
