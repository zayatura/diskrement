//! Stages one disc tree and re-hashes it (`Stager.scala`).

use crate::burn::{BurnError, BurnResult, DiscImage};
use crate::progress::ProgressSink;

pub fn stage(
    _image: &DiscImage,
    _progress: &dyn ProgressSink,
    _parallelism: usize,
) -> Result<BurnResult, BurnError> {
    unimplemented!("Task 6")
}
