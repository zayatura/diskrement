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

## The randomized sweep (P9)

`tests/sweep.rs` generates command sequences from a model of the project
lifecycle and runs each one through all three modes **at once**, diffing after
every command rather than only at the end.

```bash
cargo test -p diskrement-difftest --test sweep -- --ignored --nocapture
PROPTEST_CASES=500 cargo test -p diskrement-difftest --test sweep -- --ignored --nocapture
```

Narrow by default — twelve cases, about 76s against the corpus suite's 282s — so
it stays runnable after a change; `PROPTEST_CASES` widens it for a deep run.
There is no `.proptest-regressions` file: when the sweep finds something, the
fix is to repair the divergence and **paste the printed counterexample into
`corpus::all()` as a named scenario**, which is cheap, reviewable, and survives
this crate's deletion at P10.

Four things to know before reading a run:

- **The success line says what it exercised**, per command and split by exit
  status, and warns when no case reached a successful burn. A green run that
  touched nothing interesting is the failure mode this line exists to expose —
  at the generator's ~30% burn rate that is roughly one run in 70.
- **A failure names the diverging step**, not just the sequence, and shows both
  sides at the row that differs.
- **Under it the sweep re-runs the sequence through a second all-Scala world**,
  stepped and diffed identically. If that disagrees with itself, the report says
  so: the divergence is an unnormalized volatile value and says nothing about
  the port. That verdict is the difference between a night in `normalize.rs` and
  a night in `crates/`.
- **The generated sequence prints as `corpus.rs` source**, ready to paste. The
  general write form is rendered fully qualified because `corpus.rs` cannot
  import it — no hand-written scenario needs a third version of a file, and an
  unused import fails `-D warnings`.

The model in `src/sequence.rs` **steers generation; it is not an oracle.** A
command it wrongly believes legal is refused identically by both binaries and
the case still passes, which is why it stays small. The single exception is the
set of live files: a `Delete` of a path that is not there aborts the case, and a
test pins that against a shadow of the filesystem.

## Extending it

- **New command ported?** Append it to `runner::RUST_IMPLEMENTED` *in the same
  commit*. Until then, every scenario using it skips with that reason printed.
- **New scenario?** Add it to `corpus::all()`. Never hard-code an expected exit
  code or output line — both binaries run the step and the assertion is that
  they agree, which is why an error-path scenario is safe to add before the
  error is ported.
- **New command ported, or new argument?** Teach `sequence::Model` about it
  too, or the sweep will never generate it — and check the reachability test in
  that module still holds, since it is what stops the generator quietly drifting
  into sequences that never reach a burn.
- **A step that needs the real backend?** Use `scenario::cmd_real`, pointing it
  at a path that does not exist. The real burner and verifier both ask the
  operator before opening the device, and a runner-spawned child has no stdin,
  so the prompt is the whole comparison and no hardware is touched. A scenario
  with such a step requires `xorriso` and skips — with the reason printed —
  where it is not installed.
- **New normalization rule?** It is a hole punched in the oracle. Give it a named
  method, a doc comment saying what varies and why erasing it is safe, and a unit
  test. Never widen an existing rule to swallow an unrelated case.

## Relationship to `fixtures/`

The corpus is the JVM-free, default-on regression floor: it proves Rust reads
what Scala wrote, for one frozen scenario, on every `cargo test`. This harness is
the opt-in deep check covering what a recording structurally cannot — the reverse
direction, arbitrary scenarios, and generated command sequences (P9). Neither
replaces the other.
