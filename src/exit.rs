//! Process exit codes.
//!
//! These values are part of the command-line contract and must stay stable:
//! callers use them to tell a damaged input from a full disk without parsing
//! log output.

/// Decoding or analysis completed.
pub const SUCCESS: i32 = 0;

/// Anything not covered by a more specific code.
pub const FAILURE: i32 = 1;

/// The command line could not be parsed. Produced by the argument parser
/// itself, and recorded here so the contract is complete.
#[allow(dead_code)]
pub const USAGE: i32 = 2;

/// The input could not be read.
pub const INPUT: i32 = 3;

/// The bitstream could not be parsed.
pub const PARSE: i32 = 4;

/// The audio could not be decoded.
pub const DECODE: i32 = 5;

/// Output could not be written.
pub const WRITE: i32 = 6;

/// An error that carries the code the process should exit with.
#[derive(Debug)]
pub struct ExitError {
    pub code: i32,
    pub source: anyhow::Error,
}

impl std::fmt::Display for ExitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.source.fmt(f)
    }
}

impl std::error::Error for ExitError {}

/// Code to exit with for an error, defaulting to [`FAILURE`].
pub fn code_for(error: &anyhow::Error) -> i32 {
    error
        .chain()
        .find_map(|error| error.downcast_ref::<ExitError>())
        .map_or(FAILURE, |error| error.code)
}
