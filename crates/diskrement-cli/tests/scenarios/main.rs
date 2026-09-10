//! The ported integration scenarios (`cli/src/it/scala/diskrement/cli/it/`).
//!
//! `binary.rs` pins one command's contract at a time. These pin multi-cycle
//! flows end to end and read the staged disc back through the production
//! `burn::json` codecs, which is the part no other suite covers.
//!
//! Every scenario needs a CoW work area, because `init` refuses one that is
//! not. Without it a scenario prints `SKIPPED` and passes — unless
//! `DISKREMENT_SCENARIOS_REQUIRED` is set, which turns the skip into a
//! failure. Set it whenever a green run is being used as evidence.

mod disc;
mod support;

mod s00_harness;
