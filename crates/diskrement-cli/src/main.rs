//! diskrement CLI: argument parsing, rendering, and the exit-code contract
//! (spec §5.3, §5.5). P3 replaces this stub with the real command surface.

fn main() {
    // Proves the `cli -> shell` edge links at runtime, not just at compile time.
    let _ = diskrement_shell::probe();
    println!("diskrement {}", env!("CARGO_PKG_VERSION"));
}
