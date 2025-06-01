//! Bitstream I/O utilities for audio parsing.
//!
//! Provides bitstream reading, Huffman decoding, CRC validation,
//! and specialized bit manipulation functions for format parsing.

use std::io;
use std::io::SeekFrom;

use bitstream_io::{
    BigEndian, BitRead, BitReader, SignedInteger, UnsignedInteger, define_huffman_tree,
};

use crate::utils::crc::{Crc8, Crc16, crc8, crc16};

const STACK_BUF_SIZE: usize = 256;

define_huffman_tree!(HuffTree1 : i32 = [
        [
            [[[[[[[(-7), (-7)], (-6)], (-5)], (-4)], (-3)], (-2)], (-1)],
            [[[[[[[ 10 ,  10 ],   9 ],   8 ],   7 ],   6 ],   5 ],   4 ]
        ],
        [
            [0, 1], [2, 3]
        ]
    ]
);

define_huffman_tree!(HuffTree2 : i32 = [
        [
            [[[[[[[(-7), (-7)], (-6)], (-5)], (-4)], (-3)], (-2)], (-1)],
            [[[[[[[  8 ,   8 ],   7 ],   6 ],   5 ],   4 ],   3 ],   2 ]
        ],
        [0, 1]
    ]
);

define_huffman_tree!(HuffTree3 : i32 = [
        [
            [[[[[[[(-7), (-7)], (-6)], (-5)], (-4)], (-3)], (-2)], (-1)],
            [[[[[[[  7 ,   7 ],   6 ],   5 ],   4 ],   3 ],   2 ],   1 ]
        ],
        0
    ]
);

#[derive(Debug)]
pub struct BitstreamIoReader<R: io::Read + io::Seek> {
    bs: BitReader<R, BigEndian>,
    len: u64,
}

pub type BsIoSliceReader<'a> = BitstreamIoReader<io::Cursor<&'a [u8]>>;

