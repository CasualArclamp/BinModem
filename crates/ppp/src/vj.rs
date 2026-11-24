//! Van Jacobson TCP/IP header compression (RFC 1144).
//!
//! Forty octets of IP and TCP header on every segment, and on a link this slow
//! the segments are small: an acknowledgement is nothing but header, and a
//! keystroke is forty-one octets of which one is the keystroke. 3.2.2 gets the
//! header down to three or four by sending, instead of the fields, only how
//! they changed since the last segment on the same connection -- and most of
//! them do not change, or change by exactly the amount of data that went past.
//!
//! The scheme is simplex (3.2.1): nothing flows from the decompressor back to
//! the compressor. Each end keeps the last header it sent or received for each
//! of a small number of connections, and the compressor decides everything;
//! the decompressor does what it is told. Recovery from a lost packet is TCP's
//! own retransmission arriving with a sequence number that went backwards,
//! which the compressor cannot compress and so sends whole, resynchronising the
//! far end (4.2).
//!
//! Over PPP the packet type travels in the protocol field rather than in the
//! top bit of the first octet as it did over SLIP (RFC 1332 4), which is why
//! the change mask here has seven bits in use and not eight.

use crate::ip;

/// RFC 1332 4: the three protocol numbers a datagram may travel under once
/// this is negotiated.
pub mod protocol {
    /// "The IP protocol is not TCP, or the packet is a fragment, or cannot be
    /// compressed."
    pub const IP: u16 = 0x0021;
    /// "The TCP/IP headers are replaced by the compressed header."
    pub const COMPRESSED_TCP: u16 = 0x002d;
    /// "The IP protocol field is replaced by the slot identifier."
    pub const UNCOMPRESSED_TCP: u16 = 0x002f;
}

/// What the compressor turned a datagram into, and what the decompressor is
/// being handed (3.2.1's TYPE_IP, UNCOMPRESSED_TCP and COMPRESSED_TCP).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Ip,
    Uncompressed,
    Compressed,
}

impl Kind {
    pub fn protocol(self) -> u16 {
        match self {
            Kind::Ip => protocol::IP,
            Kind::Uncompressed => protocol::UNCOMPRESSED_TCP,
            Kind::Compressed => protocol::COMPRESSED_TCP,
        }
    }

    pub fn from_protocol(number: u16) -> Option<Self> {
        match number {
            protocol::IP => Some(Kind::Ip),
            protocol::UNCOMPRESSED_TCP => Some(Kind::Uncompressed),
            protocol::COMPRESSED_TCP => Some(Kind::Compressed),
            _ => None,
        }
    }
}

/// The change mask of 3.2.2, with the bit values Appendix A gives them.
///
/// `C` says a connection number follows; `I` that the IP ID changed by
/// something other than one; `P` copies the TCP PUSH flag, which "can (and
/// does) change in any datagram"; and `S`, `A`, `W` and `U` say the sequence
/// number, acknowledgement, window and urgent pointer each changed and their
/// deltas follow, in the order U, W, A, S.
const C: u8 = 0x40;
const I: u8 = 0x20;
const P: u8 = 0x10;
const S: u8 = 0x08;
const A: u8 = 0x04;
const W: u8 = 0x02;
const U: u8 = 0x01;
/// The two special cases of 3.2.2, spelt with combinations that cannot
/// otherwise occur. "SWU" is echoed terminal traffic -- sequence and ack both
/// moved on by the last packet's data; "SAWU" is one-way data -- only the
/// sequence number did.
const SPECIAL_I: u8 = S | W | U;
const SPECIAL_D: u8 = S | A | W | U;
const SPECIALS: u8 = S | A | W | U;

/// TCP flag bits, at octet 13 of the TCP header.
const URG: u8 = 0x20;
const ACK: u8 = 0x10;
const PSH: u8 = 0x08;
const RST: u8 = 0x04;
const SYN: u8 = 0x02;
const FIN: u8 = 0x01;

/// What the two ends agreed about the slots (RFC 1332 4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Params {
    /// Max-Slot-Id: "one less than the actual number of slots".
    pub max_slot: u8,
    /// Comp-Slot-Id: whether the connection number may be left out when it is
    /// the same as last time. Only safe where the link can tell the
    /// decompressor about a damaged frame, which PPP's check sequence does.
    pub compress_slot: bool,
}

impl Params {
    /// RFC 1332 Appendix A: "at least 4 slots, usually 16 slots".
    pub const DEFAULT: Self = Self { max_slot: 15, compress_slot: true };
}

