//! Dithering utilities for audio processing.
//!
//! Provides dither generation functions and lookup tables used in
//! lossless matrix operations for noise shaping.

/// Dither lookup table for format processing.
#[rustfmt::skip]
pub const DITHER_LUT: [i32; 256] = [
    30,  51,  22,  54,   3,   7,  -4,  38,  14,  55,  46,  81,  22,  58,  -3,   2,
    52,  31,  -7,  51,  15,  44,  74,  30,  85, -17,  10,  33,  18,  80,  28,  62,
    10,  32,  23,  69,  72,  26,  35,  17,  73,  60,   8,  56,   2,   6,  -2,  -5,
    51,   4,  11,  50,  66,  76,  21,  44,  33,  47,   1,  26,  64,  48,  57,  40,
    38,  16, -10, -28,  92,  22, -18,  29, -10,   5, -13,  49,  19,  24,  70,  34,
    61,  48,  30,  14,  -6,  25,  58,  33,  42,  60,  67,  17,  54,  17,  22,  30,
    67,  44,  -9,  50, -11,  43,  40,  32,  59,  82,  13,  49, -14,  55,  60,  36,
    48,  49,  31,  47,  15,  12,   4,  65,   1,  23,  29,  39,  45,  -2,  84,  69,
     0,  72,  37,  57,  27,  41, -15, -16,  35,  31,  14,  61,  24,   0,  27,  24,
    16,  41,  55,  34,  53,   9,  56,  12,  25,  29,  53,   5,  20, -20,  -8,  20,
    13,  28,  -3,  78,  38,  16,  11,  62,  46,  29,  21,  24,  46,  65,  43, -23,
    89,  18,  74,  21,  38, -12,  19,  12, -19,   8,  15,  33,   4,  57,   9,  -8,
    36,  35,  26,  28,   7,  83,  63,  79,  75,  11,   3,  87,  37,  47,  34,  40,
    39,  19,  20,  42,  27,  34,  39,  77,  13,  42,  59,  64,  45,  -1,  32,  37,
    45,  -5,  53,  -6,   7,  36,  50,  23,   6,  32,   9, -21,  18,  71,  27,  52,
   -25,  31,  35,  42,  -1,  68,  63,  52,  26,  43,  66,  37,  41,  25,  40,  70,
];

/// Generates dither table for 31EB substream format.
///
/// Creates a power-of-two sized dither table using the specified seed
/// and TrueHD's pseudo-random number generation algorithm.
pub fn dither_31eb(samples_per_au: usize, dither_seed: &mut u32) -> Vec<i32> {
    let samples_per_au = samples_per_au.next_power_of_two();
    let mut dither_table = Vec::with_capacity(samples_per_au);

    for _ in 0..samples_per_au {
        let dither_seed_shr15 = *dither_seed >> 15;
        dither_table.push(DITHER_LUT[dither_seed_shr15 as usize]);
        *dither_seed =
            ((*dither_seed << 8) ^ dither_seed_shr15 ^ (dither_seed_shr15 << 5)) & 0x7FFFFF;
    }

    dither_table
}
