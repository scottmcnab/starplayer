//! The short-time Fourier transform [`BandwidthExtender`](crate::BandwidthExtender) works
//! in: a committed Hann window, committed twiddle factors, and a radix-2 transform that
//! reads both as data.
//!
//! # Generated data, committed as source
//!
//! Exactly [`crate::polyphase`]'s pattern, for exactly its reason. A twiddle factor is
//! `cos`/`sin` of a rational angle and a Hann weight is `cos` of one, and libm
//! implementations disagree in the last bit — so a table built at run time would make an
//! enhanced module hash differently on x86, ARM and WASM. Both tables below are the output
//! of [`tests::generate`], which runs only under `cargo test`, and
//! [`tests::the_committed_tables_are_what_the_generator_produces`] regenerates them in
//! `f64` and asserts every value bit for bit. Editing either by hand fails that test.
//!
//! # Why 1 024 points and a hop of 256
//!
//! The transform's job is to find where a sample's content stops and to transpose the
//! octave below that point upwards, so it has to resolve *harmonics* rather than merely
//! detect energy.
//!
//! At the rate the extender actually sees — a tracker sample at 8 363 Hz that a 4x
//! upsample has already taken to 33 452 Hz — 1 024 points is 30.6 ms and 32.7 Hz per bin.
//! That resolves the partials of anything down to a low bass note, and 30 ms is short
//! enough that a drum's 80 ms decay is still three frames rather than one. Halving it to
//! 512 would put two partials of a 60 Hz-spaced harmonic series in one bin, which turns
//! the band-edge search into an energy detector; doubling it to 2 048 would smear a
//! percussive attack across 60 ms of output.
//!
//! The hop is a quarter of the transform. A Hann window at 75 % overlap is far past the
//! overlap-add criterion, so the modified frames sum back together without the
//! amplitude ripple a half-overlap would leave, and the analysis and synthesis windows can
//! **both** be applied — which is what keeps a patched frame from clicking against its
//! neighbours. See [`overlap_add_normalisation`] for why the sum of the squared window is
//! accumulated rather than assumed.

/// Points per analysis frame.
pub const STFT_SIZE: usize = 1_024;

/// Frames advance by this many samples: a quarter of [`STFT_SIZE`].
pub const STFT_HOP: usize = STFT_SIZE / 4;

/// Bins the non-negative half of the spectrum holds, `STFT_SIZE / 2 + 1`.
pub const STFT_BINS: usize = STFT_SIZE / 2 + 1;

/// One Hann weight, decoded from its committed bit pattern. Out-of-range answers `0.0`.
#[inline]
pub fn window(index: usize) -> f64 {
    match HANN_WINDOW_BITS.get(index) {
        Some(bits) => f64::from_bits(*bits),
        None => 0.0,
    }
}

/// One twiddle factor `e^(-2πi·index/STFT_SIZE)`, decoded from its committed bit patterns.
#[inline]
pub fn twiddle(index: usize) -> (f64, f64) {
    match (TWIDDLE_REAL_BITS.get(index), TWIDDLE_IMAGINARY_BITS.get(index)) {
        (Some(real), Some(imaginary)) => (f64::from_bits(*real), f64::from_bits(*imaginary)),
        _ => (0.0, 0.0),
    }
}

/// In-place radix-2 decimation-in-time transform of [`STFT_SIZE`] points.
///
/// Only `+`, `−` and `×` on committed constants, so the result is the same sequence of
/// IEEE operations on every target.
pub fn forward(real: &mut [f64; STFT_SIZE], imaginary: &mut [f64; STFT_SIZE]) {
    let mut destination = 0usize;
    for source in 1..STFT_SIZE {
        let mut bit = STFT_SIZE >> 1;
        while destination & bit != 0 {
            destination ^= bit;
            bit >>= 1;
        }
        destination |= bit;
        if source < destination {
            real.swap(source, destination);
            imaginary.swap(source, destination);
        }
    }

    let mut span = 2usize;
    while span <= STFT_SIZE {
        let half = span / 2;
        let stride = STFT_SIZE / span;
        let mut start = 0usize;
        while start < STFT_SIZE {
            for offset in 0..half {
                let (cosine, sine) = twiddle(offset * stride);
                let even = start + offset;
                let odd = even + half;
                let product_real = real[odd] * cosine - imaginary[odd] * sine;
                let product_imaginary = real[odd] * sine + imaginary[odd] * cosine;
                real[odd] = real[even] - product_real;
                imaginary[odd] = imaginary[even] - product_imaginary;
                real[even] += product_real;
                imaginary[even] += product_imaginary;
            }
            start += span;
        }
        span <<= 1;
    }
}

/// The inverse transform, by conjugation: `IDFT(X) = conj(DFT(conj(X))) / N`.
///
/// One transform and two sign flips rather than a second twiddle table.
pub fn inverse(real: &mut [f64; STFT_SIZE], imaginary: &mut [f64; STFT_SIZE]) {
    for value in imaginary.iter_mut() {
        *value = -*value;
    }
    forward(real, imaginary);
    let scale = 1.0 / STFT_SIZE as f64;
    for value in real.iter_mut() {
        *value *= scale;
    }
    for value in imaginary.iter_mut() {
        *value = -*value * scale;
    }
}

/// Why the overlap-add divides by an accumulated weight rather than by a constant.
///
/// With the window applied on analysis **and** on synthesis, a quarter hop sums the
/// squared window to `4 × mean(w²) = 1.5` everywhere the frames fully overlap — so a
/// constant `1 / 1.5` would reconstruct an unmodified signal exactly in the middle of a
/// one-shot and wrongly at both of its ends.
///
/// It would be wrong in a second, worse way for a **looped** sample. The extender folds
/// the analysis frames that run past `loop_end` back into the loop so that the result is
/// exactly periodic, and the fold does not in general land on the hop grid — the loop's
/// length need not be a multiple of 256. Accumulating the squared window alongside the
/// signal and dividing pointwise makes the reconstruction exact whatever the fold does,
/// because a periodic signal reads the same at every folded position: the numerator picks
/// up `x[n] · Σw²` and the denominator picks up `Σw²`. That identity is what
/// `tests::a_looped_sample_that_is_not_patched_comes_back_unchanged` in
/// [`crate::sbr`] checks.
pub const fn overlap_add_normalisation() {}