/// The IP header and the TCP header after it, as octet offsets.
struct Layout {
    /// Where the TCP header starts: the IP header length.
    tcp: usize,
    /// Where the data starts: both headers together.
    data: usize,
}

/// Read the two header lengths out of a datagram, if it is a whole IPv4
/// datagram carrying a TCP header.
fn layout(d: &[u8]) -> Option<Layout> {
    if d.len() < ip::HEADER_LEN || d[0] >> 4 != 4 {
        return None;
    }
    let tcp = usize::from(d[0] & 0x0f) * 4;
    if tcp < ip::HEADER_LEN || d.len() < tcp + 20 {
        return None;
    }
    let data = tcp + usize::from(d[tcp + 12] >> 4) * 4;
    if data < tcp + 20 || d.len() < data {
        return None;
    }
    let total = usize::from(be16(d, 2));
    if total < data || total > d.len() {
        return None;
    }
    Some(Layout { tcp, data })
}

fn be16(d: &[u8], at: usize) -> u16 {
    u16::from_be_bytes([d[at], d[at + 1]])
}

fn be32(d: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([d[at], d[at + 1], d[at + 2], d[at + 3]])
}

fn put16(d: &mut [u8], at: usize, v: u16) {
    d[at..at + 2].copy_from_slice(&v.to_be_bytes());
}

fn put32(d: &mut [u8], at: usize, v: u32) {
    d[at..at + 4].copy_from_slice(&v.to_be_bytes());
}

/// 3.2.2's variable-length numbers: "a change of one through 255 is
/// represented in one byte. Zero is improbable (a change of zero is never
/// sent) so a byte of zero signals an extension: the next two bytes are the
/// MSB and LSB, respectively, of a 16 bit value."
///
/// Zero does get sent, once: the IP ID delta when I is set may be zero, and
/// the urgent pointer may be. Both go as `00 00 00`, which is what the text
/// says zero encodes as.
fn encode(n: u16, out: &mut Vec<u8>) {
    if n == 0 || n > 255 {
        out.push(0);
        out.extend_from_slice(&n.to_be_bytes());
    } else {
        out.push(n as u8);
    }
}

/// The reverse, from `at`, moving `at` past what was read.
fn decode(d: &[u8], at: &mut usize) -> Option<u16> {
    let first = *d.get(*at)?;
    *at += 1;
    if first != 0 {
        return Some(u16::from(first));
    }
    let hi = *d.get(*at)?;
    let lo = *d.get(*at + 1)?;
    *at += 2;
    Some(u16::from_be_bytes([hi, lo]))
}

/// One connection's last header, at the sending end.
#[derive(Debug, Clone)]
struct Saved {
    header: Vec<u8>,
    /// When it was last used, for choosing which to give up.
    used: u64,
}

/// The sending end (3.2.3).
#[derive(Debug)]
pub struct Compressor {
    slots: Vec<Option<Saved>>,
    /// "The connection number is recorded as the last connection sent on this
    /// serial line", so the next packet on it can leave the number out.
    last_sent: Option<u8>,
    compress_slot: bool,
    clock: u64,
}

impl Compressor {
    pub fn new(params: Params) -> Self {
        Self {
            slots: vec![None; usize::from(params.max_slot) + 1],
            last_sent: None,
            compress_slot: params.compress_slot,
            clock: 0,
        }
    }

