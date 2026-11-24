//! A ZMODEM sender and receiver moving a file between them.
//!
//! The unit tests prove each layer on its own: an escape that round trips, a
//! header that decodes, a subpacket whose check catches a flipped bit. None of
//! them proves the two halves agree about what happens next, which is the only
//! thing that matters when a real board is on the other end.

use transfer::zmodem::send::Failure;
use transfer::zmodem::{FileInfo, Receiver, Sender, State};

/// How much crosses the line in one round.
///
/// A modem hands over a few hundred bytes at a time, and a harness that moves
/// a whole file in two rounds is not one a per-round fault can be injected
/// into -- nor one where a cancel part-way through has anything to interrupt.
const PIECE: usize = 200;

/// How much the line will hold that has not gone out yet.
///
/// A modem's own queue, which is the thing a sender has to be stopped from
/// filling. Without one modelled here, the harness takes everything the sender
/// offers the instant it is offered -- and a sender that is never told to stop
/// hands over the whole file, which is exactly the arrangement that made one
/// error on a real transfer cost half a megabyte.
const QUEUE: usize = 1024;

/// Run the two ends against each other, optionally spoiling the line.
///
/// `damage` is called with each piece going from sender to receiver and may
/// change it, which is where a lossy line comes from.
fn run<F>(file: FileInfo, data: Vec<u8>, mut damage: F) -> (Sender, Receiver)
where
    F: FnMut(usize, &mut Vec<u8>),
{
    let mut tx = Sender::new(file, data, 2400);
    let mut rx = Receiver::default();
    let (mut to_rx, mut to_tx): (Vec<u8>, Vec<u8>) = (Vec::new(), Vec::new());
    for round in 0..200_000 {
        tx.set_room(QUEUE.saturating_sub(to_rx.len()));
        to_rx.extend(tx.take_out());
        to_tx.extend(rx.take_out());

        let mut going: Vec<u8> = to_rx.drain(..to_rx.len().min(PIECE)).collect();
        let coming: Vec<u8> = to_tx.drain(..to_tx.len().min(PIECE)).collect();
        if !going.is_empty() {
            damage(round, &mut going);
            rx.feed(&going);
        }
        if !coming.is_empty() {
            tx.feed(&coming);
        }
        // A round with nothing crossing either way is time passing, which is
        // what the timers are for; without it a stalled transfer would spin.
        if going.is_empty() && coming.is_empty() {
            tx.tick(100);
            rx.tick(100);
        }
        if matches!(tx.state(), State::Done | State::Failed(_)) && rx.finished().is_some() {
            break;
        }
        if matches!(rx.state(), State::Failed(_)) {
            break;
        }
    }
    (tx, rx)
}

fn plain(name: &str, data: &[u8]) -> FileInfo {
    FileInfo {
        name: name.into(),
        length: Some(data.len() as u64),
        modified: Some(0x6512_3456),
        mode: 0o100644,
    }
}

#[test]
fn a_file_goes_across() {
    let data = b"MAIN MENU\r\n[1] Messages\r\n[2] Files\r\n".repeat(200);
    let (tx, rx) = run(plain("MENU.ANS", &data), data.clone(), |_, _| {});

    assert_eq!(tx.state(), State::Done, "the sender did not finish");
    let got = rx.finished().expect("the receiver has no file");
    assert_eq!(got.data, data, "what arrived is not what was sent");
    assert_eq!(got.file.name, "MENU.ANS");
    assert_eq!(got.file.length, Some(data.len() as u64));
    assert_eq!(rx.damaged(), 0, "a clean line damaged something");
}

#[test]
fn every_byte_value_goes_across() {
    // What a binary file is. Among these are the seven characters 7.2 escapes,
    // the four subpacket endings, and the ZPAD that starts a header -- each of
    // which is a way for a transfer to end early with a file that looks fine.
    let data: Vec<u8> = (0..=255u8).cycle().take(9000).collect();
    let (tx, rx) = run(plain("BINARY.DAT", &data), data.clone(), |_, _| {});
    assert_eq!(tx.state(), State::Done);
    assert_eq!(rx.finished().expect("no file").data, data);
}

#[test]
fn an_empty_file_goes_across() {
    let (tx, rx) = run(plain("EMPTY.TXT", &[]), Vec::new(), |_, _| {});
    assert_eq!(tx.state(), State::Done);
    let got = rx.finished().expect("no file");
    assert!(got.data.is_empty());
    assert_eq!(got.file.name, "EMPTY.TXT");
}

