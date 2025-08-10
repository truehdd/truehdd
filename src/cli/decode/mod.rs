pub mod atmos;
mod decode_impl;
pub mod decoder_thread;
pub mod handler;
pub mod output;
pub mod processor;
pub mod progress;

// Re-export the main decode function
pub use decode_impl::cmd_decode;