    /// Turn a datagram into what goes on the link, and say what it is.
    ///
    /// The decision procedure of 3.2.3, in its order. Anything that is not a
    /// whole TCP datagram this end can compress goes as it came, and "the
    /// compressor's state is not changed in any way".
    pub fn compress(&mut self, d: &[u8]) -> (Kind, Vec<u8>) {
        let Some(at) = layout(d) else {
            return (Kind::Ip, d.to_vec());
        };
        if d[9] != ip::PROTOCOL_TCP {
            return (Kind::Ip, d.to_vec());
        }
        // "If the packet is an IP fragment (i.e., either the fragment offset
        // field is non-zero or the more fragments bit is set), send it as
        // TYPE_IP."
        if be16(d, 6) & 0x3fff != 0 {
            return (Kind::Ip, d.to_vec());
        }
        // "If any of the TCP control bits SYN, FIN or RST are set or if the
        // ACK bit is clear, consider the packet uncompressible."
        let flags = d[at.tcp + 13];
        if flags & (SYN | FIN | RST | ACK) != ACK {
            return (Kind::Ip, d.to_vec());
        }

        self.clock += 1;
        let Some(slot) = self.find(d, &at) else {
            // "Some state is reclaimed (which should probably be the least
            // recently used) and an UNCOMPRESSED_TCP packet is sent."
            let slot = self.reclaim();
            return self.uncompressed(d, &at, slot);
        };
        let old = self.slots[usize::from(slot)]
            .as_ref()
            .map(|s| s.header.clone())
            .unwrap_or_default();
        let old_at = match layout_of_header(&old) {
            Some(l) => l,
            None => return self.uncompressed(d, &at, slot),
        };

        // "The remaining fields to check are protocol version, header length,
        // type of service, don't fragment, time-to-live, data offset, IP
        // options (if any) and TCP options (if any). If any of these fields
        // differ between the two headers, an UNCOMPRESSED_TCP packet is
        // sent."
        let unchanging = d[0] == old[0]
            && d[1] == old[1]
            && d[6..8] == old[6..8]
            && d[8] == old[8]
            && d[at.tcp + 12] == old[old_at.tcp + 12]
            && d[ip::HEADER_LEN..at.tcp] == old[ip::HEADER_LEN..old_at.tcp]
            && d[at.tcp + 20..at.data] == old[old_at.tcp + 20..old_at.data];
        if !unchanging {
            return self.uncompressed(d, &at, slot);
        }

        let mut mask = 0u8;
        let mut deltas = Vec::with_capacity(12);

        // "If the URG flag is set, the urgent data field is encoded (note
        // that it may be zero) and the U bit is set... if URG is clear, the
        // urgent data field must be checked against the previous packet and,
        // if it changes, an UNCOMPRESSED_TCP packet is sent."
        let urgent = be16(d, at.tcp + 18);
        if flags & URG != 0 {
            encode(urgent, &mut deltas);
            mask |= U;
        } else if urgent != be16(&old, old_at.tcp + 18) {
            return self.uncompressed(d, &at, slot);
        }
        // The window "is also the difference between the current and previous
        // values. However, either positive or negative changes are allowed
        // since the window is a 16 bit field."
        let dw = be16(d, at.tcp + 14).wrapping_sub(be16(&old, old_at.tcp + 14));
        if dw != 0 {
            encode(dw, &mut deltas);
            mask |= W;
        }
        // Ack and sequence: "an uncompressed packet is sent if the difference
        // is negative or more than 64K".
        let da = be32(d, at.tcp + 8).wrapping_sub(be32(&old, old_at.tcp + 8));
        if da > 0xffff {
            return self.uncompressed(d, &at, slot);
        }
        if da != 0 {
            encode(da as u16, &mut deltas);
            mask |= A;
        }
        let ds = be32(d, at.tcp + 4).wrapping_sub(be32(&old, old_at.tcp + 4));
        if ds > 0xffff {
            return self.uncompressed(d, &at, slot);
        }
        if ds != 0 {
            encode(ds as u16, &mut deltas);
            mask |= S;
        }

        // The special cases, and the combinations that must not be mistaken
        // for them.
        let last_data = usize::from(be16(&old, 2)).saturating_sub(old_at.data) as u32;
        let this_data = (usize::from(be16(d, 2)) - at.data) as u32;
        match mask & SPECIALS {
            // "To avoid ambiguity, an uncompressed packet is sent if the
            // actual changes in a packet are S * W U."
            m if m & (S | W | U) == (S | W | U) => return self.uncompressed(d, &at, slot),
            // "If only S is set, check if the change equals the amount of
            // user data in the last packet."
            m if m == S && ds == last_data => {
                mask = (mask & !SPECIALS) | SPECIAL_D;
                deltas.clear();
            }
            // "If only S and A are set, check if they both changed by the
            // same amount and that amount is the amount of user data in the
            // last packet."
            m if m == S | A && ds == da && ds == last_data => {
                mask = (mask & !SPECIALS) | SPECIAL_I;
                deltas.clear();
            }
            // "If nothing changed, check if this packet has no user data (in
            // which case it is probably a duplicate ack or window probe) or if
            // the previous packet contained user data (which means this packet
            // is a retransmission on a connection with no pipelining). In
            // either of these cases, send an UNCOMPRESSED_TCP packet."
            0 if this_data == 0 || last_data != 0 => return self.uncompressed(d, &at, slot),
            _ => {}
        }

        // "The change in the packet ID is computed and, if not one, the
        // difference is encoded (note that it may be zero or negative)."
        let di = be16(d, 4).wrapping_sub(be16(&old, 4));
        if di != 1 {
            encode(di, &mut deltas);
            mask |= I;
        }
        if flags & PSH != 0 {
            mask |= P;
        }

        self.save(d, &at, slot);
        // The connection number goes only when it is not the one that went
        // last, and always when the far end said it must (Comp-Slot-Id 0).
        let name_it = !self.compress_slot || self.last_sent != Some(slot);
        self.last_sent = Some(slot);
        if name_it {
            mask |= C;
        }
        let mut out = Vec::with_capacity(4 + deltas.len() + (d.len() - at.data));
        out.push(mask);
        if name_it {
            out.push(slot);
        }
        // "The unmodified TCP checksum so the end-to-end data integrity check
        // will still be valid."
        out.extend_from_slice(&d[at.tcp + 16..at.tcp + 18]);
        out.extend_from_slice(&deltas);
        out.extend_from_slice(&d[at.data..]);
        (Kind::Compressed, out)
    }

