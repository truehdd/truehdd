//! CRC validation utilities for audio bitstreams.
//!
//! Provides CRC-8 and CRC-16 implementations with format-specific algorithms
//! for validating block headers, substreams, and major sync information.
//!
//! Note: CRC calculations used in this module are specific to the TrueHD format
//! and are not standard CRC implementations.

/// CRC algorithm specification with polynomial and initial value.
pub struct Algorithm<T> {
    poly: T,
    init: T,
}

/// CRC-8 algorithm for restart block header validation.
pub const CRC_RESTART_BLOCK_HEADER_ALG: Algorithm<u8> = Algorithm {
    poly: 0x1d,
    init: 0x00,
};

/// CRC-8 algorithm for substream data validation.
pub const CRC_SUBSTREAM_ALG: Algorithm<u8> = Algorithm {
    poly: 0x63,
    init: 0xa2,
};

/// CRC-16 algorithm for major sync information validation.
pub const CRC_MAJOR_SYNC_INFO_ALG: Algorithm<u16> = Algorithm {
    poly: 0x2d,
    init: 0x00,
};

/// Computes CRC-8 checksum using specified polynomial.
#[inline(always)]
pub const fn crc8(poly: u8, mut value: u8, len: usize) -> u8 {
    let mut i = 0;
    while i < len {
        value = (value << 1) ^ (((value >> 7) & 1) * poly);
        i += 1;
    }

    value
}

/// Computes CRC-16 checksum using specified polynomial.
#[inline(always)]
pub const fn crc16(poly: u16, mut value: u16, len: usize) -> u16 {
    value <<= 8;

    let mut i = 0;
    while i < len {
        value = (value << 1) ^ (((value >> 15) & 1) * poly);
        i += 1;
    }

    value
}

/// `tables[k][x]` is `x` advanced through `k + 1` bytes of zeros. A byte step,
/// `crc = T(crc) ^ byte`, is linear, so eight steps are `T^8(crc)` XORed with each
/// byte advanced through the steps left after it: eight independent lookups
/// instead of a chain of eight.
#[inline(always)]
const fn crc8_tables(poly: u8) -> [[u8; 256]; 8] {
    let mut tables = [[0u8; 256]; 8];
    let mut i = 0;
    while i < 256 {
        tables[0][i] = crc8(poly, i as u8, 8);
        i += 1;
    }

    let mut k = 1;
    while k < 8 {
        let mut i = 0;
        while i < 256 {
            tables[k][i] = tables[0][tables[k - 1][i] as usize];
            i += 1;
        }
        k += 1;
    }

    tables
}

#[inline(always)]
const fn crc16_table(poly: u16) -> [u16; 256] {
    let mut table = [0u16; 256];
    let mut i = 0;
    while i < table.len() {
        table[i] = crc16(poly, i as u16, 8);
        i += 1;
    }

    table
}

#[derive(Debug)]
pub struct Crc8 {
    pub poly: u8,
    pub init: u8,
    tables: [[u8; 256]; 8],
}

#[derive(Debug)]
pub struct Crc16 {
    pub poly: u16,
    pub init: u16,
    table: [u16; 256],
}

impl Crc8 {
    pub const fn new(algorithm: &Algorithm<u8>) -> Self {
        Self {
            poly: algorithm.poly,
            init: algorithm.init,
            tables: crc8_tables(algorithm.poly),
        }
    }

    #[inline(always)]
    pub const fn update(&self, mut crc: u8, bytes: &[u8]) -> u8 {
        let t = &self.tables;
        let mut i = 0;

        while i + 8 <= bytes.len() {
            let b = bytes;
            // The bytes first and the CRC last, which is the order the compiler
            // keeps: the next step waits on this one's CRC, so whatever is
            // combined after its lookup is added to that wait.
            let stepped_bytes = t[6][b[i] as usize]
                ^ t[5][b[i + 1] as usize]
                ^ t[4][b[i + 2] as usize]
                ^ t[3][b[i + 3] as usize]
                ^ t[2][b[i + 4] as usize]
                ^ t[1][b[i + 5] as usize]
                ^ t[0][b[i + 6] as usize]
                ^ b[i + 7];
            crc = stepped_bytes ^ t[7][crc as usize];
            i += 8;
        }

        while i < bytes.len() {
            crc = t[0][crc as usize] ^ bytes[i];
            i += 1;
        }

        crc
    }
}

impl Crc16 {
    pub const fn new(algorithm: &Algorithm<u16>) -> Self {
        Self {
            poly: algorithm.poly,
            init: algorithm.init,
            table: crc16_table(algorithm.poly),
        }
    }

    const fn table_entry(&self, index: u16) -> u16 {
        self.table[(index & 0xFF) as usize]
    }

    #[inline(always)]
    pub const fn update(&self, mut crc: u16, bytes: &[u8]) -> u16 {
        let mut i = 0;

        while i < bytes.len() {
            crc = self.table_entry(crc >> 8) ^ (crc << 8) ^ bytes[i] as u16;
            i += 1;
        }

        crc
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The byte-at-a-time definition the tables must agree with.
    fn crc8_bytewise(poly: u8, mut crc: u8, bytes: &[u8]) -> u8 {
        for &byte in bytes {
            crc = crc8(poly, crc, 8) ^ byte;
        }
        crc
    }

    #[test]
    fn crc8_update_matches_the_bytewise_definition() {
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let data: Vec<u8> = (0..300)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                seed as u8
            })
            .collect();

        for algorithm in [&CRC_RESTART_BLOCK_HEADER_ALG, &CRC_SUBSTREAM_ALG] {
            let crc = Crc8::new(algorithm);
            for len in 0..data.len() {
                for init in [algorithm.init, 0x00, 0xff, 0x5a] {
                    assert_eq!(
                        crc.update(init, &data[..len]),
                        crc8_bytewise(algorithm.poly, init, &data[..len]),
                        "poly {:#04x}, init {init:#04x}, {len} bytes",
                        algorithm.poly
                    );
                }
            }
        }
    }
}
