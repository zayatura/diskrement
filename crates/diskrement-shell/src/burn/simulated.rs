//! The stage-to-directory burner (`SimulatedBurner.scala`): the disc "image"
//! is a work-dir tree. Kept as the test/dev backend; selected with
//! `--burner simulated` / `DISKREMENT_BURNER=simulated`.

use crate::burn::{stager, BurnError, BurnResult, DiscImage};
use crate::progress::ProgressSink;

pub struct SimulatedBurner<'a> {
    progress: &'a dyn ProgressSink,
    parallelism: usize,
}

impl<'a> SimulatedBurner<'a> {
    pub fn new(progress: &'a dyn ProgressSink, parallelism: usize) -> Self {
        SimulatedBurner {
            progress,
            parallelism,
        }
    }

    pub fn burn(&self, image: &DiscImage) -> Result<BurnResult, BurnError> {
        stager::stage(image, self.progress, self.parallelism)
    }
}