#[test]
fn a_file_larger_than_one_subpacket_goes_across() {
    // 7.4 caps a subpacket at 1024 bytes, so anything bigger exercises the
    // frame continuing -- which is where the ZCRCG ending and the position in
    // each ZDATA header have to agree with each other.
    let data: Vec<u8> = (0..40_000u32).map(|i| (i % 251) as u8).collect();
    let (tx, rx) = run(plain("BIG.ZIP", &data), data.clone(), |_, _| {});
    assert_eq!(tx.state(), State::Done);
    assert_eq!(rx.finished().expect("no file").data, data);
}

#[test]
fn a_damaged_subpacket_is_recovered_from() {
    // 8.2's whole error recovery: the receiver says where it has got to and
    // the sender goes back there. Nothing is numbered and no gaps are
    // remembered, because nothing past the damage was ever accepted.
    let data: Vec<u8> = (0..20_000u32).map(|i| (i % 253) as u8).collect();
    let mut spoiled = 0;
    let (tx, rx) = run(plain("LOSSY.BIN", &data), data.clone(), |round, chunk| {
        // Every so often, turn one byte over in the middle of what is going
        // out. Not the first bytes, which are the header this recovers by.
        if round % 17 == 5 && chunk.len() > 40 {
            let at = chunk.len() / 2;
            chunk[at] ^= 0xFF;
            spoiled += 1;
        }
    });

    assert!(spoiled > 5, "the line was not damaged enough to prove anything");
    assert_eq!(tx.state(), State::Done, "the transfer did not survive");
    let got = rx.finished().expect("no file");
    assert_eq!(got.data, data, "what arrived is not what was sent");
    assert!(rx.damaged() > 0, "nothing was detected as damaged");
    assert!(tx.progress().rewinds > 0, "the sender was never sent back");
}

#[test]
fn what_arrives_is_what_was_sent_or_nothing_at_all() {
    // The property that matters more than finishing. A transfer may fail --
    // lines do -- but a file that arrives must be the file, and a checksum
    // that passed on damaged data would be worse than no transfer at all.
    let data: Vec<u8> = (0..15_000u32).map(|i| (i.wrapping_mul(2654435761) >> 24) as u8).collect();
    for period in [3usize, 5, 11, 23] {
        let (_, rx) = run(plain("HARSH.BIN", &data), data.clone(), |round, chunk| {
            if round % period == 1 && chunk.len() > 8 {
                for at in (0..chunk.len()).step_by(chunk.len().max(1) / 4 + 1) {
                    chunk[at] ^= 0x5A;
                }
            }
        });
        if let Some(got) = rx.finished() {
            assert_eq!(got.data, data, "damaged data was accepted at period {period}");
        }
    }
}

#[test]
fn a_receiver_that_cancels_stops_the_sender() {
    // 8.4: eight CAN characters. The sender has to notice in the middle of
    // streaming, which is the point -- it is not waiting for anything.
    let data: Vec<u8> = vec![b'x'; 50_000];
    let mut cancelled = false;
    let (tx, _) = run(plain("STOP.BIN", &data), data.clone(), |_, _| {});
    assert_eq!(tx.state(), State::Done, "the harness cannot move this file at all");

    // And now the same transfer, interrupted part-way through.
    let mut tx = Sender::new(plain("STOP.BIN", &data), data, 9600);
    let mut rx = Receiver::default();
    let (mut to_rx, mut to_tx): (Vec<u8>, Vec<u8>) = (Vec::new(), Vec::new());
    for _ in 0..20_000 {
        to_rx.extend(tx.take_out());
        to_tx.extend(rx.take_out());
        let going: Vec<u8> = to_rx.drain(..to_rx.len().min(PIECE)).collect();
        let coming: Vec<u8> = to_tx.drain(..to_tx.len().min(PIECE)).collect();
        rx.feed(&going);
        tx.feed(&coming);
        // Once some of the file has actually crossed, so there is a transfer
        // in flight to interrupt rather than a handshake.
        if !cancelled && rx.progress().position > 4000 {
            cancelled = true;
            rx.cancel();
            to_tx.clear();
        }
        if matches!(tx.state(), State::Failed(_)) {
            break;
        }
    }
    assert!(cancelled, "the transfer never got far enough to be cancelled");
    assert_eq!(tx.state(), State::Failed(Failure::Cancelled));
}

