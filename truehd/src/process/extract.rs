use crate::log_or_err;
use crate::structs::timestamp::Timestamp;
use crate::utils::buffer_pool::BufferPool;
use crate::utils::crc::{CRC_MAJOR_SYNC_INFO_ALG, Crc16};
use crate::utils::errors::ExtractError;
use anyhow::Result;
use log::error;
use std::sync::Arc;

/// Extracts audio frames from a continuous bitstream.
///
/// Frame boundary detection by searching for major sync patterns.
/// Implements low-level frame extraction for bitstreams.
///
/// # Example
///
/// ```rust,no_run
/// use truehd::process::EXAMPLE_DATA;
/// use truehd::process::extract::Extractor;
///
/// let mut extractor = Extractor::default();
/// let data = EXAMPLE_DATA; // Example data
///
/// // Push bitstream data
/// extractor.push_bytes(data);
///
/// // Extract frames
/// for frame in extractor {
///     let frame = frame.unwrap();
///     println!("Extracted frame with {} bytes", frame.as_ref().len());
///     
///     if frame.is_major_sync() {
///         println!("Found major sync frame");
///     }
/// }
/// ```
/// # Performance Considerations
///
/// - Uses a ring buffer with 120KB capacity for efficient processing
/// - Implements streaming extraction to handle large files
/// - CRC validation ensures frame integrity
#[derive(Debug)]
pub struct Extractor {
    buffer: Vec<u8>,
    cursor: usize,
    timestamp: Option<Timestamp>,
    inited: bool,
    locked: bool,
    io_counter: usize,
    substreams: usize,
    crc: Crc16,
    buffer_pool: BufferPool,
    error_count: usize,
    frames_processed: usize,
    fail_level: log::Level,
    consumed: u64,
}

impl Default for Extractor {
    fn default() -> Self {
        Self {
            buffer: Vec::with_capacity(120_000),
            cursor: 0,
            timestamp: None,
            inited: false,
            locked: false,
            io_counter: 0,
            substreams: 0,
            crc: Crc16::new(&CRC_MAJOR_SYNC_INFO_ALG),
            buffer_pool: BufferPool::default(),
            error_count: 0,
            frames_processed: 0,
            fail_level: log::Level::Error,
            consumed: 0,
        }
    }
}

impl crate::utils::diagnostic::DiagnosticSink for Extractor {
    fn fail_level(&self) -> log::Level {
        self.fail_level
    }
}

