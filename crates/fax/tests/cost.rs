#[test]
#[ignore = "a measurement"]
fn how_long_does_coding_a_page_take() {
    for resolution in [fax::page::Resolution::Standard, fax::page::Resolution::Fine] {
        let width = fax::page::WIDTH;
        let page = fax::page::Page {
            lines: (0..resolution.lines())
                .map(|y| (0..width).map(|x| (x / 9 + y / 3) % 5 < 2).collect())
                .collect(),
            resolution,
        };
        let at = std::time::Instant::now();
        let bits = page.encode().len();
        eprintln!(
            "{}: {bits} bits in {:.1} ms",
            resolution.name(),
            at.elapsed().as_secs_f64() * 1000.0
        );
    }
}
