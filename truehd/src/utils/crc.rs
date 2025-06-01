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

#[inline(always)]
const fn crc8_table(poly: u8) -> [u8; 256] {
    let mut table = [0u8; 256];
    let mut i = 0;
    while i < table.len() {
        table[i] = crc8(poly, i as u8, 8);
        i += 1;
    }

    table
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
    table: [u8; 256],
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
            table: crc8_table(algorithm.poly),
        }
    }

    const fn table_entry(&self, index: u8) -> u8 {
        self.table[index as usize]
    }

    #[inline(always)]
    pub const fn update(&self, mut crc: u8, bytes: &[u8]) -> u8 {
        let mut i = 0;

        while i < bytes.len() {
            crc = self.table_entry(crc) ^ bytes[i];
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