impl Extractor {
    /// Adds raw bitstream data to the internal buffer.
    ///
    /// This method feeds data to the extractor's internal ring buffer. The extractor
    /// will automatically process this data during frame extraction, searching for
    /// sync patterns and frame boundaries.
    ///
    /// # Arguments
    ///
    /// * `data` - Raw bitstream bytes to add to the buffer
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use truehd::process::extract::Extractor;
    ///
    /// let mut extractor = Extractor::default();
    ///
    /// // Read data from file and push to extractor
    /// let file_data = std::fs::read("stream.thd")?;
    /// extractor.push_bytes(&file_data);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn push_bytes(&mut self, data: &[u8]) {
        self.compact_if_needed(data.len());
        self.buffer.extend(data);
        self.io_counter += 1;
    }

    #[inline]
    fn buffered_len(&self) -> usize {
        self.buffer.len().saturating_sub(self.cursor)
    }

    #[inline]
    fn buffered(&self) -> &[u8] {
        &self.buffer[self.cursor..]
    }

    fn compact_if_needed(&mut self, incoming_len: usize) {
        if self.cursor == 0 {
            return;
        }

        if self.cursor == self.buffer.len() {
            self.buffer.clear();
            self.cursor = 0;
            return;
        }

        let needs_room = self.buffer.len() + incoming_len > self.buffer.capacity();
        let cursor_is_large = self.cursor >= 64 * 1024;
        let mostly_consumed = self.cursor * 2 >= self.buffer.len();
        if !(needs_room || cursor_is_large || mostly_consumed) {
            return;
        }

        self.buffer.copy_within(self.cursor.., 0);
        self.buffer.truncate(self.buffer.len() - self.cursor);
        self.cursor = 0;
    }

    /// Forces the extractor to search for the next sync pattern.
    ///
    /// This method clears the current sync lock and searches for the next major sync
    /// pattern in the buffer. It's useful for recovering from corrupted data or
    /// manually seeking to the next valid frame boundary.
    ///
    /// # Returns
    ///
    /// Returns a [`Timestamp`] if a valid sync pattern with embedded timestamp
    /// is found, or `None` if no sync pattern is located.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use truehd::process::EXAMPLE_DATA;
    /// use truehd::process::extract::Extractor;
    ///
    /// let mut extractor = Extractor::default();
    ///
    /// let mut corrupted_data = Vec::new();
    /// corrupted_data.extend_from_slice(EXAMPLE_DATA); // Should have 2 frames
    /// corrupted_data.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]); // Example corrupted data
    /// corrupted_data.extend_from_slice(EXAMPLE_DATA);
    ///
    /// extractor.push_bytes(&corrupted_data);
    ///
    /// // Force resync after corruption
    ///
    /// let frame_count = extractor.filter(|frame|frame.is_ok()).count();
    /// assert_eq!(frame_count, 4);
    /// ```
    fn resync(&mut self) -> Result<(), ExtractError> {
        self.locked = false;

        loop {
            let trailing_bytes = if self.inited { 4 } else { 16 };
            let search_range = self.buffered_len().saturating_sub(trailing_bytes);
            if search_range < 4 {
                return self.insufficient();
            }

            let mut offset = 0;
            let mut state = 0;
            let buffer = self.buffered();
            for (i, &byte) in buffer[4..search_range].iter().enumerate() {
                match (state, byte) {
                    (_, 0xF8) => {
                        state = 1;
                        offset = i;
                    }
                    (1, 0x72) => state = 2,
                    (2, 0x6F) => state = 3,
                    (3, 0xBA) | (3, 0xBB) => {
                        state = 4;
                        break;
                    }
                    _ => state = 0,
                }
            }

            if state != 4 {
                self.consume_front(search_range);
                return self.insufficient();
            }

            // Try only once
            self.timestamp = if !self.inited && offset >= 16 {
                self.consume_front(offset - 16);
                let timestamp = Timestamp::from_bytes(&self.buffered()[..16]).ok();
                self.consume_front(16);
                timestamp
            } else {
                self.consume_front(offset);
                None
            };

            // Now frame candidate is at offset 0
            self.inited = true;

            let Some(major_sync_info_len) = self.major_sync_info_len() else {
                return self.insufficient();
            };

            if self.buffered_len() < 4 + major_sync_info_len {
                return self.insufficient();
            };

            let Some(access_unit_len) = self.access_unit_len() else {
                return self.insufficient();
            };

            if self.buffered_len() < access_unit_len || access_unit_len <= major_sync_info_len + 6 {
                return self.insufficient();
            }

            let access_unit_bytes = &self.buffered()[..access_unit_len];
            let crc_bytes = &(&access_unit_bytes[4 + major_sync_info_len..])[..2];
            let crc = u16::from_be_bytes([crc_bytes[0], crc_bytes[1]]);
            if crc != self.crc16_major_sync_info(&(&access_unit_bytes[4..])[..major_sync_info_len])
            {
                self.consume_front(access_unit_len);
                log_or_err!(self, log::Level::Error, ExtractError::ParityCheckFailed);
                continue;
            }

            self.locked = true;
            self.substreams = (self.buffered()[20] >> 4) as usize;

            return Ok(());
        }
    }

    pub fn timestamp(&self) -> Option<Timestamp> {
        self.timestamp.clone()
    }

    /// Number of corrupt-frame events encountered so far.
    ///
    /// Incremented each time a frame candidate fails validation and the
    /// extractor has to resync. Lets callers detect silently skipped
    /// corruption in otherwise successful extractions.
    pub fn error_count(&self) -> usize {
        self.error_count
    }

    fn consume_front(&mut self, cnt: usize) {
        self.cursor = self.cursor.saturating_add(cnt);
        self.consumed = self.consumed.saturating_add(cnt as u64);
        self.compact_if_needed(0);
    }

    /// Byte offset of the next unconsumed byte, counted from the first byte pushed.
    pub fn stream_position(&self) -> u64 {
        self.consumed
    }

    fn access_unit_len(&self) -> Option<usize> {
        let buffer = self.buffered();
        Some(((u16::from_be_bytes([*buffer.first()?, *buffer.get(1)?]) & 0xFFF) << 1) as usize)
    }

    fn major_sync_info_len(&self) -> Option<usize> {
        let buffer = self.buffered();

        // The extra channel meaning block that extends the major sync info is FBA-only, so
        // an FBB major sync (format_sync byte 0xBB) is always the 26-byte base regardless
        // of the bit at buffer[29]. Using the FBA extension length here would put the CRC
        // past its real position and the frame would be rejected.
        if *buffer.get(7)? == 0xBB {
            return Some(26);
        }

        let len = if buffer.get(29)? & 0x01 == 0 {
            26
        } else {
            28 + ((buffer.get(30)? >> 3) & 0x1Eu8) as usize
        };

        Some(len)
    }

    fn insufficient(&mut self) -> Result<(), ExtractError> {
        self.io_counter -= 1;
        Err(ExtractError::InsufficientData)
    }

    fn iter_insufficient(&mut self) -> Option<Result<Frame, ExtractError>> {
        self.io_counter -= 1;
        Some(Err(ExtractError::InsufficientData))
    }

    #[inline(always)]
    const fn crc16_major_sync_info(&self, data: &[u8]) -> u16 {
        self.crc.update(self.crc.init, data)
    }
}

