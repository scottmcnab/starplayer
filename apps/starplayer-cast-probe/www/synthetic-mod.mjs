// The probe's default bench module: a four-channel M.K. (ProTracker) MOD built byte by
// byte, with no fixture file on disk. Ported from `syntheticMod()` in
// `apps/starplayer-web/test/worklet-harness.mjs` — kept byte-for-byte identical to that
// function so a wire capture from either place means the same thing — and commented here
// because this copy is the one a stranger reading the cast probe will actually meet.
//
// `crates/starplayer-s3m/tests/fixtures/` is licensed for testing only and must never
// reach a published page (A4-N1's own rule); this module exists precisely so the probe
// never needs it, or any other file on disk, to have something to bench and transfer.

/**
 * Build the synthetic MOD as a fresh `Uint8Array` (mutable; callers own the copy).
 *
 * Layout (the standard 31-sample "M.K." MOD header, ProTracker's own dialect):
 * - `0..20`      song title, 20 bytes, zero-padded — `"headphone test"` here.
 * - `20..950`    31 sample descriptors, 30 bytes each. Only sample 1's fields (the
 *                first descriptor, at offset 20) are non-default:
 *   - `42..44`   sample length in **words** (big-endian) — `sampleFrames / 2`, since a
 *                word is 2 bytes; this MOD's sample is `sampleFrames` bytes long.
 *   - `45`       volume, 0..64 — set to 64 (maximum).
 * - `950`        song length: how many entries of the 128-byte order table are played.
 *                Set to 1, so only `order[0]` (already 0 from the zero-filled array)
 *                is used — the song is one pattern long.
 * - `951`        restart position — left at 0, unused since the song does not loop here.
 * - `952..1080`  the order table, 128 bytes; only `order[0] = 0` (pattern 0) matters.
 * - `1080..1084` the format tag: `"M.K."` marks a standard 4-channel ProTracker module.
 * - `1084..`     pattern data, 64 rows × 4 channels × 4 bytes = 1024 bytes for pattern 0,
 *                then the sample's own PCM.
 *
 * Row 0 of pattern 0 puts a note in all four channels: each channel's packed cell is
 * `01 AC 10 00` — period `0x1AC` (428, the Amiga period for a middle "C-3", ProTracker's
 * own tuning reference), sample number 1, no effect. That is the only note in the module;
 * the rest of the pattern is silence.
 *
 * The sample itself, appended after the pattern, is a `sampleFrames`-byte square wave:
 * alternating `0x7F`/`0x80` (approximately +127/-128 as signed 8-bit PCM), which is cheap
 * to generate, has no silence for a loudness check to trip over, and needs no compression
 * or licensing thought at all.
 */
export function syntheticMod() {
    const headerBytes = 1084;
    const patternBytes = 64 * 4 * 4;
    const sampleFrames = 256;
    const bytes = new Uint8Array(headerBytes + patternBytes + sampleFrames);
    bytes.set(new TextEncoder().encode('headphone test'), 0);
    bytes.set([(sampleFrames / 2) >> 8, (sampleFrames / 2) & 0xFF], 42);
    bytes[45] = 64;
    bytes[950] = 1;
    bytes.set(new TextEncoder().encode('M.K.'), 1080);
    for (let channel = 0; channel < 4; channel += 1) bytes.set([0x01, 0xAC, 0x10, 0x00], headerBytes + channel * 4);
    for (let index = headerBytes + patternBytes; index < bytes.length; index += 1) {
        bytes[index] = index & 1 ? 0x80 : 0x7F;
    }
    return bytes;
}
