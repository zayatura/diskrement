//! The pipeline (`Pipeline.scala`): returns reports, performs no path
//! resolution. P3 lands `init`, `scan` and `status`; later phases append the
//! rest of the nine methods.

pub mod reports;
pub mod staged;
