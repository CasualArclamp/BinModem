//! Read a capability field off the command line and say what it means.
//!
//! ```text
//! DIS=006ef800 cargo test -p fax --test read_dis -- --ignored --nocapture
//! ```
//!
//! Ignored, because it needs a field to read. The field itself comes off a
//! recording of a call: a fax machine sends its DIS every few seconds while
//! it waits, so any capture of one answering has several.

fn octets(hex: &str) -> Vec<u8> {
    hex.as_bytes()
        .chunks(2)
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
        .collect()
}

#[test]
#[ignore = "needs a DIS"]
fn what_it_can_do() {
    let hex = std::env::var("DIS").expect("set DIS to the hex of the field");
    let caps = fax::t30::capabilities(&octets(&hex));
    println!("\nDIS {hex}, {} octets\n", caps.octets);
    for (k, v) in caps.rows() {
        println!("  {k:<18} {v}");
    }
    if let Ok(csi) = std::env::var("CSI") {
        println!(
            "\n  {:<18} {}",
            "identification",
            fax::t30::identification(&octets(&csi))
        );
    }
}