    /// "An UNCOMPRESSED_TCP packet is identical to the input packet except the
    /// IP protocol field (byte 9) is changed from 6 (protocol TCP) to a
    /// connection number."
    ///
    /// The IP header checksum is left as it was, and so is wrong on the line;
    /// the far end puts the protocol back before anything reads it, and it is
    /// right again.
    fn uncompressed(&mut self, d: &[u8], at: &Layout, slot: u8) -> (Kind, Vec<u8>) {
        self.save(d, at, slot);
        self.last_sent = Some(slot);
        let mut out = d.to_vec();
        out[9] = slot;
        (Kind::Uncompressed, out)
    }

    fn save(&mut self, d: &[u8], at: &Layout, slot: u8) {
        self.slots[usize::from(slot)] = Some(Saved {
            header: d[..at.data].to_vec(),
            used: self.clock,
        });
    }

    /// The slot holding this connection: same addresses, same ports.
    fn find(&mut self, d: &[u8], at: &Layout) -> Option<u8> {
        let ports = &d[at.tcp..at.tcp + 4];
        for (i, slot) in self.slots.iter_mut().enumerate() {
            let Some(saved) = slot else { continue };
            let Some(old) = layout_of_header(&saved.header) else { continue };
            if saved.header[12..20] == d[12..20] && saved.header[old.tcp..old.tcp + 4] == *ports {
                saved.used = self.clock;
                return Some(i as u8);
            }
        }
        None
    }

    /// An empty slot, or failing that the one used longest ago.
    fn reclaim(&self) -> u8 {
        let mut oldest = 0usize;
        let mut when = u64::MAX;
        for (i, slot) in self.slots.iter().enumerate() {
            match slot {
                None => return i as u8,
                Some(s) if s.used < when => {
                    when = s.used;
                    oldest = i;
                }
                Some(_) => {}
            }
        }
        oldest as u8
    }
}

/// The layout of a saved header, which has no data after it.
fn layout_of_header(h: &[u8]) -> Option<Layout> {
    if h.len() < ip::HEADER_LEN + 20 {
        return None;
    }
    let tcp = usize::from(h[0] & 0x0f) * 4;
    if tcp < ip::HEADER_LEN || h.len() < tcp + 20 {
        return None;
    }
    let data = tcp + usize::from(h[tcp + 12] >> 4) * 4;
    if data != h.len() {
        return None;
    }
    Some(Layout { tcp, data })
}

/// The receiving end (3.2.4).
#[derive(Debug)]
pub struct Decompressor {
    slots: Vec<Option<Vec<u8>>>,
    last_received: Option<u8>,
    /// 4.1: after a damaged frame, "packets are discarded until the receiver
    /// gets an explicit connection number". Without it a compressed packet
    /// belonging to one conversation would be applied to another, and the TCP
    /// checksum has one chance in 65536 of not noticing.
    toss: bool,
}

impl Decompressor {
    pub fn new(params: Params) -> Self {
        Self {
            slots: vec![None; usize::from(params.max_slot) + 1],
            last_received: None,
            toss: false,
        }
    }

    /// The framer saw a frame it could not accept (3.2.4's TYPE_ERROR).
    pub fn error(&mut self) {
        self.toss = true;
    }

    /// Whether packets are being thrown away waiting for a connection number.
    pub fn tossing(&self) -> bool {
        self.toss
    }

    /// Turn what came off the link back into the datagram that was sent, or
    /// nothing if it cannot be trusted.
    pub fn decompress(&mut self, kind: Kind, packet: &[u8]) -> Option<Vec<u8>> {
        match kind {
            Kind::Ip => Some(packet.to_vec()),
            Kind::Uncompressed => self.uncompressed(packet),
            Kind::Compressed => self.compressed(packet),
        }
    }

