use std::fs::File;
use std::io::{self, BufReader, Read};
use std::path::Path;

use anyhow::Result;

/// Unified input reader that handles both file and pipe input with buffered reading
pub struct InputReader {
    reader: Box<dyn Read>,
    is_pipe: bool,
}

impl InputReader {
    /// Create a new InputReader from a path
    /// Use "-" for stdin pipe input
    pub fn new<P: AsRef<Path>>(input_path: P) -> Result<Self> {
        let path_str = input_path.as_ref().to_string_lossy();
        let is_pipe = path_str == "-";

        let reader: Box<dyn Read> = if is_pipe {
            Box::new(io::stdin().lock())
        } else {
            let file = File::open(input_path)?;
            Box::new(BufReader::new(file))
        };

        Ok(Self { reader, is_pipe })
    }

    /// Read a chunk of data into the provided buffer
    /// Returns the number of bytes read, 0 indicates EOF
    pub fn read_chunk(&mut self, buffer: &mut [u8]) -> Result<usize> {
        let bytes_read = self.reader.read(buffer)?;
        Ok(bytes_read)
    }

    /// Check if this is pipe input
    pub fn is_pipe(&self) -> bool {
        self.is_pipe
    }

    /// Read all remaining data for non-streaming use cases
    /// Note: This should only be used for small files or when you need all data at once
    pub fn read_all(&mut self) -> Result<Vec<u8>> {
        let mut data = Vec::new();
        self.reader.read_to_end(&mut data)?;
        Ok(data)
    }

    /// Process data in chunks using a callback function
    /// The callback receives each chunk and should return Ok(true) to continue or Ok(false) to stop
    pub fn process_chunks<F>(&mut self, chunk_size: usize, mut callback: F) -> Result<()>
    where
        F: FnMut(&[u8]) -> Result<bool>,
    {
        let mut buffer = vec![0u8; chunk_size];

        loop {
            let bytes_read = self.read_chunk(&mut buffer)?;
            if bytes_read == 0 {
                break; // EOF
            }

            if !callback(&buffer[..bytes_read])? {
                break; // Callback requested stop
            }
        }

        Ok(())
    }
}
