//! A captured V.8 bis exchange, replayed into the calling station.
//!
//! Ignored, because captures are not in the repository:
//!
//! V8BIS_CAPTURE=dist/captures/live-1795256003.wav V8BIS_CHANNEL=0 \
//!     cargo test -p datapump --test v8bis_capture -- --ignored --nocapture

use datapump::v8bis::{OnCr, OnMr, Station};
use v8::bis::{Identification, Kind, Message, Data};

#[test]
#[ignore]
fn a_captured_v8bis_exchange() {
    let path = std::env::var("V8BIS_CAPTURE").expect("V8BIS_CAPTURE");
    let channel: usize = std::env::var("V8BIS_CHANNEL").map_or(0, |c| c.parse().unwrap());
    let wav = line::wav::read(&path).unwrap();
    let fs = f64::from(wav.sample_rate);
    let ours = Message::data(
        Kind::Cl,
        Identification { v8: true, ack1: true, ..Identification::default() },
        Data { v42: true, v34: true, v32bis: true, v22bis: true, v90_analogue: true, v92_analogue: true, ..Data::default() },
    );
    let mut station = Station::calling(ours, fs).replying(OnMr::Ms, OnCr::Crd);
    for (i, &s) in wav.channel(channel).iter().take((8.0 * fs) as usize).enumerate() {
        station.step(f64::from(s));
        for note in station.take_notes() {
            println!("{:7.3}  {note}", i as f64 / fs);
        }
    }
    println!("status {:?}", station.status());
}
