//! diskrement CLI: argument parsing, rendering, and the exit-code contract
//! (spec §5.3, §5.5). Path resolution and rendering live here; the pipeline
//! returns data and performs no path resolution (CLAUDE.md invariant).

mod args;
mod paths;
mod render;

fn main() {
    println!("diskrement {}", env!("CARGO_PKG_VERSION"));
}