/// Periodic Hann weights, as `f64` bit patterns. Generated — see the module
/// documentation. Do not edit by hand.
pub const HANN_WINDOW_BITS: [u64; STFT_SIZE] = [
    0x0000000000000000, 0x3ee3bd38bab70000, 0x3f03bd2c8da4a000, 0x3f1634bb4ae5f000,
    0x3f23bcfbd9979800, 0x3f2ed71071604000, 0x3f36344004228c00, 0x3f3e389491f88400,
    0x3f43bc390d250400, 0x3f48f9e0e5514800, 0x3f4ed534e31ca600, 0x3f52a713498c3c00,
    0x3f563252fe77c600, 0x3f5a0c50d1c6c000, 0x3f5e350342a4f700, 0x3f6156300705d500,
    0x3f63b92e176d6d00, 0x3f664375eefa3e00, 0x3f68f501492cc280, 0x3f6bcdc980a46f80,
    0x3f6ecdc78f301680, 0x3f70fa7a06ef9e80, 0x3f72a1a39a8a2fc0, 0x3f745c5c6e4c1140,
    0x3f762aa03dd6ba40, 0x3f780c6a94934e00, 0x3f7a01b6cdbd9940, 0x3f7c0a80146f8940,
    0x3f7e26c163ad15c0, 0x3f802b3ac3385240, 0x3f814ccb8bdbf120, 0x3f82781041488920,
    0x3f83ad0601146a00, 0x3f84eba9d0ec2f80, 0x3f8633f89e9a1a60, 0x3f8785ef400da460,
    0x3f88e18a73634ee0, 0x3f8a46c6deecac60, 0x3f8bb5a11138a4c0, 0x3f8d2e15811bf460,
    0x3f8eb0208db9e520, 0x3f901ddf3f46a130, 0x3f90e875c1b8c3e0, 0x3f91b7d1da562440,
    0x3f928bf1897b69d0, 0x3f9364d2c3c407c0, 0x3f944273720f48c0, 0x3f9524d171857720,
    0x3f960bea939d2260, 0x3f96f7bc9e2080d0, 0x3f97e8454b32ef30, 0x3f98dd8249568bc0,
    0x3f99d7713b71eee0, 0x3f9ad60fb8d60040, 0x3f9bd95b4d43e820, 0x3f9ce15178f31db0,
    0x3f9dedefb09791b0, 0x3f9eff335d67f540, 0x3fa00a8cee920ea8, 0x3fa097d0410dc130,
    0x3fa127624999ee18, 0x3fa1b941a5f7ed00, 0x3fa24d6cee3afb28, 0x3fa2e3e2b4cbb3c0,
    0x3fa37ca1866b95d0, 0x3fa417a7ea389830, 0x3fa4b4f461b0cba8, 0x3fa5548568b60a78,
    0x3fa5f6597591b630, 0x3fa69a6ef8f88308, 0x3fa740c45e0e5120, 0x3fa7e9580a6a1370,
    0x3fa894285e19c468, 0x3fa94133b3a66850, 0x3fa9f07860181d20, 0x3faaa1f4b2fa37f8,
    0x3fab55a6f65f7058, 0x3fac0b8d6ee61868, 0x3facc3a65bbc6328, 0x3fad7deff6a4b7c8,
    0x3fae3a6873fa1278, 0x3faef90e02b47280, 0x3fafb9decc6d55b8, 0x3fb03e6c7ab2208c,
    0x3fb0a0fd4e41ab58, 0x3fb104a0edb1fc58, 0x3fb169566329bcb4, 0x3fb1cf1cb62bec5c,
    0x3fb235f2eb9a4708, 0x3fb29dd805b7afec, 0x3fb306cb042aa3b8, 0x3fb370cae3ffb130,
    0x3fb3dbd69fabf800, 0x3fb447ed2f0fae30, 0x3fb4b50d8778abbc, 0x3fb523369ba4fcb0,
    0x3fb592675bc57974, 0x3fb6029eb580658c, 0x3fb673db93f41478, 0x3fb6e61cdfb994e0,
    0x3fb759617ee761f8, 0x3fb7cda855141b24, 0x3fb842f0435941b0, 0x3fb8b9382855fca8,
    0x3fb9307ee031e2fc, 0x3fb9a8c3449fcb78, 0x3fba22042ce0a2f8, 0x3fba9c406dc648a4,
    0x3fbb1776d9b67010, 0x3fbb93a640ad8978, 0x3fbc10cd7041afcc, 0x3fbc8eeb33a59cd0,
    0x3fbd0dfe53aba2fc, 0x3fbd8e0596c8ad54, 0x3fbe0effc1174504, 0x3fbe90eb945a9cd0,
    0x3fbf13c7d001a248, 0x3fbf9793312a14d0, 0x3fc00e263951d11e, 0x3fc050f92679849a,
    0x3fc09441bb2aa0a2, 0x3fc0d7ff51615436, 0x3fc11c3141f91b3e, 0x3fc160d6e4ae5ae6,
    0x3fc1a5ef902000d2, 0x3fc1eb7a99d1250c, 0x3fc23177562aaea4, 0x3fc277e5187cfb16,
    0x3fc2bec333018866, 0x3fc30610f6dca1e0, 0x3fc34dcdb41f0f84, 0x3fc395f8b9c7c82e,
    0x3fc3de9155c5a642, 0x3fc42796d4f91f18, 0x3fc471088335fce6, 0x3fc4bae5ab451b5e,
    0x3fc5052d96e626c2, 0x3fc54fdf8ed15d92, 0x3fc59afadab954d4, 0x3fc5e67ec14cbeca,
    0x3fc6326a8838342e, 0x3fc67ebd7427fff6, 0x3fc6cb76c8c9ed8e, 0x3fc71895c8cf1972,
    0x3fc76619b5edc454, 0x3fc7b401d0e3289c, 0x3fc8024d59755256, 0x3fc850fb8e74f978,
    0x3fc8a00badbf5e8e, 0x3fc8ef7cf44029bc, 0x3fc93f4e9df34c1a, 0x3fc98f7fe5e6e352,
    0x3fc9e010063d1f94, 0x3fca30fe382e2bd2, 0x3fca8249b40a182c, 0x3fcad3f1b13ac6b0,
    0x3fcb25f56645da42, 0x3fcb785408cea7c0, 0x3fcbcb0ccd98294e, 0x3fcc1e1ee886f3ca,
    0x3fcc71898ca32e6e, 0x3fccc54bec1a8c90, 0x3fcd19653842496e, 0x3fcd6dd4a1992628,
    0x3fcdc29957c969ba, 0x3fce17b289aae302, 0x3fce6d1f6544ece4, 0x3fcec2df17d07440,
    0x3fcf18f0cdba0026, 0x3fcf6f53b2a3bbcc, 0x3fcfc606f1678292, 0x3fd00e84da0c76f8,
    0x3fd03a2d9203b2a2, 0x3fd065fd34e017cc, 0x3fd091f356884394, 0x3fd0be0f8a83d775,
    0x3fd0ea5163fc84e1, 0x3fd116b875bf19d4, 0x3fd14344523c8e3c, 0x3fd16ff48b8b1252,
    0x3fd19cc8b3671dd1, 0x3fd1c9c05b347ffa, 0x3fd1f6db13ff708c, 0x3fd224186e7da17f,
    0x3fd25177fb0f519f, 0x3fd27ef949c05ffa, 0x3fd2ac9bea49601a, 0x3fd2da5f6c10af0e,
    0x3fd308435e2b893b, 0x3fd336474f5f2101, 0x3fd3646ace21b614, 0x3fd392ad689bada8,
    0x3fd3c10eaca8ab4e, 0x3fd3ef8e27d8aa9e, 0x3fd41e2b6771198c, 0x3fd44ce5f86df384,
    0x3fd47bbd6782dd30, 0x3fd4aab1411c40f4, 0x3fd4d9c111606c1e, 0x3fd508ec6430acb6,
    0x3fd53832c52a7012, 0x3fd56793bfa861ee, 0x3fd5970edec38c4a, 0x3fd5c6a3ad5477d6,
    0x3fd5f651b5f44cfe, 0x3fd6261882fdf5a4, 0x3fd655f79e8f3f56, 0x3fd685ee9289fe3c,
    0x3fd6b5fce895307e, 0x3fd6e6222a1e2248, 0x3fd7165de0599262, 0x3fd746af9444d744,
    0x3fd77716cea704c4, 0x3fd7a79318121238, 0x3fd7d823f8e40128, 0x3fd808c8f9480481,
    0x3fd83981a137a838, 0x3fd86a4d787bf978, 0x3fd89b2c06aeaf3a, 0x3fd8cc1cd33b535f,
    0x3fd8fd1f65606c28, 0x3fd92e334430a638, 0x3fd95f57f693feec, 0x3fd9908d0348ef26,
    0x3fd9c1d1f0e5967d, 0x3fd9f32645d8e6ce, 0x3fda2489886bd033, 0x3fda55fb3ec26d52,
    0x3fda877aeedd300d, 0x3fdab9081e9a0e82, 0x3fdaeaa253b5b06a, 0x3fdb1c4913cc9cbb,
    0x3fdb4dfbe45c67b2, 0x3fdb7fba4ac4e10c, 0x3fdbb183cc4942a2, 0x3fdbe357ee115f38,
    0x3fdc1536352ad19e, 0x3fdc471e268a2c06, 0x3fdc790f470c27a4, 0x3fdcab091b76d47e,
    0x3fdcdd0b287ac979, 0x3fdd0f14f2b4549c, 0x3fdd4125feacab7d, 0x3fdd733dd0db1bed,
    0x3fdda55beda63cc1, 0x3fddd77fd9651ec9, 0x3fde09a918607df1, 0x3fde3bd72ed3f281,
    0x3fde6e09a0ef227e, 0x3fdea03ff2d6f32b, 0x3fded279a8a6baa2, 0x3fdf04b646717188,
    0x3fdf36f55042e4cc, 0x3fdf69364a20e788, 0x3fdf9b78b80c84e2, 0x3fdfcdbc1e0331ff,
    0x3fdfffffffffffff, 0x3fe01921f0fe6700, 0x3fe03243a3f9bd8f, 0x3fe04b64daef8c3b,
    0x3fe0648557de8d99, 0x3fe07da4dcc7473b, 0x3fe096c32baca2af, 0x3fe0afe00694866a,
    0x3fe0c8fb2f886ec1, 0x3fe0e214689606bf, 0x3fe0fb2b73cfc107, 0x3fe11440134d709b,
    0x3fe12d52092ce19f, 0x3fe1466117927209, 0x3fe15f6d00a9aa41, 0x3fe1787586a5d5b1,
    0x3fe1917a6bc29b43, 0x3fe1aa7b724495c0, 0x3fe1c3785c79ec2d, 0x3fe1dc70ecbae9fc,
    0x3fe1f564e56a9731, 0x3fe20e5408f75063, 0x3fe2273e19db5eae, 0x3fe24022da9d8f79,
    0x3fe259020dd1cc27, 0x3fe271db7619b1a2, 0x3fe28aaed62527ca, 0x3fe2a37bf0b2f8be,
    0x3fe2bc42889167f9, 0x3fe2d502609ec956, 0x3fe2edbb3bca17e6, 0x3fe3066cdd138c98,
    0x3fe31f17078d34c1, 0x3fe337b97e5b886c, 0x3fe3505404b6008a, 0x3fe368e65de7ace4,
    0x3fe381704d4fc9ec, 0x3fe399f196625650, 0x3fe3b269fca8a862, 0x3fe3cad943c20344,
    0x3fe3e33f2f642be3, 0x3fe3fb9b835bfdbf, 0x3fe413ee038dff6b, 0x3fe42c3673f6f6e4,
    0x3fe4447498ac7d9e, 0x3fe45ca835dd945d, 0x3fe474d10fd336cf, 0x3fe48ceeeaf0eedc,
    0x3fe4a5018bb567c0, 0x3fe4bd08b6bb00e2, 0x3fe4d50430b86054, 0x3fe4ecf3be81052e,
    0x3fe504d72505d980, 0x3fe51cae2955c415, 0x3fe53478909e39da, 0x3fe54c36202bcf08,
    0x3fe563e69d6ac7f7, 0x3fe57b89cde7a9a4, 0x3fe5931f774fc9f1, 0x3fe5aaa75f71df86,
    0x3fe5c2214c3e9168, 0x3fe5d98d03c9063e, 0x3fe5f0ea4c47733a, 0x3fe60838ec13aab1,
    0x3fe61f78a9abaa58, 0x3fe636a94bb2292c, 0x3fe64dca98ef24f5, 0x3fe664dc58506f7f,
    0x3fe67bde50ea3b62, 0x3fe692d049f7a878, 0x3fe6a9b20adb4ff2, 0x3fe6c0835b1fd002,
    0x3fe6d7440278572f, 0x3fe6edf3c8c12f40, 0x3fe70492760047b8, 0x3fe71b1fd265c002,
    0x3fe7319ba64c7118, 0x3fe74805ba3a76d6, 0x3fe75e5dd6e1b8e2, 0x3fe774a3c5207315,
    0x3fe78ad74e01bd8f, 0x3fe7a0f83abe1444, 0x3fe7b70654bbde35, 0x3fe7cd01658ff419,
    0x3fe7e2e936fe26ae, 0x3fe7f8bd92f9c483, 0x3fe80e7e43a61f5b, 0x3fe8242b1357110e,
    0x3fe839c3cc917ff6, 0x3fe84f483a0be2f0, 0x3fe864b826aec4c6, 0x3fe87a135d95473f,
    0x3fe88f59aa0da590, 0x3fe8a48ad799b676, 0x3fe8b9a6b1ef6da4, 0x3fe8cead04f95cdc,
    0x3fe8e39d9cd73463, 0x3fe8f87845de430d, 0x3fe90d3ccc99f5ac, 0x3fe921eafdcc560f,
    0x3fe93682a66e8970, 0x3fe94b0393b14e54, 0x3fe95f6d92fd79f5, 0x3fe973c071f4750a,
    0x3fe987fbfe70b81a, 0x3fe99c200686472a, 0x3fe9b02c58832cf9, 0x3fe9c420c2eff592,
    0x3fe9d7fd1490285c, 0x3fe9ebc11c62c1a2, 0x3fe9ff6ca9a2ab6a, 0x3fea12ff8bc735d9,
    0x3fea267992848eea, 0x3fea39da8dcc39a4, 0x3fea4d224dcd849c, 0x3fea6050a2f60002,
    0x3fea73655df1f2f4, 0x3fea86604facd04e, 0x3fea99414951aacc, 0x3feaac081c4ba89b,
    0x3feabeb49a467650, 0x3fead146952eb928, 0x3feae3bddf3280c6, 0x3feaf61a4ac1b83a,
    0x3feb085baa8e9670, 0x3feb1a81d18e0df4, 0x3feb2c8c92f83c1e, 0x3feb3e7bc248d787,
    0x3feb504f333f9de6, 0x3feb6206b9e0c13b, 0x3feb73a22a755457, 0x3feb8521598bb6bd,
    0x3feb96841bf7ffca, 0x3feba7ca46d46946, 0x3febb8f3af81b930, 0x3febca002ba7aaf2,
    0x3febdaef913557d6, 0x3febebc1b6619ed9, 0x3febfc7671ab8bb8, 0x3fec0d0d99dabd66,
    0x3fec1d8705ffcbb7, 0x3fec2de28d74ac66, 0x3fec3e2007dd1760, 0x3fec4e3f4d26ea54,
    0x3fec5e40358a8ba0, 0x3fec6e22998b4c66, 0x3fec7de651f7ca06, 0x3fec8d8b37ea4ed0,
    0x3fec9d1124c931fe, 0x3fecac77f24736ea, 0x3fecbbbf7a63eba0, 0x3feccae7976c0692,
    0x3fecd9f023f9c3a0, 0x3fece8d8faf5406b, 0x3fecf7a1f794d7ca, 0x3fed064af55d7c9c,
    0x3fed14d3d02313c0, 0x3fed233c6408cd64, 0x3fed31848d817d70, 0x3fed3fac294ff34e,
    0x3fed4db3148750d2, 0x3fed5b992c8b606a, 0x3fed695e4f10ea88, 0x3fed77025a1e0a3a,
    0x3fed84852c0a8100, 0x3fed91e6a38009da, 0x3fed9f269f7aab89, 0x3fedac44ff490a02,
    0x3fedb941a28cb71e, 0x3fedc61c693a8274, 0x3fedd2d5339ac869, 0x3feddf6be249c076,
    0x3fedebe05637ca94, 0x3fedf83270a9bbee, 0x3fee046213392aa4, 0x3fee106f1fd4b8d8,
    0x3fee1c5978c05ed8, 0x3fee28210095b484, 0x3fee33c59a4439cd, 0x3fee3f4729119e7a,
    0x3fee4aa5909a08fa, 0x3fee55e0b4d05c80, 0x3fee60f879fe7e2e, 0x3fee6becc4c5997a,
    0x3fee76bd7a1e63ba, 0x3fee816a7f595ec9, 0x3fee8bf3ba1f1aee, 0x3fee9659107077cf,
    0x3feea09a68a6e49d, 0x3feeaab7a9749f58, 0x3feeb4b0b9e4f346, 0x3feebe85815c767c,
    0x3feec835e79946a3, 0x3feed1c1d4b344c4, 0x3feedb29311c504d, 0x3feee46be5a08130,
    0x3feeed89db66611e, 0x3feef682fbef23ed, 0x3feeff573116df15, 0x3fef08066514c056,
    0x3fef1090827b4372, 0x3fef18f574386712, 0x3fef21352595e0be, 0x3fef294f82394ffe,
    0x3fef314476247089, 0x3fef3913edb54ba2, 0x3fef40bdd5a66886, 0x3fef48421b0efbf9,
    0x3fef4fa0ab6316ed, 0x3fef56d97473d446, 0x3fef5dec646f85ba, 0x3fef64d969e1dfc2,
    0x3fef6ba073b424b2, 0x3fef7241712d4ede, 0x3fef78bc51f239e1, 0x3fef7f110605caf6,
    0x3fef853f7dc9186c, 0x3fef8b47a9fb902e, 0x3fef91297bbb1d6c, 0x3fef96e4e4844d4e,
    0x3fef9c79d63272c4, 0x3fefa1e842ffc96e, 0x3fefa7301d859796, 0x3fefac5158bc4f42,
    0x3fefb14be7fbae58, 0x3fefb61fbefadddc, 0x3fefbaccd1d0903c, 0x3fefbf5314f31eb7,
    0x3fefc3b27d38a5d4, 0x3fefc7eaffd720ed, 0x3fefcbfc926484ce, 0x3fefcfe72ad6d964,
    0x3fefd3aabf84528c, 0x3fefd747472367dd, 0x3fefdabcb8caeba0, 0x3fefde0b0bf220c3,
    0x3fefe1323870cfea, 0x3fefe432367f5b90, 0x3fefe70afeb6d33e, 0x3fefe9bc8a1105c2,
    0x3fefec46d1e89292, 0x3fefeea9cff8fa2b, 0x3feff0e57e5ead84, 0x3feff2f9d7971ca0,
    0x3feff4e6d680c41d, 0x3feff6ac765b39e2, 0x3feff84ab2c738d6, 0x3feff9c187c6abae,
    0x3feffb10f1bcb6bf, 0x3feffc38ed6dc0f0, 0x3feffd3977ff7bae, 0x3feffe128ef8e9fc,
    0x3feffec430426686, 0x3fefff4e5a25a8d0, 0x3fefffb10b4dc96e, 0x3fefffec42c74549,
    0x3ff0000000000000, 0x3fefffec42c74549, 0x3fefffb10b4dc96e, 0x3fefff4e5a25a8d0,
    0x3feffec430426686, 0x3feffe128ef8e9fc, 0x3feffd3977ff7bae, 0x3feffc38ed6dc0f0,
    0x3feffb10f1bcb6bf, 0x3feff9c187c6abae, 0x3feff84ab2c738d6, 0x3feff6ac765b39e2,
    0x3feff4e6d680c41d, 0x3feff2f9d7971ca0, 0x3feff0e57e5ead84, 0x3fefeea9cff8fa2b,
    0x3fefec46d1e89293, 0x3fefe9bc8a1105c2, 0x3fefe70afeb6d33e, 0x3fefe432367f5b91,
    0x3fefe1323870cfea, 0x3fefde0b0bf220c3, 0x3fefdabcb8caeba0, 0x3fefd747472367de,
    0x3fefd3aabf84528c, 0x3fefcfe72ad6d964, 0x3fefcbfc926484ce, 0x3fefc7eaffd720ee,
    0x3fefc3b27d38a5d5, 0x3fefbf5314f31eb8, 0x3fefbaccd1d0903c, 0x3fefb61fbefadddc,
    0x3fefb14be7fbae58, 0x3fefac5158bc4f42, 0x3fefa7301d859796, 0x3fefa1e842ffc96e,
    0x3fef9c79d63272c4, 0x3fef96e4e4844d4e, 0x3fef91297bbb1d6d, 0x3fef8b47a9fb902f,
    0x3fef853f7dc9186c, 0x3fef7f110605caf6, 0x3fef78bc51f239e2, 0x3fef7241712d4ede,
    0x3fef6ba073b424b2, 0x3fef64d969e1dfc2, 0x3fef5dec646f85ba, 0x3fef56d97473d447,
    0x3fef4fa0ab6316ee, 0x3fef48421b0efbfa, 0x3fef40bdd5a66887, 0x3fef3913edb54ba2,
    0x3fef31447624708a, 0x3fef294f82394ffe, 0x3fef21352595e0bf, 0x3fef18f574386712,
    0x3fef1090827b4372, 0x3fef08066514c056, 0x3feeff573116df16, 0x3feef682fbef23ee,
    0x3feeed89db66611e, 0x3feee46be5a08130, 0x3feedb29311c504e, 0x3feed1c1d4b344c4,
    0x3feec835e79946a4, 0x3feebe85815c767c, 0x3feeb4b0b9e4f346, 0x3feeaab7a9749f58,
    0x3feea09a68a6e49d, 0x3fee9659107077d0, 0x3fee8bf3ba1f1aee, 0x3fee816a7f595eca,
    0x3fee76bd7a1e63ba, 0x3fee6becc4c5997b, 0x3fee60f879fe7e2f, 0x3fee55e0b4d05c80,
    0x3fee4aa5909a08fa, 0x3fee3f4729119e7a, 0x3fee33c59a4439ce, 0x3fee28210095b484,
    0x3fee1c5978c05ed8, 0x3fee106f1fd4b8d8, 0x3fee046213392aa4, 0x3fedf83270a9bbef,
    0x3fedebe05637ca95, 0x3feddf6be249c076, 0x3fedd2d5339ac86a, 0x3fedc61c693a8274,
    0x3fedb941a28cb71f, 0x3fedac44ff490a02, 0x3fed9f269f7aab8a, 0x3fed91e6a38009da,
    0x3fed84852c0a8100, 0x3fed77025a1e0a3a, 0x3fed695e4f10ea89, 0x3fed5b992c8b606a,
    0x3fed4db3148750d2, 0x3fed3fac294ff34e, 0x3fed31848d817d71, 0x3fed233c6408cd64,
    0x3fed14d3d02313c1, 0x3fed064af55d7c9c, 0x3fecf7a1f794d7ca, 0x3fece8d8faf5406c,
    0x3fecd9f023f9c3a0, 0x3feccae7976c0692, 0x3fecbbbf7a63eba1, 0x3fecac77f24736eb,
    0x3fec9d1124c931fe, 0x3fec8d8b37ea4ed1, 0x3fec7de651f7ca07, 0x3fec6e22998b4c66,
    0x3fec5e40358a8ba1, 0x3fec4e3f4d26ea56, 0x3fec3e2007dd1760, 0x3fec2de28d74ac66,
    0x3fec1d8705ffcbb8, 0x3fec0d0d99dabd66, 0x3febfc7671ab8bb8, 0x3febebc1b6619eda,
    0x3febdaef913557d8, 0x3febca002ba7aaf3, 0x3febb8f3af81b930, 0x3feba7ca46d46948,
    0x3feb96841bf7ffcc, 0x3feb8521598bb6be, 0x3feb73a22a755458, 0x3feb6206b9e0c13c,
    0x3feb504f333f9de7, 0x3feb3e7bc248d788, 0x3feb2c8c92f83c20, 0x3feb1a81d18e0df4,
    0x3feb085baa8e9670, 0x3feaf61a4ac1b83a, 0x3feae3bddf3280c7, 0x3fead146952eb928,
    0x3feabeb49a467651, 0x3feaac081c4ba89c, 0x3fea99414951aacc, 0x3fea86604facd04e,
    0x3fea73655df1f2f6, 0x3fea6050a2f60001, 0x3fea4d224dcd849c, 0x3fea39da8dcc39a4,
    0x3fea267992848eed, 0x3fea12ff8bc735d8, 0x3fe9ff6ca9a2ab6a, 0x3fe9ebc11c62c1a3,
    0x3fe9d7fd1490285e, 0x3fe9c420c2eff590, 0x3fe9b02c58832cfa, 0x3fe99c200686472d,
    0x3fe987fbfe70b81a, 0x3fe973c071f4750c, 0x3fe95f6d92fd79f6, 0x3fe94b0393b14e56,
    0x3fe93682a66e896f, 0x3fe921eafdcc5610, 0x3fe90d3ccc99f5ae, 0x3fe8f87845de4310,
    0x3fe8e39d9cd73464, 0x3fe8cead04f95cdc, 0x3fe8b9a6b1ef6da6, 0x3fe8a48ad799b675,
    0x3fe88f59aa0da592, 0x3fe87a135d954740, 0x3fe864b826aec4ca, 0x3fe84f483a0be2f0,
    0x3fe839c3cc917ff7, 0x3fe8242b1357110e, 0x3fe80e7e43a61f5e, 0x3fe7f8bd92f9c484,
    0x3fe7e2e936fe26af, 0x3fe7cd01658ff41c, 0x3fe7b70654bbde34, 0x3fe7a0f83abe1445,
    0x3fe78ad74e01bd90, 0x3fe774a3c5207318, 0x3fe75e5dd6e1b8e2, 0x3fe74805ba3a76d7,
    0x3fe7319ba64c7119, 0x3fe71b1fd265c005, 0x3fe70492760047ba, 0x3fe6edf3c8c12f41,
    0x3fe6d74402785732, 0x3fe6c0835b1fd002, 0x3fe6a9b20adb4ff2, 0x3fe692d049f7a87a,
    0x3fe67bde50ea3b65, 0x3fe664dc58506f7f, 0x3fe64dca98ef24f6, 0x3fe636a94bb2292e,
    0x3fe61f78a9abaa5b, 0x3fe60838ec13aab1, 0x3fe5f0ea4c47733b, 0x3fe5d98d03c90640,
    0x3fe5c2214c3e9167, 0x3fe5aaa75f71df86, 0x3fe5931f774fc9f3, 0x3fe57b89cde7a9a7,
    0x3fe563e69d6ac7f7, 0x3fe54c36202bcf0a, 0x3fe53478909e39dc, 0x3fe51cae2955c414,
    0x3fe504d72505d980, 0x3fe4ecf3be81052f, 0x3fe4d50430b86056, 0x3fe4bd08b6bb00e1,
    0x3fe4a5018bb567c2, 0x3fe48ceeeaf0eede, 0x3fe474d10fd336d2, 0x3fe45ca835dd945d,
    0x3fe4447498ac7d9e, 0x3fe42c3673f6f6e6, 0x3fe413ee038dff6a, 0x3fe3fb9b835bfdbf,
    0x3fe3e33f2f642be4, 0x3fe3cad943c20346, 0x3fe3b269fca8a861, 0x3fe399f196625651,
    0x3fe381704d4fc9ee, 0x3fe368e65de7ace7, 0x3fe3505404b6008a, 0x3fe337b97e5b886e,
    0x3fe31f17078d34c3, 0x3fe3066cdd138c98, 0x3fe2edbb3bca17e6, 0x3fe2d502609ec957,
    0x3fe2bc42889167fb, 0x3fe2a37bf0b2f8be, 0x3fe28aaed62527cc, 0x3fe271db7619b1a4,
    0x3fe259020dd1cc2a, 0x3fe24022da9d8f7a, 0x3fe2273e19db5eb0, 0x3fe20e5408f75066,
    0x3fe1f564e56a9730, 0x3fe1dc70ecbae9fd, 0x3fe1c3785c79ec2e, 0x3fe1aa7b724495c2,
    0x3fe1917a6bc29b42, 0x3fe1787586a5d5b2, 0x3fe15f6d00a9aa43, 0x3fe146611792720c,
    0x3fe12d52092ce19f, 0x3fe11440134d709c, 0x3fe0fb2b73cfc109, 0x3fe0e214689606be,
    0x3fe0c8fb2f886ec1, 0x3fe0afe00694866b, 0x3fe096c32baca2b1, 0x3fe07da4dcc7473b,
    0x3fe0648557de8d9a, 0x3fe04b64daef8c3e, 0x3fe03243a3f9bd92, 0x3fe01921f0fe6700,
    0x3fe0000000000001, 0x3fdfcdbc1e033203, 0x3fdf9b78b80c84e0, 0x3fdf69364a20e788,
    0x3fdf36f55042e4ce, 0x3fdf04b64671718d, 0x3fded279a8a6baa2, 0x3fdea03ff2d6f32d,
    0x3fde6e09a0ef2282, 0x3fde3bd72ed3f287, 0x3fde09a918607df2, 0x3fddd77fd9651ecb,
    0x3fdda55beda63cc5, 0x3fdd733dd0db1beb, 0x3fdd4125feacab7d, 0x3fdd0f14f2b4549e,
    0x3fdcdd0b287ac97f, 0x3fdcab091b76d47e, 0x3fdc790f470c27a6, 0x3fdc471e268a2c0a,
    0x3fdc1536352ad1a4, 0x3fdbe357ee115f38, 0x3fdbb183cc4942a4, 0x3fdb7fba4ac4e110,
    0x3fdb4dfbe45c67b0, 0x3fdb1c4913cc9cbc, 0x3fdaeaa253b5b06c, 0x3fdab9081e9a0e88,
    0x3fda877aeedd300e, 0x3fda55fb3ec26d54, 0x3fda2489886bd037, 0x3fd9f32645d8e6d4,
    0x3fd9c1d1f0e5967d, 0x3fd9908d0348ef28, 0x3fd95f57f693fef0, 0x3fd92e334430a636,
    0x3fd8fd1f65606c28, 0x3fd8cc1cd33b5361, 0x3fd89b2c06aeaf40, 0x3fd86a4d787bf978,
    0x3fd83981a137a83a, 0x3fd808c8f9480485, 0x3fd7d823f8e4012e, 0x3fd7a79318121238,
    0x3fd77716cea704c6, 0x3fd746af9444d748, 0x3fd7165de0599260, 0x3fd6e6222a1e2248,
    0x3fd6b5fce8953080, 0x3fd685ee9289fe42, 0x3fd655f79e8f3f56, 0x3fd6261882fdf5a6,
    0x3fd5f651b5f44d02, 0x3fd5c6a3ad5477dc, 0x3fd5970edec38c4b, 0x3fd56793bfa861f0,
    0x3fd53832c52a7016, 0x3fd508ec6430acb5, 0x3fd4d9c111606c1e, 0x3fd4aab1411c40f8,
    0x3fd47bbd6782dd36, 0x3fd44ce5f86df384, 0x3fd41e2b6771198e, 0x3fd3ef8e27d8aaa2,
    0x3fd3c10eaca8ab4c, 0x3fd392ad689bada8, 0x3fd3646ace21b616, 0x3fd336474f5f2104,
    0x3fd308435e2b893a, 0x3fd2da5f6c10af0e, 0x3fd2ac9bea49601e, 0x3fd27ef949c06000,
    0x3fd25177fb0f51a0, 0x3fd224186e7da181, 0x3fd1f6db13ff7090, 0x3fd1c9c05b347ff9,
    0x3fd19cc8b3671dd1, 0x3fd16ff48b8b1254, 0x3fd14344523c8e40, 0x3fd116b875bf19d3,
    0x3fd0ea5163fc84e3, 0x3fd0be0f8a83d778, 0x3fd091f35688439a, 0x3fd065fd34e017cc,
    0x3fd03a2d9203b2a4, 0x3fd00e84da0c76fc, 0x3fcfc606f167828e, 0x3fcf6f53b2a3bbcc,
    0x3fcf18f0cdba0028, 0x3fcec2df17d07446, 0x3fce6d1f6544ece0, 0x3fce17b289aae306,
    0x3fcdc29957c969c0, 0x3fcd6dd4a1992632, 0x3fcd19653842496e, 0x3fccc54bec1a8c92,
    0x3fcc71898ca32e76, 0x3fcc1e1ee886f3c6, 0x3fcbcb0ccd982950, 0x3fcb785408cea7c6,
    0x3fcb25f56645da4a, 0x3fcad3f1b13ac6ae, 0x3fca8249b40a182e, 0x3fca30fe382e2bd8,
    0x3fc9e010063d1f9e, 0x3fc98f7fe5e6e352, 0x3fc93f4e9df34c1e, 0x3fc8ef7cf44029c2,
    0x3fc8a00badbf5e8a, 0x3fc850fb8e74f97a, 0x3fc8024d5975525c, 0x3fc7b401d0e328a4,
    0x3fc76619b5edc452, 0x3fc71895c8cf1974, 0x3fc6cb76c8c9ed94, 0x3fc67ebd74280000,
    0x3fc6326a8838342e, 0x3fc5e67ec14cbecc, 0x3fc59afadab954da, 0x3fc54fdf8ed15d8e,
    0x3fc5052d96e626c2, 0x3fc4bae5ab451b64, 0x3fc471088335fcee, 0x3fc42796d4f91f16,
    0x3fc3de9155c5a644, 0x3fc395f8b9c7c832, 0x3fc34dcdb41f0f8e, 0x3fc30610f6dca1e0,
    0x3fc2bec33301886a, 0x3fc277e5187cfb1c, 0x3fc23177562aaea0, 0x3fc1eb7a99d1250e,
    0x3fc1a5ef902000d8, 0x3fc160d6e4ae5aec, 0x3fc11c3141f91b3c, 0x3fc0d7ff51615438,
    0x3fc09441bb2aa0a6, 0x3fc050f9267984a4, 0x3fc00e263951d11e, 0x3fbf9793312a14d8,
    0x3fbf13c7d001a254, 0x3fbe90eb945a9ccc, 0x3fbe0effc1174508, 0x3fbd8e0596c8ad5c,
    0x3fbd0dfe53aba308, 0x3fbc8eeb33a59ccc, 0x3fbc10cd7041afd0, 0x3fbb93a640ad8980,
    0x3fbb1776d9b67020, 0x3fba9c406dc648a4, 0x3fba22042ce0a300, 0x3fb9a8c3449fcb80,
    0x3fb9307ee031e2f8, 0x3fb8b9382855fcac, 0x3fb842f0435941b4, 0x3fb7cda855141b30,
    0x3fb759617ee761f8, 0x3fb6e61cdfb994e4, 0x3fb673db93f41480, 0x3fb6029eb580659c,
    0x3fb592675bc57974, 0x3fb523369ba4fcb4, 0x3fb4b50d8778abc8, 0x3fb447ed2f0fae30,
    0x3fb3dbd69fabf804, 0x3fb370cae3ffb134, 0x3fb306cb042aa3c4, 0x3fb29dd805b7afec,
    0x3fb235f2eb9a470c, 0x3fb1cf1cb62bec64, 0x3fb169566329bcc4, 0x3fb104a0edb1fc58,
    0x3fb0a0fd4e41ab5c, 0x3fb03e6c7ab22094, 0x3fafb9decc6d55b0, 0x3faef90e02b47288,
    0x3fae3a6873fa1288, 0x3fad7deff6a4b7e0, 0x3facc3a65bbc6328, 0x3fac0b8d6ee61870,
    0x3fab55a6f65f7068, 0x3faaa1f4b2fa3810, 0x3fa9f07860181d20, 0x3fa94133b3a66858,
    0x3fa894285e19c478, 0x3fa7e9580a6a1368, 0x3fa740c45e0e5120, 0x3fa69a6ef8f88318,
    0x3fa5f6597591b640, 0x3fa5548568b60a78, 0x3fa4b4f461b0cbb0, 0x3fa417a7ea389840,
    0x3fa37ca1866b95e0, 0x3fa2e3e2b4cbb3c0, 0x3fa24d6cee3afb30, 0x3fa1b941a5f7ed10,
    0x3fa127624999ee18, 0x3fa097d0410dc138, 0x3fa00a8cee920eb0, 0x3f9eff335d67f560,
    0x3f9dedefb09791b0, 0x3f9ce15178f31dc0, 0x3f9bd95b4d43e830, 0x3f9ad60fb8d60030,
    0x3f99d7713b71eee0, 0x3f98dd8249568bc0, 0x3f97e8454b32ef50, 0x3f96f7bc9e2080d0,
    0x3f960bea939d2260, 0x3f9524d171857730, 0x3f944273720f48d0, 0x3f9364d2c3c407c0,
    0x3f928bf1897b69d0, 0x3f91b7d1da562450, 0x3f90e875c1b8c3d0, 0x3f901ddf3f46a140,
    0x3f8eb0208db9e520, 0x3f8d2e15811bf480, 0x3f8bb5a11138a4c0, 0x3f8a46c6deecac60,
    0x3f88e18a73634f00, 0x3f8785ef400da4a0, 0x3f8633f89e9a1a60, 0x3f84eba9d0ec2f80,
    0x3f83ad0601146a20, 0x3f82781041488900, 0x3f814ccb8bdbf120, 0x3f802b3ac3385240,
    0x3f7e26c163ad1600, 0x3f7c0a80146f8940, 0x3f7a01b6cdbd9980, 0x3f780c6a94934e00,
    0x3f762aa03dd6ba80, 0x3f745c5c6e4c1140, 0x3f72a1a39a8a2fc0, 0x3f70fa7a06ef9ec0,
    0x3f6ecdc78f301680, 0x3f6bcdc980a46f80, 0x3f68f501492cc280, 0x3f664375eefa3e00,
    0x3f63b92e176d6d00, 0x3f6156300705d500, 0x3f5e350342a4f700, 0x3f5a0c50d1c6c000,
    0x3f563252fe77c600, 0x3f52a713498c3c00, 0x3f4ed534e31ca600, 0x3f48f9e0e5514800,
    0x3f43bc390d250400, 0x3f3e389491f88400, 0x3f36344004229000, 0x3f2ed71071604000,
    0x3f23bcfbd9979800, 0x3f1634bb4ae5f000, 0x3f03bd2c8da4a000, 0x3ee3bd38bab70000,
];

