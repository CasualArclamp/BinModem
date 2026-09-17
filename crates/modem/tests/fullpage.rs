//! A whole A4 page across a simulated line.
//!
//! Ignored by default: a page at 9600 bit/s takes most of a minute, and a
//! minute of audio at both ends is a minute of arithmetic. Worth having
//! anyway, because every other test here sends a handful of lines and a real
//! one sends eleven hundred and forty-three.

use modem::Modem;

const FS: f64 = 16_000.0;

fn a_full_page(resolution: fax::page::Resolution) -> fax::page::Page {
    let width = fax::page::WIDTH;
    fax::page::Page {
        lines: (0..resolution.lines())
            .map(|y| {
                (0..width)
                    .map(|x| {
                        // Something like a page of text: bands of short runs
                        // with white between them.
                        let band = (y / 24) % 3 != 2;
                        band && (x / 9 + y / 3) % 5 < 2 && (40..width - 40).contains(&x)
                    })
                    .collect()
            })
            .collect(),
        resolution,
    }
}

#[test]
#[ignore = "a minute of simulated line each way"]
fn a_whole_page_crosses() {
    for resolution in [fax::page::Resolution::Standard, fax::page::Resolution::Fine] {
        let page = a_full_page(resolution);
        let coded = fax::t4::encode(&page.lines).len();
        eprintln!(
            "{}: {} lines, {coded} bits, {:.0} s at 9600",
            resolution.name(),
            page.lines.len(),
            coded as f64 / 9600.0
        );

        let mut caller = Modem::new(FS);
        caller.fax_identification = "61399990000".to_owned();
        caller.fax_page = Some(page.clone());
        for line in ["AT+FCLASS=1\r", "ATD61388880000\r"] {
            for b in line.bytes() {
                caller.feed_dte(b);
            }
        }
        let mut answerer = Modem::new(FS);
        answerer.fax_identification = "61388880000".to_owned();
        for line in ["AT+FCLASS=1\r", "ATA\r"] {
            for b in line.bytes() {
                answerer.feed_dte(b);
            }
        }

        let (mut to_caller, mut to_answerer) = (0.0, 0.0);
        let mut arrived = None;
        let limit = (FS * (40.0 + coded as f64 / 4800.0 * 1.5)) as usize;
        for _ in 0..limit {
            let a = caller.step(to_caller);
            let b = answerer.step(to_answerer);
            to_caller = b;
            to_answerer = a;
            let _ = caller.take_dte();
            let _ = answerer.take_dte();
            if arrived.is_none() {
                arrived = answerer.take_received_page().map(|(_, page)| page);
            }
            // Everything a window reads, every sample, because a panel that
            // panics is a call that ends.
            let _ = caller.constellation_point();
            let _ = caller.take_symbol();
            let _ = caller.discriminator();
            let _ = caller.reception();
            let _ = caller.states();
            let _ = caller.shape();
            let _ = caller.standard();
            let _ = caller.line_phase();
            let _ = answerer.constellation_point();
            let _ = answerer.take_symbol();
            let _ = answerer.reception();
            let _ = answerer.shape();
            if arrived.is_some()
                && caller.fax_call().is_some_and(|c| c.phase().is_over())
                && answerer.fax_call().is_some_and(|c| c.phase().is_over())
            {
                break;
            }
        }

        let got = arrived.expect("no page reached the answering end");
        assert_eq!(got.lines.len(), page.lines.len(), "{}", resolution.name());
        assert_eq!(got.lines, page.lines, "{}", resolution.name());
    }
}
