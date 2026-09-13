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
mod s01_happy;
mod s02_append_open;
mod s03_finalized_new_disc;
mod s04_three_disc;
mod s05_spill;
mod s06_oversized;
mod s07_reset;
mod s08_idempotent;
mod s09_mark_dead;
mod s10_warm_rescan;
mod s11_exit_codes;
mod s12_realistic_tree;