/// Real parts of `e^(-2πi·k/STFT_SIZE)` for `k` in `0 .. STFT_SIZE / 2`, as `f64` bit
/// patterns. Generated — do not edit by hand.
pub const TWIDDLE_REAL_BITS: [u64; STFT_SIZE / 2] = [
    0x3ff0000000000000, 0x3fefffd8858e8a92, 0x3fefff62169b92db, 0x3feffe9cb44b51a1,
    0x3feffd886084cd0d, 0x3feffc251df1d3f8, 0x3feffa72effef75d, 0x3feff871dadb81df,
    0x3feff621e3796d7e, 0x3feff3830f8d575c, 0x3feff095658e71ad, 0x3fefed58ecb673c4,
    0x3fefe9cdad01883a, 0x3fefe5f3af2e3940, 0x3fefe1cafcbd5b09, 0x3fefdd539ff1f456,
    0x3fefd88da3d12526, 0x3fefd37914220b84, 0x3fefce15fd6da67b, 0x3fefc8646cfeb721,
    0x3fefc26470e19fd3, 0x3fefbc1617e44186, 0x3fefb5797195d741, 0x3fefae8e8e46cfbb,
    0x3fefa7557f08a517, 0x3fef9fce55adb2c8, 0x3fef97f924c9099b, 0x3fef8fd5ffae41db,
    0x3fef8764fa714ba9, 0x3fef7ea629e63d6e, 0x3fef7599a3a12077, 0x3fef6c3f7df5bbb7,
    0x3fef6297cff75cb0, 0x3fef58a2b1789e84, 0x3fef4e603b0b2f2d, 0x3fef43d085ff92dd,
    0x3fef38f3ac64e589, 0x3fef2dc9c9089a9d, 0x3fef2252f7763ada, 0x3fef168f53f7205d,
    0x3fef0a7efb9230d7, 0x3feefe220c0b95ed, 0x3feef178a3e473c2, 0x3feee482e25a9dbc,
    0x3feed740e7684963, 0x3feec9b2d3c3bf84, 0x3feebbd8c8df0b74, 0x3feeadb2e8e7a88e,
    0x3fee9f4156c62dda, 0x3fee9084361df7f3, 0x3fee817bab4cd10d, 0x3fee7227db6a9744,
    0x3fee6288ec48e112, 0x3fee529f04729ffc, 0x3fee426a4b2bc17e, 0x3fee31eae870ce25,
    0x3fee212104f686e5, 0x3fee100cca2980ac, 0x3fedfeae622dbe2b, 0x3feded05f7de47da,
    0x3feddb13b6ccc23d, 0x3fedc8d7cb410260, 0x3fedb6526238a09b, 0x3feda383a9668988,
    0x3fed906bcf328d46, 0x3fed7d0b02b8ecfa, 0x3fed696173c9e68b, 0x3fed556f52e93eb1,
    0x3fed4134d14dc93a, 0x3fed2cb220e0ef9f, 0x3fed17e7743e35dc, 0x3fed02d4feb2bd92,
    0x3feced7af43cc773, 0x3fecd7d9898b32f6, 0x3fecc1f0f3fcfc5c, 0x3fecabc169a0b901,
    0x3fec954b213411f5, 0x3fec7e8e52233cf3, 0x3fec678b3488739b, 0x3fec5042012b6907,
    0x3fec38b2f180bdb1, 0x3fec20de3fa971b0, 0x3fec08c426725549, 0x3febf064e15377dd,
    0x3febd7c0ac6f952a, 0x3febbed7c49380ea, 0x3feba5aa673590d3, 0x3feb8c38d27504e9,
    0x3feb728345196e3e, 0x3feb5889fe921405, 0x3feb3e4d3ef55712, 0x3feb23cd470013b4,
    0x3feb090a58150200, 0x3feaee04b43c1474, 0x3fead2bc9e21d511, 0x3feab7325916c0d4,
    0x3fea9b66290ea1a3, 0x3fea7f58529fe69d, 0x3fea63091b02fae2, 0x3fea4678c8119ac8,
    0x3fea29a7a0462782, 0x3fea0c95eabaf937, 0x3fe9ef43ef29af94, 0x3fe9d1b1f5ea80d6,
    0x3fe9b3e047f38741, 0x3fe995cf2ed80d22, 0x3fe9777ef4c7d742, 0x3fe958efe48e6dd7,
    0x3fe93a22499263fc, 0x3fe91b166fd49da2, 0x3fe8fbcca3ef940d, 0x3fe8dc45331698cc,
    0x3fe8bc806b151741, 0x3fe89c7e9a4dd4ab, 0x3fe87c400fba2ebf, 0x3fe85bc51ae958cc,
    0x3fe83b0e0bff976e, 0x3fe81a1b33b57acc, 0x3fe7f8ece3571771, 0x3fe7d7836cc33db3,
    0x3fe7b5df226aafaf, 0x3fe79400574f55e5, 0x3fe771e75f037261, 0x3fe74f948da8d28d,
    0x3fe72d0837efff97, 0x3fe70a42b3176d7a, 0x3fe6e74454eaa8ae, 0x3fe6c40d73c18275,
    0x3fe6a09e667f3bcd, 0x3fe67cf78491af10, 0x3fe6591925f0783e, 0x3fe63503a31c1be9,
    0x3fe610b7551d2cdf, 0x3fe5ec3495837074, 0x3fe5c77bbe65018d, 0x3fe5a28d2a5d7251,
    0x3fe57d69348cec9f, 0x3fe5581038975137, 0x3fe5328292a35596, 0x3fe50cc09f59a09b,
    0x3fe4e6cabbe3e5e9, 0x3fe4c0a145ec0005, 0x3fe49a449b9b0939, 0x3fe473b51b987347,
    0x3fe44cf325091dd6, 0x3fe425ff178e6bb2, 0x3fe3fed9534556d5, 0x3fe3d78238c58344,
    0x3fe3affa292050b9, 0x3fe3884185dfeb22, 0x3fe36058b10659f3, 0x3fe338400d0c8e57,
    0x3fe30ff7fce17036, 0x3fe2e780e3e8ea17, 0x3fe2bedb25faf3ea, 0x3fe2960727629ca8,
    0x3fe26d054cdd12df, 0x3fe243d5fb98ac20, 0x3fe21a799933eb59, 0x3fe1f0f08bbc861b,
    0x3fe1c73b39ae68c9, 0x3fe19d5a09f2b9b8, 0x3fe1734d63dedb49, 0x3fe14915af336cec,
    0x3fe11eb3541b4b23, 0x3fe0f426bb2a8e7f, 0x3fe0c9704d5d898e, 0x3fe09e907417c5e0,
    0x3fe073879922ffed, 0x3fe0485626ae221a, 0x3fe01cfc874c3eb7, 0x3fdfe2f64be71210,
    0x3fdf8ba4dbf89abb, 0x3fdf3405963fd069, 0x3fdedc1952ef78d7, 0x3fde83e0eaf85116,
    0x3fde2b5d3806f63e, 0x3fddd28f1481cc57, 0x3fdd79775b86e389, 0x3fdd2016e8e9db5b,
    0x3fdcc66e9931c45e, 0x3fdc6c7f4997000b, 0x3fdc1249d8011ee8, 0x3fdbb7cf2304bd02,
    0x3fdb5d1009e15cc2, 0x3fdb020d6c7f400b, 0x3fdaa6c82b6d3fcc, 0x3fda4b4127dea1e4,
    0x3fd9ef7943a8ed8a, 0x3fd993716141bdfe, 0x3fd9372a63bc93d7, 0x3fd8daa52ec8a4b0,
    0x3fd87de2a6aea964, 0x3fd820e3b04eaac5, 0x3fd7c3a9311dcce8, 0x3fd766340f2418f8,
    0x3fd7088530fa45a1, 0x3fd6aa9d7dc77e19, 0x3fd64c7ddd3f27c5, 0x3fd5ee27379ea693,
    0x3fd58f9a75ab1fdd, 0x3fd530d880af3c24, 0x3fd4d1e24278e76b, 0x3fd472b8a5571055,
    0x3fd4135c94176603, 0x3fd3b3cefa0414b9, 0x3fd35410c2e18154, 0x3fd2f422daec0389,
    0x3fd294062ed59f05, 0x3fd233bbabc3bb71, 0x3fd1d3443f4cdb3d, 0x3fd172a0d7765177,
    0x3fd111d262b1f678, 0x3fd0b0d9cfdbdb91, 0x3fd04fb80e37fdaf, 0x3fcfdcdc1adfedfc,
    0x3fcf19f97b215f1e, 0x3fce56ca1e101a20, 0x3fcd934fe5454317, 0x3fcccf8cb312b284,
    0x3fcc0b826a7e4f62, 0x3fcb4732ef3d6722, 0x3fca82a025b00451, 0x3fc9bdcbf2dc4368,
    0x3fc8f8b83c69a60d, 0x3fc83366e89c64c8, 0x3fc76dd9de50bf35, 0x3fc6a81304f64ab6,
    0x3fc5e214448b3fcb, 0x3fc51bdf8597c5f8, 0x3fc45576b1293e58, 0x3fc38edbb0cd8d13,
    0x3fc2c8106e8e613a, 0x3fc20116d4ec7bcf, 0x3fc139f0cedaf578, 0x3fc072a047ba831f,
    0x3fbf564e56a97314, 0x3fbdc70ecbae9fd1, 0x3fbc3785c79ec2de, 0x3fbaa7b724495c0e,
    0x3fb917a6bc29b438, 0x3fb787586a5d5b1f, 0x3fb5f6d00a9aa418, 0x3fb4661179272096,
    0x3fb2d52092ce19f8, 0x3fb1440134d709b6, 0x3faf656e79f820ea, 0x3fac428d12c0d7f0,
    0x3fa91f65f10dd824, 0x3fa5fc00d290cd57, 0x3fa2d865759455e4, 0x3f9f693731d1cef4,
    0x3f992155f7a36677, 0x3f92d936bbe30efd, 0x3f8921d1fcdec78f, 0x3f7921f0fe67009f,
    0x3c91a62633145c07, 0xbf7921f0fe670012, 0xbf8921d1fcdec749, 0xbf92d936bbe30ed9,
    0xbf992155f7a36654, 0xbf9f693731d1ced1, 0xbfa2d865759455d2, 0xbfa5fc00d290cd45,
    0xbfa91f65f10dd813, 0xbfac428d12c0d7df, 0xbfaf656e79f820d9, 0xbfb1440134d709ad,
    0xbfb2d52092ce19ef, 0xbfb466117927208e, 0xbfb5f6d00a9aa40f, 0xbfb787586a5d5b16,
    0xbfb917a6bc29b42f, 0xbfbaa7b724495c05, 0xbfbc3785c79ec2d5, 0xbfbdc70ecbae9fc8,
    0xbfbf564e56a9730b, 0xbfc072a047ba831b, 0xbfc139f0cedaf574, 0xbfc20116d4ec7bcb,
    0xbfc2c8106e8e6136, 0xbfc38edbb0cd8d0f, 0xbfc45576b1293e54, 0xbfc51bdf8597c5f3,
    0xbfc5e214448b3fc7, 0xbfc6a81304f64ab2, 0xbfc76dd9de50bf30, 0xbfc83366e89c64c4,
    0xbfc8f8b83c69a608, 0xbfc9bdcbf2dc4363, 0xbfca82a025b0044d, 0xbfcb4732ef3d671e,
    0xbfcc0b826a7e4f5e, 0xbfcccf8cb312b280, 0xbfcd934fe5454312, 0xbfce56ca1e101a1c,
    0xbfcf19f97b215f1a, 0xbfcfdcdc1adfedf7, 0xbfd04fb80e37fdad, 0xbfd0b0d9cfdbdb8f,
    0xbfd111d262b1f676, 0xbfd172a0d7765175, 0xbfd1d3443f4cdb3b, 0xbfd233bbabc3bb6f,
    0xbfd294062ed59f02, 0xbfd2f422daec0387, 0xbfd35410c2e18152, 0xbfd3b3cefa0414b7,
    0xbfd4135c94176600, 0xbfd472b8a5571053, 0xbfd4d1e24278e769, 0xbfd530d880af3c22,
    0xbfd58f9a75ab1fdb, 0xbfd5ee27379ea691, 0xbfd64c7ddd3f27c3, 0xbfd6aa9d7dc77e17,
    0xbfd7088530fa459f, 0xbfd766340f2418f6, 0xbfd7c3a9311dcce6, 0xbfd820e3b04eaac3,
    0xbfd87de2a6aea962, 0xbfd8daa52ec8a4ae, 0xbfd9372a63bc93d5, 0xbfd993716141bdfc,
    0xbfd9ef7943a8ed88, 0xbfda4b4127dea1e2, 0xbfdaa6c82b6d3fc6, 0xbfdb020d6c7f4009,
    0xbfdb5d1009e15cbc, 0xbfdbb7cf2304bd00, 0xbfdc1249d8011ee2, 0xbfdc6c7f49970009,
    0xbfdcc66e9931c460, 0xbfdd2016e8e9db59, 0xbfdd79775b86e38a, 0xbfddd28f1481cc55,
    0xbfde2b5d3806f63c, 0xbfde83e0eaf85110, 0xbfdedc1952ef78d5, 0xbfdf3405963fd063,
    0xbfdf8ba4dbf89ab9, 0xbfdfe2f64be7120b, 0xbfe01cfc874c3eb6, 0xbfe0485626ae221b,
    0xbfe073879922ffed, 0xbfe09e907417c5e1, 0xbfe0c9704d5d898d, 0xbfe0f426bb2a8e7e,
    0xbfe11eb3541b4b21, 0xbfe14915af336ceb, 0xbfe1734d63dedb47, 0xbfe19d5a09f2b9b7,
    0xbfe1c73b39ae68c6, 0xbfe1f0f08bbc861a, 0xbfe21a799933eb59, 0xbfe243d5fb98ac1e,
    0xbfe26d054cdd12df, 0xbfe2960727629ca7, 0xbfe2bedb25faf3ea, 0xbfe2e780e3e8ea15,
    0xbfe30ff7fce17035, 0xbfe338400d0c8e55, 0xbfe36058b10659f2, 0xbfe3884185dfeb23,
    0xbfe3affa292050b8, 0xbfe3d78238c58344, 0xbfe3fed9534556d3, 0xbfe425ff178e6bb2,
    0xbfe44cf325091dd5, 0xbfe473b51b987347, 0xbfe49a449b9b0937, 0xbfe4c0a145ec0004,
    0xbfe4e6cabbe3e5e7, 0xbfe50cc09f59a09b, 0xbfe5328292a35597, 0xbfe5581038975136,
    0xbfe57d69348ceca0, 0xbfe5a28d2a5d724f, 0xbfe5c77bbe65018c, 0xbfe5ec3495837073,
    0xbfe610b7551d2cdf, 0xbfe63503a31c1be7, 0xbfe6591925f0783d, 0xbfe67cf78491af0e,
    0xbfe6a09e667f3bcc, 0xbfe6c40d73c18276, 0xbfe6e74454eaa8ae, 0xbfe70a42b3176d7a,
    0xbfe72d0837efff95, 0xbfe74f948da8d28d, 0xbfe771e75f037260, 0xbfe79400574f55e5,
    0xbfe7b5df226aafad, 0xbfe7d7836cc33db2, 0xbfe7f8ece357176f, 0xbfe81a1b33b57acb,
    0xbfe83b0e0bff976e, 0xbfe85bc51ae958cb, 0xbfe87c400fba2ebf, 0xbfe89c7e9a4dd4a9,
    0xbfe8bc806b151741, 0xbfe8dc45331698cb, 0xbfe8fbcca3ef940d, 0xbfe91b166fd49da0,
    0xbfe93a22499263fb, 0xbfe958efe48e6dd5, 0xbfe9777ef4c7d741, 0xbfe995cf2ed80d23,
    0xbfe9b3e047f38740, 0xbfe9d1b1f5ea80d6, 0xbfe9ef43ef29af93, 0xbfea0c95eabaf937,
    0xbfea29a7a0462781, 0xbfea4678c8119ac8, 0xbfea63091b02fae0, 0xbfea7f58529fe69c,
    0xbfea9b66290ea1a4, 0xbfeab7325916c0d4, 0xbfead2bc9e21d511, 0xbfeaee04b43c1473,
    0xbfeb090a58150200, 0xbfeb23cd470013b3, 0xbfeb3e4d3ef55712, 0xbfeb5889fe921404,
    0xbfeb728345196e3d, 0xbfeb8c38d27504e7, 0xbfeba5aa673590d2, 0xbfebbed7c49380eb,
    0xbfebd7c0ac6f9529, 0xbfebf064e15377dd, 0xbfec08c426725548, 0xbfec20de3fa971b0,
    0xbfec38b2f180bdb0, 0xbfec5042012b6907, 0xbfec678b3488739a, 0xbfec7e8e52233cf3,
    0xbfec954b213411f4, 0xbfecabc169a0b900, 0xbfecc1f0f3fcfc5d, 0xbfecd7d9898b32f5,
    0xbfeced7af43cc773, 0xbfed02d4feb2bd92, 0xbfed17e7743e35dc, 0xbfed2cb220e0ef9e,
    0xbfed4134d14dc93a, 0xbfed556f52e93eb0, 0xbfed696173c9e68b, 0xbfed7d0b02b8ecf8,
    0xbfed906bcf328d46, 0xbfeda383a9668988, 0xbfedb6526238a09a, 0xbfedc8d7cb410260,
    0xbfeddb13b6ccc23c, 0xbfeded05f7de47da, 0xbfedfeae622dbe2a, 0xbfee100cca2980ac,
    0xbfee212104f686e4, 0xbfee31eae870ce25, 0xbfee426a4b2bc17d, 0xbfee529f04729ffc,
    0xbfee6288ec48e112, 0xbfee7227db6a9744, 0xbfee817bab4cd10d, 0xbfee9084361df7f2,
    0xbfee9f4156c62dda, 0xbfeeadb2e8e7a88d, 0xbfeebbd8c8df0b74, 0xbfeec9b2d3c3bf83,
    0xbfeed740e7684963, 0xbfeee482e25a9dbb, 0xbfeef178a3e473c2, 0xbfeefe220c0b95ed,
    0xbfef0a7efb9230d7, 0xbfef168f53f7205d, 0xbfef2252f7763ad9, 0xbfef2dc9c9089a9d,
    0xbfef38f3ac64e588, 0xbfef43d085ff92dd, 0xbfef4e603b0b2f2c, 0xbfef58a2b1789e84,
    0xbfef6297cff75cb0, 0xbfef6c3f7df5bbb7, 0xbfef7599a3a12077, 0xbfef7ea629e63d6e,
    0xbfef8764fa714ba9, 0xbfef8fd5ffae41da, 0xbfef97f924c9099b, 0xbfef9fce55adb2c8,
    0xbfefa7557f08a517, 0xbfefae8e8e46cfba, 0xbfefb5797195d741, 0xbfefbc1617e44186,
    0xbfefc26470e19fd3, 0xbfefc8646cfeb721, 0xbfefce15fd6da67b, 0xbfefd37914220b84,
    0xbfefd88da3d12525, 0xbfefdd539ff1f456, 0xbfefe1cafcbd5b09, 0xbfefe5f3af2e3940,
    0xbfefe9cdad01883a, 0xbfefed58ecb673c4, 0xbfeff095658e71ad, 0xbfeff3830f8d575c,
    0xbfeff621e3796d7e, 0xbfeff871dadb81df, 0xbfeffa72effef75d, 0xbfeffc251df1d3f8,
    0xbfeffd886084cd0d, 0xbfeffe9cb44b51a1, 0xbfefff62169b92db, 0xbfefffd8858e8a92,
];

