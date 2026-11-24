//! Code page 437, the IBM PC character set.
//!
//! Every BBS of the era drew its screens with this, so the box-drawing and
//! block characters in `0xB0`-`0xDF` are not decoration: without them ANSI art
//! renders as mojibake. Bytes `0x20`-`0x7E` are plain ASCII; the table below
//! covers the high half plus `0x7F`.

/// CP437 code points for bytes `0x80`-`0xFF`.
const HIGH: [char; 128] = [
    // 0x80
    'Ç', 'ü', 'é', 'â', 'ä', 'à', 'å', 'ç', 'ê', 'ë', 'è', 'ï', 'î', 'ì', 'Ä', 'Å',
    // 0x90
    'É', 'æ', 'Æ', 'ô', 'ö', 'ò', 'û', 'ù', 'ÿ', 'Ö', 'Ü', '¢', '£', '¥', '₧', 'ƒ',
    // 0xA0
    'á', 'í', 'ó', 'ú', 'ñ', 'Ñ', 'ª', 'º', '¿', '⌐', '¬', '½', '¼', '¡', '«', '»',
    // 0xB0 — shading and single/double box drawing begin here
    '░', '▒', '▓', '│', '┤', '╡', '╢', '╖', '╕', '╣', '║', '╗', '╝', '╜', '╛', '┐',
    // 0xC0
    '└', '┴', '┬', '├', '─', '┼', '╞', '╟', '╚', '╔', '╩', '╦', '╠', '═', '╬', '╧',
    // 0xD0
    '╨', '╤', '╥', '╙', '╘', '╒', '╓', '╫', '╪', '┘', '┌', '█', '▄', '▌', '▐', '▀',
    // 0xE0
    'α', 'ß', 'Γ', 'π', 'Σ', 'σ', 'µ', 'τ', 'Φ', 'Θ', 'Ω', 'δ', '∞', 'φ', 'ε', '∩',
    // 0xF0
    '≡', '±', '≥', '≤', '⌠', '⌡', '÷', '≈', '°', '∙', '·', '√', 'ⁿ', '²', '■', '\u{a0}',
];

/// Glyphs for `0x00`-`0x1F`, used only when a byte reaches the screen as a
/// printable rather than being consumed as a control code.
const LOW: [char; 32] = [
    ' ', '☺', '☻', '♥', '♦', '♣', '♠', '•', '◘', '○', '◙', '♂', '♀', '♪', '♫', '☼',
    '►', '◄', '↕', '‼', '¶', '§', '▬', '↨', '↑', '↓', '→', '←', '∟', '↔', '▲', '▼',
];

/// Translate one CP437 byte to Unicode.
pub fn decode(byte: u8) -> char {
    match byte {
        0x00..=0x1f => LOW[byte as usize],
        0x7f => '⌂',
        0x80..=0xff => HIGH[(byte - 0x80) as usize],
        _ => byte as char,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_passes_through() {
        assert_eq!(decode(b'A'), 'A');
        assert_eq!(decode(b' '), ' ');
        assert_eq!(decode(b'~'), '~');
    }

    #[test]
    fn box_drawing_is_correct() {
        // The characters BBS art is actually made of.
        assert_eq!(decode(0xB3), '│');
        assert_eq!(decode(0xC4), '─');
        assert_eq!(decode(0xDA), '┌');
        assert_eq!(decode(0xBF), '┐');
        assert_eq!(decode(0xC0), '└');
        assert_eq!(decode(0xD9), '┘');
        assert_eq!(decode(0xC5), '┼');
    }

    #[test]
    fn double_line_box_drawing_is_correct() {
        assert_eq!(decode(0xC9), '╔');
        assert_eq!(decode(0xBB), '╗');
        assert_eq!(decode(0xC8), '╚');
        assert_eq!(decode(0xBC), '╝');
        assert_eq!(decode(0xCD), '═');
        assert_eq!(decode(0xBA), '║');
    }

    #[test]
    fn shading_and_blocks_are_correct() {
        assert_eq!(decode(0xB0), '░');
        assert_eq!(decode(0xB1), '▒');
        assert_eq!(decode(0xB2), '▓');
        assert_eq!(decode(0xDB), '█');
        assert_eq!(decode(0xDC), '▄');
        assert_eq!(decode(0xDF), '▀');
    }

    #[test]
    fn the_table_covers_every_byte() {
        for b in 0..=255u8 {
            let c = decode(b);
            assert!(c != '\0' || b == 0, "byte {b:#04x} decoded to NUL");
        }
    }

    #[test]
    fn high_half_endpoints_are_right() {
        assert_eq!(decode(0x80), 'Ç');
        assert_eq!(decode(0xFF), '\u{a0}'); // non-breaking space
        assert_eq!(decode(0xFE), '■');
        assert_eq!(decode(0x9F), 'ƒ');
    }
}
