//! A file crossing a call, through everything.
//!
//! Every other test here proves one layer. This is the only one that puts the
//! whole stack together: V.8 agrees a modulation, the data pump carries the bits,
//! V.42 makes them reliable, V.42bis compresses them, and ZMODEM moves a file
//! over the top of all of it -- with nothing knowing about anything below it.

use modem::{Modem, State};
use transfer::zmodem::{FileInfo, Receiver, Sender};

const FS: f64 = 16_000.0;

/// Two modems on one line, connected, with a file going one way.
fn transfer(data: Vec<u8>) -> (Vec<u8>, Option<Vec<u8>>) {
    let mut caller = Modem::new(FS);
    let mut host = Modem::new(FS);
    for m in [&mut caller, &mut host] {
        for b in b"AT+MS=V32,0,9600,9600\r" {
            m.feed_dte(*b);
        }
        m.take_dte();
    }
    for b in b"ATA\r" {
        host.feed_dte(*b);
    }
    for b in b"ATD5551234\r" {
        caller.feed_dte(*b);
    }

    let file = FileInfo {
        name: "GOING.BIN".into(),
        length: Some(data.len() as u64),
        modified: Some(0x6512_3456),
        mode: 0,
    };
    let mut tx: Option<Sender> = None;
    let mut rx = Receiver::default();

    let (mut from_caller, mut from_host) = (0.0, 0.0);
    for i in 0..(90.0 * FS) as usize {
        let (a, b) = (from_caller, from_host);
        from_caller = caller.step(b);
        from_host = host.step(a);

        // Once the call is up, start the transfer and let the two halves talk
        // through the modems' terminal interfaces -- which is exactly where a
        // real one lives.
        let up = caller.state() == State::Data && host.state() == State::Data;
        if up && tx.is_none() {
            let rate = caller.rate().unwrap_or(2400);
            tx = Some(Sender::new(file.clone(), data.clone(), rate));
        }
        if let Some(sender) = tx.as_mut() {
            for byte in caller.take_dte() {
                sender.feed(&[byte]);
            }
            for byte in sender.take_out() {
                caller.feed_dte(byte);
            }
            for byte in host.take_dte() {
                rx.feed(&[byte]);
            }
            for byte in rx.take_out() {
                host.feed_dte(byte);
            }
            // A millisecond of the line is a millisecond for the protocol.
            if i % (FS as usize / 1000) == 0 {
                sender.tick(1);
                rx.tick(1);
            }
            if rx.finished().is_some() {
                break;
            }
        } else {
            caller.take_dte();
            host.take_dte();
        }
    }
    (data, rx.finished().map(|f| f.data))
}

#[test]
fn a_file_crosses_a_call() {
    // Text, which V.42bis will squeeze hard -- so this also proves the file
    // that comes out the far side is the file and not the compressed form of
    // it, which is a thing that can only go wrong when both are running.
    let data = b"MAIN MENU\r\n[1] Messages\r\n[2] Files\r\n[3] Doors\r\n".repeat(40);
    let (sent, got) = transfer(data);
    assert_eq!(got.as_ref(), Some(&sent), "the file did not arrive intact");
}

#[test]
fn a_binary_file_crosses_a_call() {
    // Every byte value, including the seven ZMODEM escapes, the four subpacket
    // endings, and the HDLC flag that V.42 frames with underneath. Each of
    // those is a layer that could end early on data meant for the layer above.
    let data: Vec<u8> = (0..=255u8).cycle().take(1800).collect();
    let (sent, got) = transfer(data);
    assert_eq!(got.as_ref(), Some(&sent), "the file did not arrive intact");
}