    fn uncompressed(&mut self, packet: &[u8]) -> Option<Vec<u8>> {
        let Some(at) = layout(packet) else {
            self.toss = true;
            return None;
        };
        let slot = packet[9];
        if usize::from(slot) >= self.slots.len() {
            self.toss = true;
            return None;
        }
        // "The toss flag is cleared, the index is copied to the state's last
        // connection received field... the TCP protocol number is restored to
        // the IP protocol field, the packet header is copied to the indicated
        // state slot."
        self.toss = false;
        self.last_received = Some(slot);
        let mut out = packet.to_vec();
        out[9] = ip::PROTOCOL_TCP;
        self.slots[usize::from(slot)] = Some(out[..at.data].to_vec());
        Some(out)
    }

    fn compressed(&mut self, packet: &[u8]) -> Option<Vec<u8>> {
        let mask = *packet.first()?;
        let mut at = 1usize;
        if mask & C != 0 {
            let slot = *packet.get(at)?;
            at += 1;
            if usize::from(slot) >= self.slots.len() {
                self.toss = true;
                return None;
            }
            self.last_received = Some(slot);
            self.toss = false;
        } else if self.toss {
            return None;
        }
        let slot = usize::from(self.last_received?);
        // Taken out and put back rather than edited in place: the failures
        // below all have to set the toss flag, and they cannot do that while
        // a borrow of the slot is still alive.
        let Some(mut header) = self.slots[slot].clone() else {
            // A connection number nothing has seeded: a packet this end cannot
            // rebuild, and a sign the two ends disagree.
            self.toss = true;
            return None;
        };
        let saved = &mut header;
        let Some(h) = layout_of_header(saved) else {
            self.toss = true;
            return None;
        };

        // "The next two bytes in the incoming packet are the TCP checksum."
        let sum = [*packet.get(at)?, *packet.get(at + 1)?];
        at += 2;
        saved[h.tcp + 16..h.tcp + 18].copy_from_slice(&sum);
        if mask & P != 0 {
            saved[h.tcp + 13] |= PSH;
        } else {
            saved[h.tcp + 13] &= !PSH;
        }

        let last_data = u32::from(be16(saved, 2)).saturating_sub(h.data as u32);
        let mut seq = be32(saved, h.tcp + 4);
        let mut ack = be32(saved, h.tcp + 8);
        match mask & SPECIALS {
            SPECIAL_D => seq = seq.wrapping_add(last_data),
            SPECIAL_I => {
                seq = seq.wrapping_add(last_data);
                ack = ack.wrapping_add(last_data);
            }
            _ => {
                if mask & U != 0 {
                    saved[h.tcp + 13] |= URG;
                    let urgent = decode(packet, &mut at)?;
                    put16(saved, h.tcp + 18, urgent);
                } else {
                    saved[h.tcp + 13] &= !URG;
                }
                if mask & W != 0 {
                    let dw = decode(packet, &mut at)?;
                    let window = be16(saved, h.tcp + 14).wrapping_add(dw);
                    put16(saved, h.tcp + 14, window);
                }
                if mask & A != 0 {
                    ack = ack.wrapping_add(u32::from(decode(packet, &mut at)?));
                }
                if mask & S != 0 {
                    seq = seq.wrapping_add(u32::from(decode(packet, &mut at)?));
                }
            }
        }
        put32(saved, h.tcp + 4, seq);
        put32(saved, h.tcp + 8, ack);
        // "If the I bit is set... decoded and added to the IP ID field of the
        // saved packet. Otherwise, one is added to the IP ID."
        let di = if mask & I != 0 { decode(packet, &mut at)? } else { 1 };
        let id = be16(saved, 4).wrapping_add(di);
        put16(saved, 4, id);

        // "The length of the remaining data is added to the length of the
        // saved IP and TCP headers and the result is put into the saved IP
        // total length field. The saved IP header is now up to date so its
        // checksum is recalculated."
        let data = &packet[at..];
        let total = (h.data + data.len()) as u16;
        put16(saved, 2, total);
        put16(saved, 10, 0);
        let sum = ip::checksum(&saved[..h.tcp]);
        put16(saved, 10, sum);

        let mut out = Vec::with_capacity(h.data + data.len());
        out.extend_from_slice(saved);
        out.extend_from_slice(data);
        // The slot now holds this packet's header, which is what the next
        // one's changes will be measured against.
        self.slots[slot] = Some(header);
        Some(out)
    }
}

#[cfg(test)]
mod tests;
