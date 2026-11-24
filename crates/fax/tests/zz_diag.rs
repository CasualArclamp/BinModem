use datapump::v21;
use ec::hdlc::{Decoder, Fcs};

#[test]
#[ignore = "diag"]
fn diag() {
    let path = std::env::var("FAX_CAPTURE").unwrap();
    let wav = line::wav::read(&path).unwrap();
    let fs = f64::from(wav.sample_rate);
    for channel in 0..2 {
        let samples = wav.channel(channel);
        let mut rx = v21::Receiver::new(fs);
        let mut dec = Decoder::new(Fcs::Bits16).with_max_octets(256);
        let mut ones = 0usize;
        let mut last_bit_at = 0.0;
        println!("--- channel {channel}");
        // Envelope: 50 ms RMS, report changes above/below threshold.
        let win = (fs * 0.05) as usize;
        let mut on = false;
        let mut acc = 0.0f64;
        for (i, &s) in samples.iter().enumerate() {
            let x = f64::from(s);
            acc += x * x;
            if (i + 1) % win == 0 {
                let rms = (acc / win as f64).sqrt();
                acc = 0.0;
                let db = 20.0 * (rms + 1e-9).log10();
                let now_on = db > -45.0;
                if now_on != on {
                    println!("  {:7.2}s level {} ({db:.1} dB)", i as f64 / fs, if now_on { "up" } else { "down" });
                    on = now_on;
                }
            }
            if let Some(bit) = rx.feed(x) {
                last_bit_at = i as f64 / fs;
                ones = if bit { ones + 1 } else { 0 };
                if let Some(r) = dec.feed(bit) {
                    match r {
                        Ok(o) => println!("  {:7.2}s ok  {}", i as f64 / fs, o.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ")),
                        Err(e) => println!("  {:7.2}s ERR {e:?}", i as f64 / fs),
                    }
                }
            }
        }
        let _ = last_bit_at;
    }
}
