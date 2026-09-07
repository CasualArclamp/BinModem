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