#[test]
fn a_far_end_that_never_speaks_is_given_up_on() {
    // 8.1 gives forty seconds before a session that will not start stops
    // pretending. Nothing is fed in, so nothing ever answers.
    let mut tx = Sender::new(plain("NOBODY.TXT", b"hello"), b"hello".to_vec(), 2400);
    for _ in 0..500 {
        tx.take_out();
        tx.tick(100);
    }
    assert_eq!(tx.state(), State::Failed(Failure::NoAnswer));
}

#[test]
fn a_board_talking_first_does_not_confuse_the_receiver() {
    // What a real transfer starts with: a board saying something to a person,
    // then the protocol. 8.1 has the sender "display a message intended for
    // human consumption" before the ZRQINIT.
    let data = b"the file itself".to_vec();
    let mut tx = Sender::new(plain("FILE.TXT", &data), data.clone(), 2400);
    let mut rx = Receiver::default();
    rx.feed(b"\r\nSending FILE.TXT (15 bytes). Ctrl-X twice to abort.\r\n");
    rx.feed(b"*** please wait ***\r\n");
    for _ in 0..2000 {
        let going = tx.take_out();
        rx.feed(&going);
        let coming = rx.take_out();
        tx.feed(&coming);
        if rx.finished().is_some() {
            break;
        }
    }
    assert_eq!(rx.finished().expect("no file").data, data);
}

#[test]
fn the_progress_adds_up() {
    // What the window will show, and it has to reach the end rather than
    // stopping wherever the last acknowledgement was.
    let data: Vec<u8> = vec![b'z'; 12_345];
    let (tx, rx) = run(plain("SIZE.BIN", &data), data.clone(), |_, _| {});
    let p = tx.progress();
    assert_eq!(p.position, data.len() as u64);
    assert_eq!(p.total, Some(data.len() as u64));
    assert_eq!(rx.progress().position, data.len() as u64);
    assert_eq!(rx.progress().total, Some(data.len() as u64));
}

#[test]
fn a_rewind_costs_what_is_in_flight_and_not_the_rest_of_the_file() {
    // The fault a real transfer showed and no test here could: the sender had
    // handed the whole file to the line's queue, so when the receiver asked it
    // to go back, everything already queued was stale and still had to be sent
    // before the answer to that question was even begun. One error, half a
    // megabyte resent, and the far end stopped dead at the position it had
    // asked for while the line spent minutes delivering data it had refused.
    //
    // The cost of an error should be what is in flight, which is the queue --
    // and nothing beyond it.
    let data: Vec<u8> = (0..60_000u32).map(|i| (i % 251) as u8).collect();
    let mut spoiled = 0;
    let (tx, rx) = run(plain("INFLIGHT.BIN", &data), data.clone(), |round, chunk| {
        // Once, in the middle, and only once.
        if spoiled == 0 && round > 60 && chunk.len() > 40 {
            let at = chunk.len() / 2;
            chunk[at] ^= 0xFF;
            spoiled += 1;
        }
    });

    assert_eq!(spoiled, 1, "the line was not damaged exactly once");
    assert_eq!(tx.state(), State::Done, "the transfer did not survive");
    assert_eq!(rx.finished().expect("no file").data, data);

    let resent = tx.progress().resent;
    assert!(tx.progress().rewinds > 0, "the sender was never sent back");
    // What is in flight is the queue plus whatever subpacket was being built
    // when the answer arrived. Generous, and still nowhere near the file.
    let in_flight = (QUEUE + subpacket_len()) as u64;
    assert!(
        resent <= in_flight * 3,
        "one error resent {resent} bytes of a {} byte file; in flight was {in_flight}",
        data.len()
    );
}

/// The subpacket size the harness's sender is using, from 7.4.
fn subpacket_len() -> usize {
    transfer::zmodem::subpacket::recommended_length(2400)
}

