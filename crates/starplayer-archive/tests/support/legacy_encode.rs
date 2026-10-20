//! Test-only PKZIP streams; payload is the first 192 bytes of Scott McNab's REFLEX.S3M
//! (owner-authored fixture; see starplayer-s3m/tests/fixtures/README.md for provenance).
//! These prefixes test archive bytes, not module loading. No Hornet bytes are used.

pub fn payload() -> &'static [u8] {
    &include_bytes!("../../../starplayer-s3m/tests/fixtures/REFLEX.S3M")[..192]
}

#[derive(Default)]
pub struct Bits {
    bytes: Vec<u8>,
    position: usize,
}

impl Bits {
    pub fn write(&mut self, value: u16, count: usize) {
        for bit in 0..count {
            if self.position.is_multiple_of(8) { self.bytes.push(0); }
            let last = self.bytes.len() - 1;
            self.bytes[last] |= (((value >> bit) & 1) as u8) << (self.position % 8);
            self.position += 1;
        }
    }

    fn symbol(&mut self, value: u16, count: usize) {
        self.write((!value).reverse_bits() >> (16 - count), count);
    }

    pub fn finish(self) -> Vec<u8> { self.bytes }
}

pub fn shrink(input: &[u8]) -> Vec<u8> {
    // Literal-only LZW: stay below the initial 9-bit dictionary's capacity.
    assert!(input.len() <= 256);
    let mut bits = Bits::default();
    for byte in input { bits.write(*byte as u16, 9); }
    bits.finish()
}

/// Fill the dictionary with literal pairs, then assign and use slot 8191 via KwKwK
/// (the previous literal extended by itself). Optionally rebuild the free-code queue
/// first, covering the second allocation path fixed by zip2 PR #1029.
pub fn shrink_max_code(partial_clear: bool) -> (Vec<u8>, Vec<u8>) {
    let mut bits = Bits::default();
    let mut input = vec![payload()[0]];
    let mut code_size = 9;
    bits.write(input[0] as u16, code_size);
    if partial_clear {
        bits.write(256, code_size);
        bits.write(2, code_size);
    }
    for next_code in 257..=8190 {
        if next_code == 1 << code_size {
            bits.write(256, code_size);
            bits.write(1, code_size); // increase code width
            code_size += 1;
        }
        let literal = payload()[(next_code - 257) % payload().len()];
        bits.write(literal as u16, code_size);
        input.push(literal);
    }
    bits.write(8191, code_size);
    let last_literal = input[input.len() - 1];
    input.extend_from_slice(&[last_literal; 2]);
    (input, bits.finish())
}

pub fn implode(input: &[u8], flags: u16) -> Vec<u8> {
    let mut bits = Bits::default();
    if flags & 4 != 0 {
        // 256 symbols, all eight bits: sixteen runs of sixteen lengths.
        bits.write(15, 8);
        for _ in 0..16 { bits.write(0xF7, 8); }
    }
    // Length and distance: 64 symbols, all six bits, four runs each.
    for _ in 0..2 {
        bits.write(3, 8);
        for _ in 0..4 { bits.write(0xF5, 8); }
    }
    let minimum_length = if flags & 4 != 0 { 3 } else { 2 };
    let mut position = 0;
    while position < input.len() {
        // Exercise length/distance matches as well as literals, including overlap.
        let can_match = position > 0 && position + minimum_length <= input.len()
            && input[position..position + minimum_length].iter().all(|byte| *byte == input[position - 1]);
        if can_match {
            bits.write(0, 1);
            bits.write(0, if flags & 2 != 0 { 7 } else { 6 }); // distance 1
            bits.symbol(0, 6);
            bits.symbol(0, 6); // minimum length
            position += minimum_length;
        } else {
            bits.write(1, 1);
            if flags & 4 != 0 { bits.symbol(input[position] as u16, 8); }
            else { bits.write(input[position] as u16, 8); }
            position += 1;
        }
    }
    bits.finish()
}

pub fn reduce(input: &[u8]) -> Vec<u8> {
    let mut bits = Bits::default();
    // All 256 follower sets empty: bytes pass through literally at every factor.
    for _ in 0..256 { bits.write(0, 6); }
    for byte in input {
        bits.write(*byte as u16, 8);
        if *byte == 0x90 { bits.write(0, 8); } // escaped DLE
    }
    bits.finish()
}

fn crc32(input: &[u8]) -> u32 {
    let mut checksum = !0u32;
    for byte in input {
        checksum ^= *byte as u32;
        for _ in 0..8 { checksum = (checksum >> 1) ^ (0xEDB88320 & 0u32.wrapping_sub(checksum & 1)); }
    }
    !checksum
}

type Entry<'a> = (&'a str, &'a [u8], u16, u16, &'a [u8]);

/// A small ZIP writer accepting already compressed streams (including invalid ones).
pub fn archive(entries: &[Entry<'_>]) -> Vec<u8> {
    let mut output = Vec::new();
    let mut directory = Vec::new();
    for (name, input, method, flags, compressed) in entries {
        let offset = output.len() as u32;
        let checksum = crc32(input);
        output.extend_from_slice(b"PK\x03\x04");
        word(&mut output, 10);
        word(&mut output, *flags);
        word(&mut output, *method);
        output.extend_from_slice(&[0; 4]); // time/date
        dword(&mut output, checksum);
        dword(&mut output, compressed.len() as u32);
        dword(&mut output, input.len() as u32);
        word(&mut output, name.len() as u16);
        word(&mut output, 0);
        output.extend_from_slice(name.as_bytes());
        output.extend_from_slice(compressed);
        directory.extend_from_slice(b"PK\x01\x02");
        word(&mut directory, 10); // made by
        word(&mut directory, 10); // needed
        word(&mut directory, *flags);
        word(&mut directory, *method);
        directory.extend_from_slice(&[0; 4]);
        dword(&mut directory, checksum);
        dword(&mut directory, compressed.len() as u32);
        dword(&mut directory, input.len() as u32);
        word(&mut directory, name.len() as u16);
        directory.extend_from_slice(&[0; 12]); // extra, comment, disk, attributes
        dword(&mut directory, offset);
        directory.extend_from_slice(name.as_bytes());
    }
    let directory_offset = output.len() as u32;
    output.extend_from_slice(&directory);
    output.extend_from_slice(b"PK\x05\x06");
    output.extend_from_slice(&[0; 4]);
    word(&mut output, entries.len() as u16);
    word(&mut output, entries.len() as u16);
    dword(&mut output, directory.len() as u32);
    dword(&mut output, directory_offset);
    word(&mut output, 0);
    output
}

fn word(output: &mut Vec<u8>, value: u16) { output.extend_from_slice(&value.to_le_bytes()); }
fn dword(output: &mut Vec<u8>, value: u32) { output.extend_from_slice(&value.to_le_bytes()); }