impl Iterator for Extractor {
    type Item = Result<Frame, ExtractError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.io_counter == 0 {
            return None;
        }

        loop {
            'locked: {
                if !self.locked && self.resync().is_err() {
                    return None;
                }

                if self.buffered_len() < 6 {
                    return self.iter_insufficient();
                };

                let buffer = self.buffered();
                let mut offset = 0;
                let mut pre = 4;
                let mut skip = if buffer[4] == 0xF8 && buffer[5] == 0x72 {
                    if self.buffered_len() < 21 {
                        return self.iter_insufficient();
                    };

                    let substreams = buffer[20] as usize >> 4;
                    if self.substreams != substreams {
                        let error = ExtractError::SubstreamMismatch {
                            found: substreams,
                            expected: self.substreams,
                        };
                        error!("Substream count mismatch: {error}");

                        break 'locked;
                    }

                    let Some(major_sync_info_len) = self.major_sync_info_len() else {
                        return self.iter_insufficient();
                    };

                    major_sync_info_len + 2
                } else {
                    0
                };

                let mut post = 0;
                let mut substreams = self.substreams;
                let mut parity = 0;

                'outer: {
                    'inner: loop {
                        if pre > 0 {
                            pre -= 1;

                            let Some(&byte) = buffer.get(offset) else {
                                break 'inner;
                            };

                            parity ^= byte;
                            offset += 1;

                            continue;
                        }
                        if skip > 0 {
                            skip -= 1;
                            offset += 1;

                            continue;
                        }
                        if post > 0 {
                            post -= 1;

                            let Some(&byte) = buffer.get(offset) else {
                                break 'inner;
                            };

                            parity ^= byte;
                            offset += 1;

                            continue;
                        }
                        if substreams > 0 {
                            substreams -= 1;

                            let Some(&byte) = buffer.get(offset) else {
                                break 'inner;
                            };

                            post += 2 + if (byte >> 7) != 0 { 2 } else { 0 };

                            continue;
                        }

                        break 'outer;
                    }

                    return self.iter_insufficient();
                }

                if ((parity >> 4) ^ parity) & 0xF != 0xF {
                    let error = ExtractError::ParityCheckFailed;
                    error!("Frame parity check failed: {error}");

                    break 'locked;
                }

                let Some(access_unit_len) = self.access_unit_len() else {
                    return self.iter_insufficient();
                };

                if self.buffered_len() < access_unit_len {
                    return self.iter_insufficient();
                };

                // Use pooled buffer for zero-copy frame creation
                let mut frame_buffer = self.buffer_pool.acquire();
                frame_buffer.extend_from_slice(&self.buffered()[..access_unit_len]);
                let offset = self.consumed;
                self.consume_front(access_unit_len);

                let timestamp = if self.timestamp.is_some() {
                    let timestamp = self.timestamp.clone();
                    self.timestamp = None;

                    timestamp
                } else {
                    None
                };

                let frame = Frame {
                    timestamp,
                    data: frame_buffer.into(),
                    index: self.frames_processed as u64,
                    offset,
                };

                self.frames_processed += 1;
                return Some(Ok(frame));
            }

            if self.inited {
                self.error_count += 1;
                if self.buffered_len() > 0 {
                    self.consume_front(1);
                }
            }

            match self.resync() {
                Ok(_) => continue,
                Err(_) => return None,
            }
        }
    }
}

/// A single audio frame extracted from a bitstream.
///
/// # Data Access
///
/// Frame data can be accessed through the [`AsRef<[u8]>`] implementation:
///
/// ```rust,no_run
/// use truehd::process::extract::{Extractor, Frame};
///
/// fn process_frame(frame: &Frame) {
///     let raw_data: &[u8] = frame.as_ref();
///     println!("Frame size: {} bytes", raw_data.len());
///     
///     if frame.is_major_sync() {
///         println!("Major sync frame detected");
///     }
/// }
/// ```
///
/// Major sync frames are identified by the sync pattern `0xF872` at bytes 4-5.
#[derive(Debug, Clone)]
pub struct Frame {
    pub timestamp: Option<Timestamp>,
    pub data: Arc<[u8]>,

