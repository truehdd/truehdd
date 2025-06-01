use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// A thread-safe buffer pool for zero-copy frame processing.
///
/// Maintains a pool of reusable byte buffers to minimize allocations
/// during frame extraction and processing.
#[derive(Debug)]
pub struct BufferPool {
    pool: Arc<Mutex<VecDeque<Vec<u8>>>>,
    max_size: usize,
    buffer_capacity: usize,
}

impl BufferPool {
    /// Creates a new buffer pool with the specified parameters.
    ///
    /// # Arguments
    ///
    /// * `max_size` - Maximum number of buffers to keep in the pool
    /// * `buffer_capacity` - Initial capacity for each buffer
    pub fn new(max_size: usize, buffer_capacity: usize) -> Self {
        Self {
            pool: Arc::new(Mutex::new(VecDeque::with_capacity(max_size))),
            max_size,
            buffer_capacity,
        }
    }

    /// Acquires a buffer from the pool or creates a new one if none available.
    pub fn acquire(&self) -> Vec<u8> {
        let mut pool = self.pool.lock().unwrap();
        pool.pop_front()
            .unwrap_or_else(|| Vec::with_capacity(self.buffer_capacity))
    }

    /// Returns a buffer to the pool for reuse.
    pub fn release(&self, mut buffer: Vec<u8>) {
        buffer.clear();

        let mut pool = self.pool.lock().unwrap();
        if pool.len() < self.max_size {
            pool.push_back(buffer);
        }
    }
}

impl Default for BufferPool {
    fn default() -> Self {
        Self::new(16, 64 * 1024)
    }
}