/// What `sz -e ONE.TXT TWO.BIN` puts on the line, in the order it does.
///
/// This end's own Sender sends one file and never a ZSINIT, so it cannot
/// show either of the two things a real sender did to the receiver: a ZSINIT
/// at the start, which went unanswered until `sz` gave up, and a batch, of
/// which only the last file came out.
fn a_batch_as_sz_sends_it(files: &[(&str, &[u8])]) -> Vec<Vec<u8>> {
    use transfer::zmodem::Kind;
    use transfer::zmodem::{Ending, Header, Style, capability, subpacket};

    let mut steps = Vec::new();
    steps.push(Header::position(Kind::Rqinit, 0).encode(Style::Hex));
    // 8.1: a ZSINIT asking for control characters escaped goes as a hex
    // header, and its subpacket carries the attention string.
    let mut sinit = Header::flags(Kind::Sinit, 0, 0, 0, capability::ESCCTL).encode(Style::Hex);
    sinit.extend(subpacket::encode(b"\x03\x8e\0", Ending::Wait, false, 0));
    steps.push(sinit);
    for (name, data) in files {
        let mut offer = Header::position(Kind::File, 0).encode(Style::Binary32);
        offer.extend(subpacket::encode(&plain(name, data).encode(), Ending::Wait, true, 0));
        steps.push(offer);
        // 7.4's longest subpacket, as `sz` sends them, and the last one
        // ending the frame.
        let mut body = Header::position(Kind::Data, 0).encode(Style::Binary32);
        let pieces: Vec<&[u8]> = data.chunks(1024).collect();
        for (i, piece) in pieces.iter().enumerate() {
            let ending = if i + 1 == pieces.len() { Ending::End } else { Ending::Go };
            body.extend(subpacket::encode(piece, ending, true, 0));
        }
        body.extend(Header::position(Kind::Eof, data.len() as u32).encode(Style::Hex));
        steps.push(body);
    }
    steps.push(Header::position(Kind::Fin, 0).encode(Style::Hex));
    steps.push(b"OO".to_vec());
    steps
}

/// The headers the receiver said, by kind.
fn said(bytes: &[u8]) -> Vec<transfer::zmodem::Kind> {
    let mut kinds = Vec::new();
    let mut rest = bytes;
    while let Some(start) = transfer::zmodem::header::find(rest) {
        rest = &rest[start..];
        match transfer::zmodem::header::decode(rest) {
            Ok((h, _, used)) => {
                kinds.push(h.kind);
                rest = &rest[used..];
            }
            Err(_) => rest = &rest[1..],
        }
    }
    kinds
}

#[test]
fn a_zsinit_is_answered_with_a_zack() {
    use transfer::zmodem::Kind;

    let steps = a_batch_as_sz_sends_it(&[("ONE.TXT", b"one")]);
    let mut rx = Receiver::default();
    rx.take_out();
    rx.feed(&steps[0]);
    rx.take_out();
    rx.feed(&steps[1]);
    assert_eq!(said(&rx.take_out()), vec![Kind::Ack], "the ZSINIT went unanswered");
    for step in &steps[2..] {
        rx.feed(step);
    }
    assert_eq!(rx.state(), State::Done);
    assert_eq!(rx.finished().expect("no file").data, b"one");
}

#[test]
fn every_file_of_a_batch_is_kept() {
    let one = b"MAIN MENU\r\n".repeat(300);
    let two: Vec<u8> = (0..50_000u32).map(|i| (i % 253) as u8).collect();
    let three = b"bye".to_vec();
    let files: [(&str, &[u8]); 3] = [("ONE.TXT", &one), ("TWO.BIN", &two), ("THREE", &three)];
    let mut rx = Receiver::default();
    let mut arrived = Vec::new();
    for step in a_batch_as_sz_sends_it(&files) {
        rx.feed(&step);
        rx.take_out();
        arrived.extend(rx.take_arrived());
    }
    assert_eq!(rx.state(), State::Done);
    let names: Vec<&str> = arrived.iter().map(|r| r.file.name.as_str()).collect();
    assert_eq!(names, ["ONE.TXT", "TWO.BIN", "THREE"]);
    assert_eq!(arrived[0].data, one);
    assert_eq!(arrived[1].data, two);
    assert_eq!(arrived[2].data, three);
    assert_eq!(rx.finished().expect("no file").data, three, "finished() is the last file");
    assert!(rx.take_arrived().is_empty(), "a file was handed up twice");
}

#[test]
fn a_batch_that_fails_partway_keeps_what_had_arrived() {
    let one = b"first".to_vec();
    let two = b"second, never finished".to_vec();
    let files: [(&str, &[u8]); 2] = [("ONE.TXT", &one), ("TWO.TXT", &two)];
    let steps = a_batch_as_sz_sends_it(&files);
    let mut rx = Receiver::default();
    let mut arrived = Vec::new();
    // Up to the second file's offer, and then the line goes quiet.
    for step in &steps[..steps.len() - 3] {
        rx.feed(step);
        arrived.extend(rx.take_arrived());
    }
    for _ in 0..1_000 {
        rx.tick(1_000);
    }
    arrived.extend(rx.take_arrived());
    assert!(matches!(rx.state(), State::Failed(_)), "the receiver is {:?}", rx.state());
    assert_eq!(arrived.len(), 1);
    assert_eq!(arrived[0].file.name, "ONE.TXT");
    assert_eq!(arrived[0].data, one);
}
