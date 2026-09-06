//! The filesystem subsystem: hashing, scanning, the fingerprint cache, and the
//! CoW source snapshot (spec §5.3).

pub mod fingerprint_cache;
pub mod hasher;
pub mod scanner;