impl<R> BitstreamIoReader<R>
where
    R: io::Read + io::Seek,
{
    pub fn new(read: R, len_bytes: u64) -> Self {
        Self {
            bs: BitReader::new(read),
            len: len_bytes << 3,
        }
    }

    #[inline(always)]
    pub fn get(&mut self) -> io::Result<bool> {
        self.bs.read_bit()
    }

    #[inline(always)]
    pub fn get_n<I: UnsignedInteger>(&mut self, n: u32) -> io::Result<I> {
        // Skip bounds check for small reads - bitstream_io handles EOF internally
        if n <= 32 {
            match self.bs.read_unsigned_var(n) {
                Ok(val) => Ok(val),
                Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                    // Only call position() on error path to avoid overhead
                    Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        format!(
                            "get_n({}): out of bounds bits at {}",
                            n,
                            self.bs.position_in_bits().unwrap_or(0)
                        ),
                    ))
                }
                Err(e) => Err(e),
            }
        } else {
            // For larger reads, keep bounds check
            self.available().and_then(|avail| {
                if n as u64 > avail {
                    Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        format!(
                            "get_n({}): out of bounds bits at {}",
                            n,
                            self.bs.position_in_bits().unwrap_or(0)
                        ),
                    ))
                } else {
                    self.bs.read_unsigned_var(n)
                }
            })
        }
    }

    #[inline(always)]
    pub fn get_s<S: SignedInteger>(&mut self, n: u32) -> io::Result<S> {
        // Skip bounds check for small reads - bitstream_io handles EOF internally
        if n <= 32 {
            match self.bs.read_signed_var(n) {
                Ok(val) => Ok(val),
                Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                    // Only call position() on error path to avoid overhead
                    Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        format!(
                            "get_s({}): out of bounds bits at {}",
                            n,
                            self.bs.position_in_bits().unwrap_or(0)
                        ),
                    ))
                }
                Err(e) => Err(e),
            }
        } else {
            // For larger reads, keep bounds check
            self.available().and_then(|avail| {
                if n as u64 > avail {
                    Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        format!(
                            "get_s({}): out of bounds bits at {}",
                            n,
                            self.bs.position_in_bits().unwrap_or(0)
                        ),
                    ))
                } else {
                    self.bs.read_signed_var(n)
                }
            })
        }
    }

    #[inline(always)]
    pub fn get_huffman(&mut self, huff_type: usize) -> io::Result<i32> {
        match huff_type {
            1 => self.bs.read_huffman::<HuffTree1>(),
            2 => self.bs.read_huffman::<HuffTree2>(),
            3 => self.bs.read_huffman::<HuffTree3>(),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "get_huffman: unsupported huff_type",
            )),
        }
    }

    #[inline(always)]
    pub fn get_variable_bits_max(&mut self, n: u32, max_num_groups: u32) -> io::Result<u32> {
        let mut value = 0;
        let mut num_group = 0;
        let mut b_read_more = true;

        while b_read_more && num_group < max_num_groups {
            let read = self.get_n::<u32>(n)?;
            value += read;
            b_read_more = self.get()?;
            if b_read_more {
                value = (value + 1) << n;
                num_group += 1;
            }
        }

        Ok(value)
    }

    #[inline(always)]
    pub fn seek(&mut self, offset: i64) -> io::Result<u64> {
        if (offset < 0 && self.position()? as i64 + offset >= 0)
            || (offset >= 0 && self.available()? as i64 >= offset)
        {
            return self.bs.seek_bits(SeekFrom::Current(offset));
        }

        Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            format!(
                "seek({}): out of bounds bits at {}",
                offset,
                self.position()?
            ),
        ))
    }

    #[inline(always)]
    // TODO: byte boundary
    pub fn parity_check_for_last_n_bits(&mut self, len: u64) -> io::Result<u8> {
        let position = self.position()?;

        self.seek(-(len as i64))?;

        let bytes_len = (len >> 3) as usize;

        let parity = if bytes_len <= STACK_BUF_SIZE {
            let mut stack_buf = [0u8; STACK_BUF_SIZE];
            let buf = &mut stack_buf[..bytes_len];
            self.bs.read_bytes(buf)?;
            buf.iter().fold(0, |acc, x| acc ^ x)
        } else {
            let mut heap_buf = vec![0; bytes_len];
            self.bs.read_bytes(&mut heap_buf)?;
            heap_buf.iter().fold(0, |acc, x| acc ^ x)
        };

        self.bs.seek_bits(SeekFrom::Start(position))?;

        Ok(parity)
    }

    #[inline(always)]
    pub fn parity_check_nibble_for_last_n_bits(&mut self, len: u64) -> io::Result<u8> {
        let mut parity = self.parity_check_for_last_n_bits(len)?;

        parity ^= parity >> 4;
        parity &= 0xF;

        Ok(parity)
    }

    #[inline(always)]
    pub fn crc8_check(&mut self, crc: &Crc8, start: u64, len: u64) -> io::Result<u8> {
        let position = self.position()?;

        if start + len > self.len {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "crc8_check: out of bounds bits",
            ));
        }

        self.bs.seek_bits(SeekFrom::Start(start))?;

        let mut checksum = crc.init;

        let prefix_len = start & 7;
        let suffix_len = (len - prefix_len) & 7;
        let middle_len = (len - prefix_len - suffix_len) as usize;

        if prefix_len != 0 {
            let prefix: u8 = self.bs.read_var(prefix_len as u32)?;
            checksum = crc8(crc.poly, checksum, prefix_len as usize) ^ prefix;
        }

        let bytes_len = middle_len >> 3;
        if bytes_len <= STACK_BUF_SIZE {
            let mut stack_buf = [0u8; STACK_BUF_SIZE];
            let buf = &mut stack_buf[..bytes_len];
            self.bs.read_bytes(buf)?;
            checksum = crc.update(checksum, buf);
        } else {
            let mut heap_buf = vec![0; bytes_len];
            self.bs.read_bytes(&mut heap_buf)?;
            checksum = crc.update(checksum, &heap_buf);
        };

        if suffix_len != 0 {
            let suffix: u8 = self.bs.read_var(suffix_len as u32)?;
            checksum = crc8(crc.poly, checksum, suffix_len as usize) ^ suffix;
        }

        self.bs.seek_bits(SeekFrom::Start(position))?;

        Ok(checksum)
    }

    #[inline(always)]
    pub fn crc16_check(&mut self, crc: &Crc16, start: u64, len: u64) -> io::Result<u16> {
        let position = self.position()?;

        if start + len > self.len {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "crc8_check: out of bounds bits",
            ));
        }

        self.bs.seek_bits(SeekFrom::Start(start))?;

        let mut checksum = crc.init;

        let prefix_len = start & 7;
        let suffix_len = (len - prefix_len) & 7;
        let middle_len = (len - prefix_len - suffix_len) as usize;

        if prefix_len != 0 {
            let prefix: u16 = self.bs.read_var(prefix_len as u32)?;
            checksum = crc16(crc.poly, checksum, prefix_len as usize) ^ prefix;
        }

        let bytes_len = middle_len >> 3;
        if bytes_len <= STACK_BUF_SIZE {
            let mut stack_buf = [0u8; STACK_BUF_SIZE];
            let buf = &mut stack_buf[..bytes_len];
            self.bs.read_bytes(buf)?;
            checksum = crc.update(checksum, buf);
        } else {
            let mut heap_buf = vec![0; bytes_len];
            self.bs.read_bytes(&mut heap_buf)?;
            checksum = crc.update(checksum, &heap_buf);
        };

        if suffix_len != 0 {
            let suffix: u16 = self.bs.read_var(suffix_len as u32)?;
            checksum = crc16(crc.poly, checksum, suffix_len as usize) ^ suffix;
        }

        self.bs.seek_bits(SeekFrom::Start(position))?;

        Ok(checksum)
    }

    #[inline(always)]
    pub fn align_16bit(&mut self) -> io::Result<()> {
        self.bs.byte_align();

        let position = self.bs.position_in_bits()?;
        if position & 15 > 0 {
            self.skip_n(8)?;
        }

        Ok(())
    }

    #[inline(always)]
    pub fn available(&mut self) -> io::Result<u64> {
        self.bs.position_in_bits().map(|pos| self.len - pos)
    }

    #[inline(always)]
    pub fn skip_n(&mut self, n: u32) -> io::Result<()> {
        // Skip bounds check for small skips - bitstream_io handles EOF internally
        if n <= 64 {
            self.bs.skip(n)
        } else {
            // For larger skips, keep bounds check
            self.available().and_then(|avail| {
                if n as u64 > avail {
                    Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "skip_n: out of bounds bits",
                    ))
                } else {
                    self.bs.skip(n)
                }
            })
        }
    }

    #[inline(always)]
    pub fn position(&mut self) -> io::Result<u64> {
        self.bs.position_in_bits()
    }
}

impl<'a> BsIoSliceReader<'a> {
    pub fn from_slice(buf: &'a [u8]) -> Self {
        let len = buf.len() as u64;
        let read = io::Cursor::new(buf);

        Self::new(read, len)
    }
}

impl Default for BsIoSliceReader<'_> {
    fn default() -> Self {
        Self::from_slice(&[])
    }
}
