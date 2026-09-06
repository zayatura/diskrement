# diskrement-difftest

Dev-only differential harness for the Rust rewrite (spec §8.1). It runs a
scenario through the Scala binary, the Rust binary, or the two alternating, and
proves the runs agree — in command output and in the end state left on disk.

**This crate is disposable.** It exists only while both implementations do, and
P10 deletes it together with the Scala tree. Nothing shipping may depend on it.

## Running

```bash
sbt cli/stage
export DISKREMENT_SCALA_BIN="$PWD/cli/target/universal/stage/bin/diskrement"
export TMPDIR=/path/on/btrfs-or-xfs      # `init` refuses a non-CoW work area
cargo build -p diskrement-cli
cargo test -p diskrement-difftest -- --ignored --nocapture
```

Every differential test is `#[ignore]`d, so routine `cargo test` stays JVM-free.
The unit tests for `content`, `scenario`, `normalize` and `state_diff` are not
ignored and run normally.

## The three modes

| mode | what it proves |
|---|---|
| `Mode::Scala` | **the harness itself** — one scenario run twice through one binary must be identical. Run this first; a failure here is an unnormalized volatile value, not a porting bug. |
| `Mode::Rust` | forward parity, for any scenario rather than the corpus's single harvested one |
| `Mode::Interleaved` | that switching binaries mid-project is invisible (spec §3.1). Subsumes the reverse direction: Scala reads what Rust wrote. |

## Extending it

- **New command ported?** Append it to `runner::RUST_IMPLEMENTED` *in the same
  commit*. Until then, every scenario using it skips with that reason printed.
- **New scenario?** Add it to `corpus::all()`. Never hard-code an expected exit
  code or output line — both binaries run the step and the assertion is that
  they agree, which is why an error-path scenario is safe to add before the
  error is ported.
- **New normalization rule?** It is a hole punched in the oracle. Give it a named
  method, a doc comment saying what varies and why erasing it is safe, and a unit
  test. Never widen an existing rule to swallow an unrelated case.

## Relationship to `fixtures/`

The corpus is the JVM-free, default-on regression floor: it proves Rust reads
what Scala wrote, for one frozen scenario, on every `cargo test`. This harness is
the opt-in deep check covering what a recording structurally cannot — the reverse
direction, arbitrary scenarios, and generated command sequences (P9). Neither
replaces the other.