    /// Zero-based position of this access unit in the sequence the extractor emitted.
    pub index: u64,

    /// Byte offset of the access unit, counted from the first byte pushed into the
    /// extractor. Absolute within the stream only if the whole stream was pushed.
    pub offset: u64,
}

impl AsRef<[u8]> for Frame {
    fn as_ref(&self) -> &[u8] {
        &self.data
    }
}

impl Frame {
    /// Checks if this frame contains major sync information.
    ///
    /// Major sync frames occur periodically in streams and contain
    /// complete stream configuration including:
    /// - Sample rate and channel configuration  
    /// - Format information and presentation details
    /// - Channel meaning and DRC parameters
    /// - SMPTE timestamps (when present)
    ///
    /// # Returns
    ///
    /// `true` if this is a major sync frame, `false` for continuation frames.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use truehd::process::EXAMPLE_DATA;
    /// use truehd::process::extract::Extractor;
    ///
    /// let mut extractor = Extractor::default();
    /// let data = EXAMPLE_DATA; // Example data
    /// extractor.push_bytes(&data);
    ///
    /// for frame in extractor {
    ///     let frame = frame.unwrap();
    ///     if frame.is_major_sync() {
    ///         println!("Found stream configuration frame");
    ///         // Parse for format information
    ///     } else {
    ///         println!("Continuation frame with audio data");
    ///     }
    /// }
    /// ```
    pub fn is_major_sync(&self) -> bool {
        self.data[4] == 0xF8 && self.data[5] == 0x72
    }
}

#[test]
fn buf_extract() -> anyhow::Result<()> {
    use crate::process::EXAMPLE_DATA;
    let mut extractor = Extractor::default();

    // Generate deterministic "random" data for testing
    let mut test_buf = vec![0u8; 120_000];
    for (i, byte) in test_buf.iter_mut().enumerate() {
        *byte = ((i * 37 + 123) % 256) as u8; // Simple deterministic pattern
    }

    extractor.push_bytes(&test_buf);
    let _ = extractor.resync();
    assert!(!extractor.locked);

    extractor.push_bytes(&EXAMPLE_DATA[..42]);
    let _ = extractor.resync();
    assert!(!extractor.locked);

    extractor.push_bytes(&EXAMPLE_DATA[42..]);

    let frame = extractor.next().unwrap()?;
    assert_eq!(frame.as_ref().len(), 84);

    let frame = extractor.next().unwrap().unwrap();
    assert_eq!(frame.as_ref().len(), 20);
    Ok(())
}

/// The offset a frame carries must address that frame's first byte in the pushed stream.
#[test]
fn frames_carry_their_stream_position() {
    use crate::process::EXAMPLE_DATA;

    let mut data = Vec::new();
    data.extend_from_slice(EXAMPLE_DATA);
    data.extend_from_slice(EXAMPLE_DATA);

    let mut extractor = Extractor::default();
    extractor.push_bytes(&data);

    let frames: Vec<Frame> = extractor.by_ref().filter_map(|f| f.ok()).collect();
    assert_eq!(frames.len(), 4);

    for (i, frame) in frames.iter().enumerate() {
        assert_eq!(frame.index, i as u64);

        let start = frame.offset as usize;
        assert_eq!(&data[start..start + frame.data.len()], frame.as_ref());
    }
}

#[test]
fn skip_invalid_data() -> Result<()> {
    use crate::process::EXAMPLE_DATA;
    use crate::process::extract::Extractor;

    let mut extractor = Extractor::default();

    let mut corrupted_data = Vec::new();
    corrupted_data.extend_from_slice(EXAMPLE_DATA); // Should have 2 frames
    corrupted_data.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]); // Example corrupted data
    corrupted_data.extend_from_slice(EXAMPLE_DATA);
    corrupted_data.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]);

    extractor.push_bytes(&corrupted_data);

    let mut frame_count = 0;
    let mut end_with_insufficient_data = false;
    for result in &mut extractor {
        match result {
            Ok(_) => frame_count += 1,
            Err(e) => match e {
                ExtractError::InsufficientData => end_with_insufficient_data = true,
                _ => continue,
            },
        }
    }
    assert_eq!(frame_count, 4);
    assert!(end_with_insufficient_data);
    Ok(())
}