/// Imaginary parts of `e^(-2πi·k/STFT_SIZE)`, as `f64` bit patterns. Generated — do not
/// edit by hand.
pub const TWIDDLE_IMAGINARY_BITS: [u64; STFT_SIZE / 2] = [
    0x8000000000000000, 0xbf7921f0fe670071, 0xbf8921d1fcdec784, 0xbf92d936bbe30efd,
    0xbf992155f7a3667e, 0xbf9f693731d1cf01, 0xbfa2d865759455cd, 0xbfa5fc00d290cd43,
    0xbfa91f65f10dd814, 0xbfac428d12c0d7e2, 0xbfaf656e79f820e0, 0xbfb1440134d709b2,
    0xbfb2d52092ce19f6, 0xbfb4661179272096, 0xbfb5f6d00a9aa419, 0xbfb787586a5d5b21,
    0xbfb917a6bc29b42c, 0xbfbaa7b724495c04, 0xbfbc3785c79ec2d5, 0xbfbdc70ecbae9fc8,
    0xbfbf564e56a9730e, 0xbfc072a047ba831d, 0xbfc139f0cedaf576, 0xbfc20116d4ec7bce,
    0xbfc2c8106e8e613a, 0xbfc38edbb0cd8d14, 0xbfc45576b1293e5a, 0xbfc51bdf8597c5f2,
    0xbfc5e214448b3fc6, 0xbfc6a81304f64ab2, 0xbfc76dd9de50bf31, 0xbfc83366e89c64c5,
    0xbfc8f8b83c69a60a, 0xbfc9bdcbf2dc4366, 0xbfca82a025b00451, 0xbfcb4732ef3d6722,
    0xbfcc0b826a7e4f63, 0xbfcccf8cb312b286, 0xbfcd934fe5454311, 0xbfce56ca1e101a1b,
    0xbfcf19f97b215f1a, 0xbfcfdcdc1adfedf8, 0xbfd04fb80e37fdae, 0xbfd0b0d9cfdbdb90,
    0xbfd111d262b1f677, 0xbfd172a0d7765177, 0xbfd1d3443f4cdb3d, 0xbfd233bbabc3bb72,
    0xbfd294062ed59f05, 0xbfd2f422daec0386, 0xbfd35410c2e18152, 0xbfd3b3cefa0414b7,
    0xbfd4135c94176602, 0xbfd472b8a5571054, 0xbfd4d1e24278e76a, 0xbfd530d880af3c24,
    0xbfd58f9a75ab1fdd, 0xbfd5ee27379ea693, 0xbfd64c7ddd3f27c6, 0xbfd6aa9d7dc77e16,
    0xbfd7088530fa459e, 0xbfd766340f2418f6, 0xbfd7c3a9311dcce7, 0xbfd820e3b04eaac4,
    0xbfd87de2a6aea963, 0xbfd8daa52ec8a4af, 0xbfd9372a63bc93d7, 0xbfd993716141bdfe,
    0xbfd9ef7943a8ed8a, 0xbfda4b4127dea1e4, 0xbfdaa6c82b6d3fc9, 0xbfdb020d6c7f4009,
    0xbfdb5d1009e15cc0, 0xbfdbb7cf2304bd01, 0xbfdc1249d8011ee7, 0xbfdc6c7f4997000a,
    0xbfdcc66e9931c45d, 0xbfdd2016e8e9db5b, 0xbfdd79775b86e389, 0xbfddd28f1481cc58,
    0xbfde2b5d3806f63b, 0xbfde83e0eaf85113, 0xbfdedc1952ef78d5, 0xbfdf3405963fd068,
    0xbfdf8ba4dbf89aba, 0xbfdfe2f64be71210, 0xbfe01cfc874c3eb7, 0xbfe0485626ae221a,
    0xbfe073879922ffed, 0xbfe09e907417c5e1, 0xbfe0c9704d5d898f, 0xbfe0f426bb2a8e7d,
    0xbfe11eb3541b4b22, 0xbfe14915af336ceb, 0xbfe1734d63dedb49, 0xbfe19d5a09f2b9b8,
    0xbfe1c73b39ae68c8, 0xbfe1f0f08bbc861b, 0xbfe21a799933eb58, 0xbfe243d5fb98ac1f,
    0xbfe26d054cdd12df, 0xbfe2960727629ca8, 0xbfe2bedb25faf3ea, 0xbfe2e780e3e8ea16,
    0xbfe30ff7fce17035, 0xbfe338400d0c8e57, 0xbfe36058b10659f3, 0xbfe3884185dfeb22,
    0xbfe3affa292050b9, 0xbfe3d78238c58343, 0xbfe3fed9534556d4, 0xbfe425ff178e6bb1,
    0xbfe44cf325091dd6, 0xbfe473b51b987347, 0xbfe49a449b9b0938, 0xbfe4c0a145ec0004,
    0xbfe4e6cabbe3e5e9, 0xbfe50cc09f59a09b, 0xbfe5328292a35596, 0xbfe5581038975137,
    0xbfe57d69348cec9f, 0xbfe5a28d2a5d7250, 0xbfe5c77bbe65018c, 0xbfe5ec3495837074,
    0xbfe610b7551d2cde, 0xbfe63503a31c1be9, 0xbfe6591925f0783e, 0xbfe67cf78491af10,
    0xbfe6a09e667f3bcc, 0xbfe6c40d73c18275, 0xbfe6e74454eaa8ae, 0xbfe70a42b3176d7a,
    0xbfe72d0837efff96, 0xbfe74f948da8d28d, 0xbfe771e75f037261, 0xbfe79400574f55e4,
    0xbfe7b5df226aafaf, 0xbfe7d7836cc33db2, 0xbfe7f8ece3571770, 0xbfe81a1b33b57acb,
    0xbfe83b0e0bff976d, 0xbfe85bc51ae958cc, 0xbfe87c400fba2ebf, 0xbfe89c7e9a4dd4aa,
    0xbfe8bc806b151741, 0xbfe8dc45331698cc, 0xbfe8fbcca3ef940c, 0xbfe91b166fd49da2,
    0xbfe93a22499263fb, 0xbfe958efe48e6dd7, 0xbfe9777ef4c7d741, 0xbfe995cf2ed80d22,
    0xbfe9b3e047f38740, 0xbfe9d1b1f5ea80d5, 0xbfe9ef43ef29af94, 0xbfea0c95eabaf936,
    0xbfea29a7a0462782, 0xbfea4678c8119ac8, 0xbfea63091b02fae2, 0xbfea7f58529fe69d,
    0xbfea9b66290ea1a3, 0xbfeab7325916c0d4, 0xbfead2bc9e21d510, 0xbfeaee04b43c1473,
    0xbfeb090a581501ff, 0xbfeb23cd470013b3, 0xbfeb3e4d3ef55712, 0xbfeb5889fe921405,
    0xbfeb728345196e3e, 0xbfeb8c38d27504e9, 0xbfeba5aa673590d2, 0xbfebbed7c49380ea,
    0xbfebd7c0ac6f9529, 0xbfebf064e15377dd, 0xbfec08c426725549, 0xbfec20de3fa971af,
    0xbfec38b2f180bdb0, 0xbfec5042012b6907, 0xbfec678b3488739b, 0xbfec7e8e52233cf3,
    0xbfec954b213411f5, 0xbfecabc169a0b900, 0xbfecc1f0f3fcfc5c, 0xbfecd7d9898b32f6,
    0xbfeced7af43cc773, 0xbfed02d4feb2bd92, 0xbfed17e7743e35db, 0xbfed2cb220e0ef9f,
    0xbfed4134d14dc93a, 0xbfed556f52e93eb1, 0xbfed696173c9e68b, 0xbfed7d0b02b8ecf9,
    0xbfed906bcf328d46, 0xbfeda383a9668987, 0xbfedb6526238a09a, 0xbfedc8d7cb410260,
    0xbfeddb13b6ccc23c, 0xbfeded05f7de47d9, 0xbfedfeae622dbe2b, 0xbfee100cca2980ac,
    0xbfee212104f686e5, 0xbfee31eae870ce25, 0xbfee426a4b2bc17e, 0xbfee529f04729ffc,
    0xbfee6288ec48e112, 0xbfee7227db6a9744, 0xbfee817bab4cd10c, 0xbfee9084361df7f2,
    0xbfee9f4156c62ddb, 0xbfeeadb2e8e7a88e, 0xbfeebbd8c8df0b74, 0xbfeec9b2d3c3bf84,
    0xbfeed740e7684963, 0xbfeee482e25a9dbc, 0xbfeef178a3e473c2, 0xbfeefe220c0b95ec,
    0xbfef0a7efb9230d7, 0xbfef168f53f7205d, 0xbfef2252f7763ad9, 0xbfef2dc9c9089a9d,
    0xbfef38f3ac64e589, 0xbfef43d085ff92dd, 0xbfef4e603b0b2f2d, 0xbfef58a2b1789e84,
    0xbfef6297cff75cb0, 0xbfef6c3f7df5bbb7, 0xbfef7599a3a12077, 0xbfef7ea629e63d6e,
    0xbfef8764fa714ba9, 0xbfef8fd5ffae41db, 0xbfef97f924c9099b, 0xbfef9fce55adb2c8,
    0xbfefa7557f08a517, 0xbfefae8e8e46cfbb, 0xbfefb5797195d741, 0xbfefbc1617e44186,
    0xbfefc26470e19fd3, 0xbfefc8646cfeb721, 0xbfefce15fd6da67b, 0xbfefd37914220b84,
    0xbfefd88da3d12525, 0xbfefdd539ff1f456, 0xbfefe1cafcbd5b09, 0xbfefe5f3af2e3940,
    0xbfefe9cdad01883a, 0xbfefed58ecb673c4, 0xbfeff095658e71ad, 0xbfeff3830f8d575c,
    0xbfeff621e3796d7e, 0xbfeff871dadb81df, 0xbfeffa72effef75d, 0xbfeffc251df1d3f8,
    0xbfeffd886084cd0d, 0xbfeffe9cb44b51a1, 0xbfefff62169b92db, 0xbfefffd8858e8a92,
    0xbff0000000000000, 0xbfefffd8858e8a92, 0xbfefff62169b92db, 0xbfeffe9cb44b51a1,
    0xbfeffd886084cd0d, 0xbfeffc251df1d3f8, 0xbfeffa72effef75d, 0xbfeff871dadb81df,
    0xbfeff621e3796d7e, 0xbfeff3830f8d575c, 0xbfeff095658e71ad, 0xbfefed58ecb673c4,
    0xbfefe9cdad01883a, 0xbfefe5f3af2e3941, 0xbfefe1cafcbd5b09, 0xbfefdd539ff1f456,
    0xbfefd88da3d12526, 0xbfefd37914220b84, 0xbfefce15fd6da67b, 0xbfefc8646cfeb721,
    0xbfefc26470e19fd3, 0xbfefbc1617e44186, 0xbfefb5797195d741, 0xbfefae8e8e46cfbb,
    0xbfefa7557f08a517, 0xbfef9fce55adb2c8, 0xbfef97f924c9099b, 0xbfef8fd5ffae41db,
    0xbfef8764fa714ba9, 0xbfef7ea629e63d6e, 0xbfef7599a3a12077, 0xbfef6c3f7df5bbb7,
    0xbfef6297cff75cb0, 0xbfef58a2b1789e84, 0xbfef4e603b0b2f2d, 0xbfef43d085ff92dd,
    0xbfef38f3ac64e589, 0xbfef2dc9c9089a9d, 0xbfef2252f7763ada, 0xbfef168f53f7205d,
    0xbfef0a7efb9230d7, 0xbfeefe220c0b95ed, 0xbfeef178a3e473c2, 0xbfeee482e25a9dbc,
    0xbfeed740e7684963, 0xbfeec9b2d3c3bf84, 0xbfeebbd8c8df0b75, 0xbfeeadb2e8e7a88e,
    0xbfee9f4156c62ddb, 0xbfee9084361df7f2, 0xbfee817bab4cd10d, 0xbfee7227db6a9744,
    0xbfee6288ec48e112, 0xbfee529f04729ffd, 0xbfee426a4b2bc17f, 0xbfee31eae870ce25,
    0xbfee212104f686e5, 0xbfee100cca2980ac, 0xbfedfeae622dbe2b, 0xbfeded05f7de47da,
    0xbfeddb13b6ccc23c, 0xbfedc8d7cb410260, 0xbfedb6526238a09b, 0xbfeda383a9668988,
    0xbfed906bcf328d46, 0xbfed7d0b02b8ecfa, 0xbfed696173c9e68b, 0xbfed556f52e93eb1,
    0xbfed4134d14dc93a, 0xbfed2cb220e0ef9f, 0xbfed17e7743e35dd, 0xbfed02d4feb2bd92,
    0xbfeced7af43cc774, 0xbfecd7d9898b32f6, 0xbfecc1f0f3fcfc5d, 0xbfecabc169a0b901,
    0xbfec954b213411f4, 0xbfec7e8e52233cf4, 0xbfec678b3488739b, 0xbfec5042012b6908,
    0xbfec38b2f180bdb1, 0xbfec20de3fa971b0, 0xbfec08c426725549, 0xbfebf064e15377de,
    0xbfebd7c0ac6f952a, 0xbfebbed7c49380eb, 0xbfeba5aa673590d3, 0xbfeb8c38d27504e8,
    0xbfeb728345196e3e, 0xbfeb5889fe921405, 0xbfeb3e4d3ef55712, 0xbfeb23cd470013b4,
    0xbfeb090a58150201, 0xbfeaee04b43c1474, 0xbfead2bc9e21d512, 0xbfeab7325916c0d5,
    0xbfea9b66290ea1a5, 0xbfea7f58529fe69d, 0xbfea63091b02fae1, 0xbfea4678c8119ac9,
    0xbfea29a7a0462782, 0xbfea0c95eabaf938, 0xbfe9ef43ef29af94, 0xbfe9d1b1f5ea80d7,
    0xbfe9b3e047f38741, 0xbfe995cf2ed80d24, 0xbfe9777ef4c7d742, 0xbfe958efe48e6dd6,
    0xbfe93a22499263fc, 0xbfe91b166fd49da1, 0xbfe8fbcca3ef940e, 0xbfe8dc45331698cc,
    0xbfe8bc806b151742, 0xbfe89c7e9a4dd4aa, 0xbfe87c400fba2ec0, 0xbfe85bc51ae958cd,
    0xbfe83b0e0bff976f, 0xbfe81a1b33b57acc, 0xbfe7f8ece3571770, 0xbfe7d7836cc33db3,
    0xbfe7b5df226aafae, 0xbfe79400574f55e6, 0xbfe771e75f037261, 0xbfe74f948da8d28e,
    0xbfe72d0837efff96, 0xbfe70a42b3176d7b, 0xbfe6e74454eaa8af, 0xbfe6c40d73c18277,
    0xbfe6a09e667f3bcd, 0xbfe67cf78491af0f, 0xbfe6591925f0783e, 0xbfe63503a31c1be9,
    0xbfe610b7551d2ce0, 0xbfe5ec3495837074, 0xbfe5c77bbe65018e, 0xbfe5a28d2a5d7250,
    0xbfe57d69348ceca1, 0xbfe5581038975138, 0xbfe5328292a35598, 0xbfe50cc09f59a09c,
    0xbfe4e6cabbe3e5e8, 0xbfe4c0a145ec0005, 0xbfe49a449b9b0938, 0xbfe473b51b987348,
    0xbfe44cf325091dd6, 0xbfe425ff178e6bb3, 0xbfe3fed9534556d4, 0xbfe3d78238c58346,
    0xbfe3affa292050ba, 0xbfe3884185dfeb24, 0xbfe36058b10659f4, 0xbfe338400d0c8e56,
    0xbfe30ff7fce17036, 0xbfe2e780e3e8ea16, 0xbfe2bedb25faf3eb, 0xbfe2960727629ca8,
    0xbfe26d054cdd12e0, 0xbfe243d5fb98ac1f, 0xbfe21a799933eb5b, 0xbfe1f0f08bbc861c,
    0xbfe1c73b39ae68c8, 0xbfe19d5a09f2b9b9, 0xbfe1734d63dedb48, 0xbfe14915af336cec,
    0xbfe11eb3541b4b22, 0xbfe0f426bb2a8e7f, 0xbfe0c9704d5d898f, 0xbfe09e907417c5e2,
    0xbfe073879922ffee, 0xbfe0485626ae221d, 0xbfe01cfc874c3eb8, 0xbfdfe2f64be7120e,
    0xbfdf8ba4dbf89abc, 0xbfdf3405963fd066, 0xbfdedc1952ef78d8, 0xbfde83e0eaf85113,
    0xbfde2b5d3806f63f, 0xbfddd28f1481cc58, 0xbfdd79775b86e38d, 0xbfdd2016e8e9db5c,
    0xbfdcc66e9931c463, 0xbfdc6c7f4997000c, 0xbfdc1249d8011ee5, 0xbfdbb7cf2304bd03,
    0xbfdb5d1009e15cbf, 0xbfdb020d6c7f400c, 0xbfdaa6c82b6d3fc9, 0xbfda4b4127dea1e8,
    0xbfd9ef7943a8ed8b, 0xbfd993716141be03, 0xbfd9372a63bc93d8, 0xbfd8daa52ec8a4b5,
    0xbfd87de2a6aea965, 0xbfd820e3b04eaac2, 0xbfd7c3a9311dccea, 0xbfd766340f2418f5,
    0xbfd7088530fa45a2, 0xbfd6aa9d7dc77e17, 0xbfd64c7ddd3f27ca, 0xbfd5ee27379ea694,
    0xbfd58f9a75ab1fe2, 0xbfd530d880af3c25, 0xbfd4d1e24278e770, 0xbfd472b8a5571056,
    0xbfd4135c94176600, 0xbfd3b3cefa0414ba, 0xbfd35410c2e18151, 0xbfd2f422daec038a,
    0xbfd294062ed59f06, 0xbfd233bbabc3bb76, 0xbfd1d3443f4cdb3f, 0xbfd172a0d776517c,
    0xbfd111d262b1f679, 0xbfd0b0d9cfdbdb96, 0xbfd04fb80e37fdb0, 0xbfcfdcdc1adfedf6,
    0xbfcf19f97b215f21, 0xbfce56ca1e101a1a, 0xbfcd934fe5454319, 0xbfcccf8cb312b286,
    0xbfcc0b826a7e4f6c, 0xbfcb4732ef3d6724, 0xbfca82a025b0045b, 0xbfc9bdcbf2dc436a,
    0xbfc8f8b83c69a617, 0xbfc83366e89c64cb, 0xbfc76dd9de50bf2f, 0xbfc6a81304f64ab9,
    0xbfc5e214448b3fc6, 0xbfc51bdf8597c5fa, 0xbfc45576b1293e5b, 0xbfc38edbb0cd8d1d,
    0xbfc2c8106e8e613c, 0xbfc20116d4ec7bda, 0xbfc139f0cedaf57a, 0xbfc072a047ba831a,
    0xbfbf564e56a97319, 0xbfbdc70ecbae9fc5, 0xbfbc3785c79ec2e2, 0xbfbaa7b724495c03,
    0xbfb917a6bc29b43c, 0xbfb787586a5d5b23, 0xbfb5f6d00a9aa42c, 0xbfb466117927209b,
    0xbfb2d52092ce1a0c, 0xbfb1440134d709bb, 0xbfaf656e79f820d3, 0xbfac428d12c0d7f9,
    0xbfa91f65f10dd80d, 0xbfa5fc00d290cd60, 0xbfa2d865759455cd, 0xbf9f693731d1cf46,
    0xbf992155f7a36689, 0xbf92d936bbe30f4e, 0xbf8921d1fcdec7b3, 0xbf7921f0fe6701e6,
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    /// Regenerate all three tables in `f64`, exactly as the committed constants were
    /// produced.
    fn generate() -> (Vec<f64>, Vec<f64>, Vec<f64>) {
        let hann: Vec<f64> = (0..STFT_SIZE)
            .map(|index| 0.5 - 0.5 * (core::f64::consts::TAU * index as f64 / STFT_SIZE as f64).cos())
            .collect();
        let mut twiddle_real = Vec::with_capacity(STFT_SIZE / 2);
        let mut twiddle_imaginary = Vec::with_capacity(STFT_SIZE / 2);
        for index in 0..STFT_SIZE / 2 {
            let angle = -core::f64::consts::TAU * index as f64 / STFT_SIZE as f64;
            twiddle_real.push(angle.cos());
            twiddle_imaginary.push(angle.sin());
        }
        (hann, twiddle_real, twiddle_imaginary)
    }

    /// The regeneration gate. If this fails, either a table was edited by hand or the
    /// generator changed — and the second moves every `sbr`-enhanced module's hash.
    #[test]
    fn the_committed_tables_are_what_the_generator_produces() {
        let (hann, twiddle_real, twiddle_imaginary) = generate();
        for (index, expected) in hann.iter().enumerate() {
            assert_eq!(expected.to_bits(), HANN_WINDOW_BITS[index], "Hann weight {index} of the committed table is not what the generator produces");
        }
        for (index, expected) in twiddle_real.iter().enumerate() {
            assert_eq!(expected.to_bits(), TWIDDLE_REAL_BITS[index], "twiddle {index}'s real part is not what the generator produces");
        }
        for (index, expected) in twiddle_imaginary.iter().enumerate() {
            assert_eq!(expected.to_bits(), TWIDDLE_IMAGINARY_BITS[index], "twiddle {index}'s imaginary part is not what the generator produces");
        }
    }

    /// A quarter-hop Hann-squared overlap sums to 1.5 wherever four frames meet, which is
    /// the constant `overlap_add_normalisation` explains the extender does *not* use.
    #[test]
    fn the_squared_window_sums_to_three_halves_at_a_quarter_hop() {
        for offset in 0..STFT_HOP {
            let sum: f64 = (0..STFT_SIZE / STFT_HOP).map(|frame| {
                let weight = window(offset + frame * STFT_HOP);
                weight * weight
            }).sum();
            assert!((sum - 1.5).abs() < 1.0e-12, "offset {offset} sums to {sum}");
        }
    }

    #[test]
    fn a_pure_tone_lands_in_one_bin() {
        let bin = 37usize;
        let mut real = [0.0f64; STFT_SIZE];
        let mut imaginary = [0.0f64; STFT_SIZE];
        for (index, value) in real.iter_mut().enumerate() {
            *value = (core::f64::consts::TAU * bin as f64 * index as f64 / STFT_SIZE as f64).cos();
        }
        forward(&mut real, &mut imaginary);
        for index in 0..STFT_SIZE {
            let magnitude = std::primitive::f64::sqrt(real[index] * real[index] + imaginary[index] * imaginary[index]);
            match index == bin || index == STFT_SIZE - bin {
                true => assert!((magnitude - STFT_SIZE as f64 / 2.0).abs() < 1.0e-8, "bin {index} carries {magnitude}"),
                false => assert!(magnitude < 1.0e-8, "bin {index} should be empty, carries {magnitude}"),
            }
        }
    }

    #[test]
    fn the_inverse_undoes_the_forward_transform() {
        let mut real = [0.0f64; STFT_SIZE];
        let mut imaginary = [0.0f64; STFT_SIZE];
        for (index, value) in real.iter_mut().enumerate() {
            let position = index as f64;
            *value = (0.31 * position).sin() + 0.5 * (1.7 * position + 0.4).cos();
        }
        let original = real;
        forward(&mut real, &mut imaginary);
        inverse(&mut real, &mut imaginary);
        for index in 0..STFT_SIZE {
            assert!((real[index] - original[index]).abs() < 1.0e-11, "frame {index}: {} against {}", real[index], original[index]);
            assert!(imaginary[index].abs() < 1.0e-11, "frame {index} grew an imaginary part");
        }
    }

    /// Emit the committed constants. Ignored by default; run it with
    /// `cargo test -p starplayer-enhance -- --ignored --nocapture emit_tables` and paste
    /// the output over the three constants above after changing the generator.
    #[test]
    #[ignore = "a generator, not a check"]
    fn emit_tables() {
        let (hann, twiddle_real, twiddle_imaginary) = generate();
        let emit = |name: &str, length: &str, values: &[f64]| {
            std::println!("pub const {name}: [u64; {length}] = [");
            for chunk in values.chunks(4) {
                let cells: Vec<std::string::String> = chunk.iter().map(|value| std::format!("0x{:016x}", value.to_bits())).collect();
                std::println!("    {},", cells.join(", "));
            }
            std::println!("];");
        };
        emit("HANN_WINDOW_BITS", "STFT_SIZE", &hann);
        emit("TWIDDLE_REAL_BITS", "STFT_SIZE / 2", &twiddle_real);
        emit("TWIDDLE_IMAGINARY_BITS", "STFT_SIZE / 2", &twiddle_imaginary);
    }
}
