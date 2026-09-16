//! Object audio metadata, re-exported from the [`oamd`] crate.
//!
//! These structures are not TrueHD: the same payload reaches a decoder through E-AC-3 with JOC
//! and through AC-4 as well, so they live in a crate of their own. This module keeps the path
//! they were reachable at.

pub use ::oamd::*;
