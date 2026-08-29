//! Text fields as a 1990s tracker wrote them: **code page 437**, the IBM PC's OEM
//! character set, one byte per character.
//!
//! A DOS tracker's title and sample names are not UTF-8 and are not Latin-1 either.
//! Scream Tracker 3 ran in text mode on code page 437, so the byte `0xFF` it writes into a
//! sample name is the *non-breaking space* it displayed as one — ST3 names a headerless
//! raw sample `<file> (no\xFFheader)` — and `0xB3` is `│`, not `³`. Decoding as Latin-1
//! turns those into `ÿ` and `³`, which is what the first web player showed.
//!
//! The mapping never fails: every byte has a glyph. Bytes `0x01..=0x1F` map to the glyphs
//! CP437 draws for them (`☺`, `♪`, `►` …), which trackers did use in names; `0x00` ends the
//! field.

use alloc::string::String;

/// The glyph for each byte `0x01..=0x1F`, then `0x80..=0xFF`. `0x20..=0x7F` is ASCII.
const CP437_LOW: [char; 31] = [
    '\u{263a}', '\u{263b}', '\u{2665}', '\u{2666}', '\u{2663}', '\u{2660}', '\u{2022}', '\u{25d8}', '\u{25cb}', '\u{25d9}', '\u{2642}', '\u{2640}', '\u{266a}', '\u{266b}', '\u{263c}', '\u{25ba}',
    // 0x11
    '\u{25c4}', '\u{2195}', '\u{203c}', '\u{00b6}', '\u{00a7}', '\u{25ac}', '\u{21a8}', '\u{2191}', '\u{2193}', '\u{2192}', '\u{2190}', '\u{221f}', '\u{2194}', '\u{25b2}', '\u{25bc}',
];

const CP437_HIGH: [char; 128] = [
    // 0x80
    '\u{00c7}', '\u{00fc}', '\u{00e9}', '\u{00e2}', '\u{00e4}', '\u{00e0}', '\u{00e5}', '\u{00e7}', '\u{00ea}', '\u{00eb}', '\u{00e8}', '\u{00ef}', '\u{00ee}', '\u{00ec}', '\u{00c4}', '\u{00c5}',
    // 0x90
    '\u{00c9}', '\u{00e6}', '\u{00c6}', '\u{00f4}', '\u{00f6}', '\u{00f2}', '\u{00fb}', '\u{00f9}', '\u{00ff}', '\u{00d6}', '\u{00dc}', '\u{00a2}', '\u{00a3}', '\u{00a5}', '\u{20a7}', '\u{0192}',
    // 0xA0
    '\u{00e1}', '\u{00ed}', '\u{00f3}', '\u{00fa}', '\u{00f1}', '\u{00d1}', '\u{00aa}', '\u{00ba}', '\u{00bf}', '\u{2310}', '\u{00ac}', '\u{00bd}', '\u{00bc}', '\u{00a1}', '\u{00ab}', '\u{00bb}',
    // 0xB0
    '\u{2591}', '\u{2592}', '\u{2593}', '\u{2502}', '\u{2524}', '\u{2561}', '\u{2562}', '\u{2556}', '\u{2555}', '\u{2563}', '\u{2551}', '\u{2557}', '\u{255d}', '\u{255c}', '\u{255b}', '\u{2510}',
    // 0xC0
    '\u{2514}', '\u{2534}', '\u{252c}', '\u{251c}', '\u{2500}', '\u{253c}', '\u{255e}', '\u{255f}', '\u{255a}', '\u{2554}', '\u{2569}', '\u{2566}', '\u{2560}', '\u{2550}', '\u{256c}', '\u{2567}',
    // 0xD0
    '\u{2568}', '\u{2564}', '\u{2565}', '\u{2559}', '\u{2558}', '\u{2552}', '\u{2553}', '\u{256b}', '\u{256a}', '\u{2518}', '\u{250c}', '\u{2588}', '\u{2584}', '\u{258c}', '\u{2590}', '\u{2580}',
    // 0xE0
    '\u{03b1}', '\u{00df}', '\u{0393}', '\u{03c0}', '\u{03a3}', '\u{03c3}', '\u{00b5}', '\u{03c4}', '\u{03a6}', '\u{0398}', '\u{03a9}', '\u{03b4}', '\u{221e}', '\u{03c6}', '\u{03b5}', '\u{2229}',
    // 0xF0
    '\u{2261}', '\u{00b1}', '\u{2265}', '\u{2264}', '\u{2320}', '\u{2321}', '\u{00f7}', '\u{2248}', '\u{00b0}', '\u{2219}', '\u{00b7}', '\u{221a}', '\u{207f}', '\u{00b2}', '\u{25a0}', '\u{00a0}',
];

/// The character code page 437 shows for `byte`.
pub fn cp437_char(byte: u8) -> char {
    match byte {
        0x00 => '\0',
        0x01..=0x1F => CP437_LOW.get((byte - 1) as usize).copied().unwrap_or(' '),
        0x20..=0x7F => byte as char,
        _ => CP437_HIGH.get((byte - 0x80) as usize).copied().unwrap_or(' '),
    }
}

/// Decode a fixed-width text field: everything before the first NUL, as code page 437,
/// with trailing blanks — including CP437's `0xFF` non-breaking space — removed.
///
/// Leading blanks are kept: centred captions are a thing in 1994 sample names.
pub fn decode_cp437(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|byte| *byte == 0).unwrap_or(bytes.len());
    let field = bytes.get(..end).unwrap_or(bytes);
    let mut text: String = field.iter().map(|byte| cp437_char(*byte)).collect();
    while text.ends_with([' ', '\t', '\r', '\n', '\u{a0}']) {
        text.pop();
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_passes_through_and_the_field_ends_at_the_first_nul() {
        assert_eq!(decode_cp437(b"Reflex\0\0\0"), "Reflex");
        assert_eq!(decode_cp437(b"Reflex   "), "Reflex");
        assert_eq!(decode_cp437(b"  centred  "), "  centred");
        assert_eq!(decode_cp437(b""), "");
    }

    #[test]
    fn scream_tracker_s_no_header_caption_uses_the_cp437_non_breaking_space() {
        assert_eq!(decode_cp437(b"bassguit.pcm (no\xffheader)\0\0\0\0"), "bassguit.pcm (no\u{a0}header)");
        assert_eq!(decode_cp437(b"name\xff\xff"), "name", "trailing non-breaking spaces are blanks");
    }

    #[test]
    fn high_bytes_are_cp437_glyphs_not_latin_one() {
        assert_eq!(cp437_char(0xB3), '│');
        assert_eq!(cp437_char(0x82), 'é');
        assert_eq!(cp437_char(0xFF), '\u{a0}');
        assert_eq!(cp437_char(0x0E), '♫');
        assert_eq!(cp437_char(b'A'), 'A');
    }
}
