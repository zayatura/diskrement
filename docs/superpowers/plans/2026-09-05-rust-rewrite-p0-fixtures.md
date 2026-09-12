# Rust Rewrite — Phase P0: Golden Fixtures, Perf Baseline, and Workspace Scaffold

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Capture everything the Scala build knows that the Rust build will have to reproduce — a golden fixture corpus, a render oracle, and a performance baseline — then stand up an empty-but-green three-crate Cargo workspace beside the untouched Scala tree.

**Architecture:** `scripts/harvest-fixtures.sh` drives a staged Scala launcher through one deterministic multi-cycle scenario and records catalogs, scancaches, staged work dirs, disc trees, source bytes, and every command's stdout and exit code under `fixtures/`. `scripts/perf-bench.sh` records the scan/staging baseline P9 will compare against. Then a Cargo workspace lands under `crates/` — deliberately *not* in the `core/`, `shell/`, `cli/` directories the sbt modules occupy — so both build systems coexist and the Scala tree stays readable and runnable until P9.

**Tech Stack:** bash (harvest script), Rust stable pinned by `rust-toolchain.toml` (edition 2021), Cargo workspace. No async runtime. sbt and the Scala sources are read but never modified.

**Spec:** `docs/superpowers/specs/2026-09-05-rust-rewrite-design.md` — this plan implements phase P0 (§9); the corpus is §4, the workspace layout is §5.3.

---

## Context for the implementer

Nothing in this phase ports any behavior. Its whole job is to make later phases
*checkable*: after P0, "does the Rust version write what the Scala version
wrote?" is a question `cargo test` can answer, and "did the rewrite get slower?"
is a question with a recorded answer to compare against.

Three constraints that are easy to violate:

1. **Do not touch the Scala tree.** No edits under `core/`, `shell/`, `cli/`,
   `build.sbt`, or `project/`. Task 5 verifies this. The Scala sources are the
   specification for phases P1–P8 and the fixture generator for the whole
   rewrite; they are deleted in P9 and not before.
2. **The corpus is a spec-level artifact.** Anchors in the harvest script mirror
   assertions the integration suite already pins. If an anchor fails, the
   scenario drifted — stop and investigate. Never loosen an anchor to make the
   script pass.
3. **`init` requires a CoW filesystem.** The harvest must run with its scratch
   area on btrfs/XFS/bcachefs. The script probes for this and refuses early with
   a clear message rather than failing obscurely inside `init`.

### What "done" means for this phase

```
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
sbt core/test
```

All four clean; `fixtures/` committed and proven re-derivable; `git status`
showing no modifications under `core/`, `shell/`, or `cli/`.

---

## File structure

| File | Responsibility |
|------|----------------|
| `scripts/harvest-fixtures.sh` | Drives the Scala launcher through the fixture scenario; records the corpus and the render oracle |
| `fixtures/README.md` | What each artifact is, which are byte-stable, and the rules consumers must follow |
| `fixtures/run-manifest.txt` | Every command, exit code, and stdout — the render oracle |
| `fixtures/perf-baseline.txt` | `perf-bench.sh` output for the Scala build, for P9 to compare against |
| `rust-toolchain.toml` | Pins the stable channel so the whole rewrite compiles identically |
| `.cargo/config.toml` | `rustflags = ["-D", "warnings"]` — warnings-are-failures in one place |
| `Cargo.toml` | Workspace root: members, shared package metadata, shared dependency versions |
| `crates/diskrement-core/` | Stub crate proving the pure-core edge; P1 fills it |
| `crates/diskrement-shell/` | Stub crate proving the `shell → core` edge; P2 fills it |
| `crates/diskrement-cli/` | Stub binary crate proving the `cli → shell` edge and the spawned-binary test harness |

---

## Task 1: The harvest script

**Files:**
- Create: `scripts/harvest-fixtures.sh`

**Interfaces:**
- Consumes: a staged Scala launcher (`sbt cli/stage` output).
- Produces: a corpus directory at fixed paths that P2+ compatibility tests rely on:
  - `catalog/NN-after-<state>.sqlite`
  - `scancache/NN-after-scanN.sqlite`
  - `work/NN-after-planN/` (whole work dir at the staged-plan moments)
  - `discs/disc-N/` (`data/` + `.diskrement/{manifest,project,pointers}.json` + `catalog.sqlite`)
  - `source/` (the exact source tree)
  - `run-manifest.txt` (the render oracle), `stderr.log`, `SHA256SUMS`

- [ ] **Step 1: Write `scripts/harvest-fixtures.sh`**

The scenario: two 800-byte files spill across discs 1 and 2 in one burn; a
modify+delete cycle lands `a@2` on disc 3; disc 3 is marked dead; the live
content re-plans onto disc 4; `plan --finalize` seals disc 4. That single flow
exercises spill, supersede, delete, derived-`DEAD`, `LOST`-per-copy, recovery
burn, and `FINALIZED`. `DISKREMENT_DISC_BYTES=1000` with 800-byte files is the
same parameterisation `Scenario04ThreeDisc` and `Scenario05Spill` use.

```bash
#!/usr/bin/env bash
# Harvest the golden fixture corpus for the Rust rewrite.
#
# Spec: docs/superpowers/specs/2026-09-05-rust-rewrite-design.md (§4)
# Plan: docs/superpowers/plans/2026-09-05-rust-rewrite-p0-fixtures.md
#
# Drives the SCALA launcher through a deterministic multi-cycle scenario and
# records catalogs, scancaches, staged work states, disc trees, source bytes,
# and command outputs. The corpus pins the on-disc formats the Rust port must
# reproduce; compatibility tests from P2 onward consume it.
#
# Usage: harvest-fixtures.sh <launcher> <output-dir>
#   <launcher>   staged diskrement launcher from the Scala implementation
#                (`sbt cli/stage` -> cli/target/universal/stage/bin/diskrement)
#   <output-dir> must not exist; the script creates it
#
# Requirements: bash 4+, coreutils, and a scratch area on a CoW filesystem
# (btrfs/XFS/bcachefs), because `init` refuses anything else. TMPDIR selects
# the scratch area; set it if /tmp is not CoW.

set -euo pipefail

usage() { echo "usage: $0 <launcher> <output-dir>" >&2; exit 2; }
[ $# -eq 2 ] || usage
LAUNCHER=$1
OUTDIR=$2

[ -x "$LAUNCHER" ] || { echo "error: launcher '$LAUNCHER' is not executable" >&2; exit 2; }
[ -e "$OUTDIR" ] && { echo "error: output dir '$OUTDIR' already exists" >&2; exit 2; }

# --- scratch layout (mirrors the IT suite's Scratch) -------------------------
SCRATCH=$(mktemp -d "${TMPDIR:-/tmp}/diskrement-harvest.XXXXXX")
trap 'rm -rf "$SCRATCH"' EXIT
SRC=$SCRATCH/source; WORK=$SCRATCH/work
CAT=$SCRATCH/catalog/cat.sqlite
mkdir -p "$SRC" "$WORK" "$SCRATCH/catalog"

# IT-suite-style env isolation plus the harvest's fixed knobs.
export HOME=$SCRATCH/home
export XDG_DATA_HOME=$SCRATCH/home/.local/share
export STATE_DIRECTORY=$SCRATCH/home/state
export DISKREMENT_BURNER=simulated
export DISKREMENT_DISC_BYTES=1000
unset DISKREMENT_DEVICE DISKREMENT_HASH_PARALLELISM || true
mkdir -p "$HOME" "$XDG_DATA_HOME" "$STATE_DIRECTORY"

# --- CoW probe (mirrors the IT suite's CoW.capable) --------------------------
# Written as a function so that `!` negates the whole probe rather than binding
# to the first command of an && chain (which would invert the result).
cow_capable() {
  mkdir -p "$SCRATCH/.cow" \
    && printf 'abcd' > "$SCRATCH/.cow/src" \
    && cp --reflink=always "$SCRATCH/.cow/src" "$SCRATCH/.cow/dst" 2>/dev/null
}
if ! cow_capable; then
  echo "error: $SCRATCH is not on a CoW filesystem (btrfs/XFS/bcachefs); init would refuse." >&2
  echo "hint: set TMPDIR to a directory on a CoW filesystem and re-run." >&2
  exit 3
fi

# --- deterministic content (mirrors the IT suite's Tree) ---------------------
# Content derives from sha256("relPath@version"), repeated to the requested
# size: deterministic, distinct per (path, version), and byte-stable across
# re-harvests on any machine.
gen() { # gen <relPath> <version> <size>  -> bytes on stdout
  local seed out=""
  seed=$(printf '%s@%s' "$1" "$2" | sha256sum | cut -d' ' -f1)
  while [ "${#out}" -lt "$3" ]; do out+="$seed"; done
  printf '%s' "${out:0:$3}"
}
tree_write()  { mkdir -p "$(dirname "$SRC/$1")"; gen "$1" 0 "$2" > "$SRC/$1"; }
tree_modify() { mkdir -p "$(dirname "$SRC/$1")"; gen "$1" 1 "$2" > "$SRC/$1"; }
tree_delete() { rm -f "$SRC/$1"; }

# --- output skeleton --------------------------------------------------------
mkdir -p "$OUTDIR/catalog" "$OUTDIR/scancache" "$OUTDIR/work" "$OUTDIR/discs"
RUNLOG=$OUTDIR/run-manifest.txt
ERRLOG=$OUTDIR/stderr.log
{
  echo "# diskrement golden fixture run manifest"
  echo "# launcher: $LAUNCHER"
  echo "# launcher sha256: $(sha256sum "$LAUNCHER" | cut -d' ' -f1)"
  echo "# generated: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "# env: DISKREMENT_BURNER=simulated DISKREMENT_DISC_BYTES=1000"
} > "$RUNLOG"
: > "$ERRLOG"

# --- command recorder -------------------------------------------------------
STEP=0
LAST_EXIT=0
LAST_OUT=
run() { # run <label> <args...>  (the --catalog prefix is added here)
  local label=$1; shift
  STEP=$((STEP + 1))
  {
    printf '\n== step %02d [%s]\n$ diskrement' "$STEP" "$label"
    printf ' %q' "$@"
    printf '\n'
  } >> "$RUNLOG"
  printf '\n== step %02d [%s]\n' "$STEP" "$label" >> "$ERRLOG"
  local out code
  if out=$("$LAUNCHER" --catalog "$CAT" "$@" 2>>"$ERRLOG"); then code=0; else code=$?; fi
  LAST_EXIT=$code
  LAST_OUT=$out
  printf '%s\n[exit %d]\n' "$out" "$code" >> "$RUNLOG"
}
ok() { # ok <label> <args...> — run and require exit 0
  local label=$1; shift
  run "$label" "$@"
  if [ "$LAST_EXIT" -ne 0 ]; then
    echo "error: step $STEP ($label) exited $LAST_EXIT; see $ERRLOG" >&2
    exit 4
  fi
}
anchor() { # anchor <substring> — require the previous step's stdout to contain it
  if ! printf '%s' "$LAST_OUT" | grep -qF -- "$1"; then
    echo "error: step $STEP output missing anchor: '$1'" >&2
    exit 4
  fi
}
snap_cat()   { cp "$CAT" "$OUTDIR/catalog/$1.sqlite"; }
snap_cache() { cp "$SCRATCH/catalog/cat.p.scancache.sqlite" "$OUTDIR/scancache/$1.sqlite"; }
snap_work()  { cp -r "$WORK" "$OUTDIR/work/$1"; }

# --- the scenario -----------------------------------------------------------
# Anchors come from Render.scala: "Scanned: N added, ..." (:16), "new disc"
# (:27), "Wrote disc" (:48), "files OK" (:65), "marked DEAD" (:71). `init` and
# `plan --finalize` wording is not pinned by any test, so those steps assert
# exit 0 only. Never loosen an anchor without confirming against the Scala
# implementation that the new output is correct.

# cycle 1: two 800-byte files spill across discs 1 and 2 in one burn.
tree_write a.bin 800
tree_write b.bin 800

ok "init p" init p --source "$SRC" --work-dir "$WORK"
snap_cat 01-after-init

ok "scan p" scan p
anchor "2 added"
snap_cat 02-after-scan1
snap_cache 01-after-scan1

ok "plan p" plan p
anchor "new disc"
snap_work 01-after-plan1

ok "burn p --yes" burn p --yes
anchor "Wrote disc"
# disc 1 = [a@1] and is FINALIZED by the spill rule (a non-last planned disc is
# always finalized); disc 2 = [b@1] and is the OPEN disc, with 200 B headroom.
snap_cat 03-after-burn1

ok "verify p" verify p
anchor "files OK"

# cycle 2: modify a (supersede), delete b.
tree_modify a.bin 800
tree_delete b.bin

ok "scan p" scan p
anchor "1 modified, 1 deleted"
snap_cat 04-after-scan2
snap_cache 02-after-scan2

ok "plan p" plan p
# a@2 is 800 B and does not fit disc 2's 200 B headroom — disc 2 is the open
# disc here, not disc 1, which the cycle-1 spill finalized. So a fresh disc 3.
anchor "new disc"
snap_work 02-after-plan2

ok "burn p --yes" burn p --yes
anchor "Wrote disc"
snap_cat 05-after-burn2          # disc 3 = [a@2]

# the coaster: disc 3 dies. a@2 is current and had only that one copy, so it
# must be re-burned.
ok "disc mark-dead p 3 --yes" disc mark-dead p 3 --yes
anchor "marked DEAD"
snap_cat 06-after-mark-dead

ok "scan p" scan p
anchor "1 added"                 # a@2's burned view died with disc 3
snap_cat 07-after-scan3
snap_cache 03-after-scan3

ok "plan p" plan p
anchor "new disc"

ok "burn p --yes" burn p --yes
anchor "Wrote disc"
snap_cat 08-after-burn3          # disc 4 = [a@2], a second physical copy

ok "history p a.bin" history p a.bin
# LOST is a@2's copy on the dead disc 3; CURRENT is a@2's live copy on disc 4.
# Both rows share one content hash — that is the LOST-per-physical-copy
# invariant, not a bug. a@1 reads SUPERSEDED: disc 1 is alive.
anchor "LOST"
anchor "CURRENT"

# seal the non-full disc: --finalize on an empty changeset, then burn.
ok "plan p --finalize" plan p --finalize

ok "burn p --yes" burn p --yes
anchor "Wrote disc"
snap_cat 09-after-finalize       # disc 4 FINALIZED

ok "status p" status p
anchor "DEAD"

ok "verify p" verify p           # verifies 1, 2, 4; skips dead 3
anchor "files OK"

ok "history p b.bin" history p b.bin
anchor "DELETED"

# --- capture final artifacts ------------------------------------------------
# `burn` never clears staging (only `reset` does), so every staged disc tree
# from every cycle is still present, each with the .diskrement metadata that
# was written for it.
shopt -s nullglob
staged=("$WORK"/staging/disc-*)
shopt -u nullglob
[ "${#staged[@]}" -gt 0 ] || { echo "error: no staged discs under $WORK/staging" >&2; exit 4; }
for d in "${staged[@]}"; do cp -r "$d" "$OUTDIR/discs/"; done
cp -r "$SRC" "$OUTDIR/source"

# --- integrity manifest -----------------------------------------------------
( cd "$OUTDIR" && find . -type f ! -name SHA256SUMS -print0 \
    | sort -z | xargs -0 sha256sum > SHA256SUMS )

echo "ok: harvested $STEP steps into $OUTDIR"
```

- [ ] **Step 2: Make it executable and check it with shellcheck if available**

```bash
chmod +x scripts/harvest-fixtures.sh
command -v shellcheck >/dev/null && shellcheck scripts/harvest-fixtures.sh || echo "shellcheck not installed; skipping"
```

Expected: no output from `chmod`; shellcheck clean, or the skip message.

- [ ] **Step 3: Confirm the CoW probe returns the right answer**

The probe is the one piece of shell logic in this script whose failure mode is
silent and inverted, so verify it in isolation before trusting a run.

```bash
bash -c 'set -euo pipefail
S=$(mktemp -d); trap "rm -rf $S" EXIT
cow() { mkdir -p "$S/.cow" && printf abcd > "$S/.cow/src" && cp --reflink=always "$S/.cow/src" "$S/.cow/dst" 2>/dev/null; }
if ! cow; then echo "PROBE: not CoW"; else echo "PROBE: CoW ok"; fi'
stat -f -c %T "${TMPDIR:-/tmp}"
```

Expected: `PROBE: CoW ok` on a machine whose `TMPDIR` is btrfs/XFS/bcachefs
(the `stat` line tells you which filesystem it actually is). If it prints
`PROBE: not CoW` on a filesystem you know is CoW, the negation is binding to
`mkdir` instead of the whole function — do not proceed.

- [ ] **Step 4: Commit the script**

```bash
git add scripts/harvest-fixtures.sh
git commit -m "feat(rewrite): add golden fixture harvest script"
```

---

## Task 2: Run the harvest and commit the corpus

**Files:**
- Create: `fixtures/` (generated), `fixtures/README.md`

**Interfaces:**
- Consumes: `scripts/harvest-fixtures.sh`, a staged Scala launcher.
- Produces: the committed corpus.

- [ ] **Step 1: Stage the Scala launcher**

```bash
sbt cli/stage
LAUNCHER=$PWD/cli/target/universal/stage/bin/diskrement
"$LAUNCHER" --help
```

Expected: `--help` prints usage and exits 0. Prerequisites: JDK 17+, sbt (a cold
dependency cache needs network).

- [ ] **Step 2: Run the harvest**

```bash
bash scripts/harvest-fixtures.sh "$LAUNCHER" fixtures
```

Expected: ends with `ok: harvested 18 steps into fixtures`, exit 0.

If a step fails with an anchor error, the Scala output strings drifted from the
ones the integration suite pins — **stop and investigate**. Do not loosen the
anchor. If it fails with `not on a CoW filesystem`, set `TMPDIR` to a directory
on btrfs/XFS/bcachefs and re-run.

- [ ] **Step 3: Verify the corpus layout**

```bash
find fixtures -type f | sort
```

Expected exactly this set, with `work/` and `discs/` expanding to their trees:

```
fixtures/SHA256SUMS
fixtures/catalog/01-after-init.sqlite
fixtures/catalog/02-after-scan1.sqlite
fixtures/catalog/03-after-burn1.sqlite
fixtures/catalog/04-after-scan2.sqlite
fixtures/catalog/05-after-burn2.sqlite
fixtures/catalog/06-after-mark-dead.sqlite
fixtures/catalog/07-after-scan3.sqlite
fixtures/catalog/08-after-burn3.sqlite
fixtures/catalog/09-after-finalize.sqlite
fixtures/discs/disc-1/...        (data/a.bin + .diskrement/*)
fixtures/discs/disc-2/...        (data/b.bin + .diskrement/*)
fixtures/discs/disc-3/...        (data/a.bin + .diskrement/*)
fixtures/discs/disc-4/...        (data/a.bin + .diskrement/*)
fixtures/run-manifest.txt
fixtures/scancache/01-after-scan1.sqlite
fixtures/scancache/02-after-scan2.sqlite
fixtures/scancache/03-after-scan3.sqlite
fixtures/source/a.bin
fixtures/stderr.log
fixtures/work/01-after-plan1/...
fixtures/work/02-after-plan2/...
```

Note `fixtures/source/` holds only `a.bin`: the scenario deletes `b.bin` in
cycle 2, and `source/` is captured at the end. `b.bin`'s bytes survive in
`fixtures/discs/disc-2/data/b.bin`, which is where the compatibility tests need
them.

- [ ] **Step 4: Sanity-check the oracle records the interesting states**

```bash
grep -c '^\[exit 0\]' fixtures/run-manifest.txt
grep -q '1 modified, 1 deleted' fixtures/run-manifest.txt && echo "supersede+delete recorded"
grep -q 'marked DEAD'           fixtures/run-manifest.txt && echo "coaster recorded"
grep -q 'LOST'                  fixtures/run-manifest.txt && echo "LOST recorded"
grep -q 'DELETED'               fixtures/run-manifest.txt && echo "tombstone recorded"
ls fixtures/discs | wc -l
```

Expected: `18`, then all four confirmations, then `4`.

- [ ] **Step 5: Confirm the JSON the corpus froze**

The manifest and pointer JSON are the artifacts with no schema constraint
protecting them, so eyeball the enum spellings and timestamp shapes now — these
are the exact strings the Rust serde types must emit (spec §3.1).

```bash
cat fixtures/discs/disc-4/.diskrement/manifest.json
cat fixtures/discs/disc-4/.diskrement/project.json
grep -o '"[a-zA-Z]*":"[0-9T:.Z-]*"' fixtures/discs/disc-4/.diskrement/project.json
```

Expected: bare uppercase enum names (`ADDED`, `MODIFIED`, `BD_R`, …) and
`Z`-suffixed ISO-8601 timestamps. Record in the commit message body whether the
timestamps carry 0, 3, 6, or 9 fractional digits — P2's `time` module test needs
real examples of both the whole-second and sub-second forms.

- [ ] **Step 6: Write `fixtures/README.md`**

```markdown
# Golden fixture corpus (Rust rewrite)

Harvested from the **Scala** implementation by `scripts/harvest-fixtures.sh`.
Scenario: two 800-byte files spill across discs 1-2 -> modify + delete cycle
lands a@2 on disc 3 -> disc 3 marked dead -> recovery burn onto disc 4 ->
`plan --finalize` seals disc 4. Run with `DISKREMENT_DISC_BYTES=1000` and the
simulated burner.

## What each piece is

| path | what | byte-stable across re-harvests? |
|---|---|---|
| `catalog/NN-*.sqlite` | catalog at each recorded state | **no** — compare logically (query outputs), never bytes |
| `scancache/NN-*.sqlite` | fingerprint sidecar at each scan | **no** — same rule |
| `work/NN-after-planN/` | whole work dir at the staged-plan moments (the recovery unit: staged scan + plan + snapshot) | JSON yes; sqlite no |
| `discs/disc-N/` | staged disc trees: `data/` + `.diskrement/{manifest,project,pointers}.json` + `catalog.sqlite` | data + JSON yes; embedded `catalog.sqlite` no |
| `source/` | source tree as it stood at the end of the run (`b.bin` was deleted in cycle 2; its bytes live on in `discs/disc-2/data/`) | **yes** |
| `run-manifest.txt` | every command, exit code, and stdout — the **render oracle** | **yes** |
| `stderr.log` | stderr — reference only, **not** an oracle (progress lines vary) | no |
| `perf-baseline.txt` | `perf-bench.sh` output for the Scala build; P9 compares against it | no (timings) |
| `SHA256SUMS` | integrity pins for what is committed | — |

## Rules for consumers (P2+ compatibility tests)

- Catalog and scancache comparisons are **logical** — dumped rows or query
  outputs. SQLite file layout is not stable across library builds, and xerial's
  SQLite is not `libsqlite3-sys`'s.
- Manifest and pointer JSON compare **semantically** (parsed-value equality);
  field order is not an oracle.
- `run-manifest.txt` pins stdout and exit codes byte-for-byte for the
  success-path commands it records. It is the only cross-implementation render
  oracle; `insta` snapshots pin Rust against Rust and cannot catch a wording
  drift from Scala.
- The timestamp strings in these files are `java.time.Instant.toString()` output
  with variable fractional precision. They are the test data for
  `shell::catalog::time`.

## Regenerating

Do **not** regenerate casually — corpus changes are spec-level events. To
reproduce: `sbt cli/stage`, then
`scripts/harvest-fixtures.sh <launcher> <fresh-dir>` with `TMPDIR` on a CoW
filesystem, and diff `run-manifest.txt` (minus its `#` header), `source/`, and
the disc trees excluding `*.sqlite`.
```

- [ ] **Step 7: Commit the corpus**

```bash
git add fixtures/
git commit -m "test(rewrite): harvest golden fixture corpus from the Scala launcher"
```

---

## Task 3: Record the performance baseline

**Files:**
- Create: `fixtures/perf-baseline.txt`

**Interfaces:**
- Consumes: `scripts/perf-bench.sh SOURCE_DIR WORK_AREA [BIN]` and the staged Scala launcher.
- Produces: the recorded baseline P9's comparison run is measured against.

Without this, "no regression on the scan/staging paths" (spec §1) has nothing to
be measured against, and by P9 the Scala build is being deleted. Record it now.

- [ ] **Step 1: Pick a source tree and work area on the same CoW filesystem**

`SOURCE_DIR` and `WORK_AREA` are values you supply, not placeholders to fill in
later: pick a real tree big enough for the timings to mean something (tens of GB
of photos is the workload this tool exists for; a few hundred MB will not show a
cache regression) and a scratch directory on the same filesystem. Write the two
paths you chose into the header in Step 3 — P9 must use the same pair.

```bash
stat -f -c %T <SOURCE_DIR> <WORK_AREA>
df -h <WORK_AREA> | tail -1
```

Expected: both report btrfs/XFS/bcachefs, and the work area has room for the
metadata-only reflink staging the script documents. `SOURCE_DIR` is only ever
read. Use the same pair of paths in P9 — a baseline against a different tree is
not a baseline.

- [ ] **Step 2: Run the benchmark against the Scala launcher**

```bash
scripts/perf-bench.sh <SOURCE_DIR> <WORK_AREA> \
  "$PWD/cli/target/universal/stage/bin/diskrement" \
  | tee fixtures/perf-baseline.txt
```

Expected: per-step wall time and the three `/proc/<pid>/io` columns for
`init → scan → plan → burn → scan`. The second `scan` should show `rchar` far
below the source size — that is the fingerprint cache working, and it is the
number P9 most needs to match.

For a cold-cache figure, re-run with `DROP_CACHES=1` (needs root) and append it
to the same file, labelled.

- [ ] **Step 3: Annotate and commit**

Prepend a header recording the conditions, because a timing without its context
is not comparable:

```bash
{
  echo "# Scala baseline for the Rust rewrite (spec §1, compared in P9)"
  echo "# date: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "# commit: $(git rev-parse --short HEAD)"
  echo "# source: <SOURCE_DIR>  work: <WORK_AREA>"
  echo "# fs: $(stat -f -c %T <WORK_AREA>)  cpu: $(nproc) threads"
  echo "# java: $(java -version 2>&1 | head -1)"
  echo "#"
  cat fixtures/perf-baseline.txt
} > /tmp/pb && mv /tmp/pb fixtures/perf-baseline.txt

( cd fixtures && find . -type f ! -name SHA256SUMS -print0 | sort -z | xargs -0 sha256sum > SHA256SUMS && sha256sum -c SHA256SUMS )
git add fixtures/
git commit -m "test(rewrite): record Scala perf baseline for the P9 comparison"
```

Expected: `sha256sum -c` reports every file `OK`.

---

## Task 4: Cargo workspace scaffold

**Files:**
- Create: `rust-toolchain.toml`, `.cargo/config.toml`, `Cargo.toml`
- Create: `crates/diskrement-core/Cargo.toml`, `crates/diskrement-core/src/lib.rs`
- Create: `crates/diskrement-shell/Cargo.toml`, `crates/diskrement-shell/src/lib.rs`
- Create: `crates/diskrement-cli/Cargo.toml`, `crates/diskrement-cli/src/main.rs`, `crates/diskrement-cli/tests/binary.rs`

**Interfaces:**
- Consumes: nothing (greenfield).
- Produces: crates `diskrement-core`, `diskrement-shell`, `diskrement-cli`
  (binary name `diskrement`), version `0.1.0`, edition 2021; a `cargo test
  --workspace` that proves the toolchain, both dependency edges, the
  warnings-are-failures policy, and the spawned-binary test harness.

The stub items (`Probe`, `probe()`) exist only to give the dependency edges
something real to link, and they are the analogue of the Scala `SmokeTest` /
`SmokeSpec` / `BuildSmokeSpec` trio. P1 replaces `Probe` with the real model
types and deletes it; P2 does the same for `probe()`.

- [ ] **Step 1: Write the toolchain and workspace files**

`rust-toolchain.toml`:
```toml
[toolchain]
channel = "stable"
```

`.cargo/config.toml`:
```toml
# Warnings are failures for this workspace (spec §5.4). Kept here rather than as
# per-crate `#![deny(warnings)]` so the policy lives in one place.
#
# These rustflags reach dependency compilation too, but registry crates are
# built with `--cap-lints allow`, so third-party warnings cannot fail the build.
[build]
rustflags = ["-D", "warnings"]
```

`Cargo.toml` (workspace root):
```toml
[workspace]
resolver = "2"
members = [
    "crates/diskrement-core",
    "crates/diskrement-shell",
    "crates/diskrement-cli",
]

[workspace.package]
version = "0.1.0"
edition = "2021"
license = "GPL-3.0-only"
publish = false
repository = "https://github.com/zayatura/diskrement"

[workspace.dependencies]
diskrement-core = { path = "crates/diskrement-core" }
diskrement-shell = { path = "crates/diskrement-shell" }
chrono = { version = "0.4", default-features = false, features = ["std"] }
clap = { version = "4", features = ["derive"] }
rayon = "1"
reflink-copy = "0.1"
rusqlite = { version = "0.32", features = ["bundled"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
sha2 = "0.10"
thiserror = "2"
walkdir = "2"
assert_cmd = "2"
insta = "1"
predicates = "3"
proptest = "1"
tempfile = "3"
```

Only the dependencies each crate actually needs are wired into that crate below;
the rest sit in `[workspace.dependencies]` unused until the phase that needs
them, which is what keeps the version list in one place.

- [ ] **Step 2: Write the core crate**

`crates/diskrement-core/Cargo.toml`:
```toml
[package]
name = "diskrement-core"
description = "diskrement functional core: diff, planning, manifests — pure, no I/O"
version.workspace = true
edition.workspace = true
license.workspace = true
publish.workspace = true
repository.workspace = true

# Deliberately dependency-poor. `chrono` with default features disabled excludes
# the `clock` feature, so `Utc::now()` is not reachable from here — purity is
# enforced by the dependency graph, not by discipline. Do not add a dependency
# to this crate without a very good reason.
[dependencies]
chrono.workspace = true

[dev-dependencies]
proptest.workspace = true
```

`crates/diskrement-core/src/lib.rs`:
```rust
//! diskrement's functional core: pure data and pure functions, no I/O.
//!
//! Nothing here touches the filesystem, SQLite, a subprocess, or the clock.
//! The shell crate owns every effect; this crate owns the change-set,
//! planning, and manifest models (spec §5.3).

/// Temporary P0 scaffolding: exists only so the `shell -> core` and
/// `cli -> shell -> core` dependency edges have something real to link.
/// P1 replaces it with the actual model types and deletes it.
pub struct Probe;

#[cfg(test)]
mod tests {
    #[test]
    fn crate_builds() {
        // Mirrors the Scala `SmokeTest`: proves the crate compiles and its test
        // harness runs before any real code exists.
    }
}
```

- [ ] **Step 3: Write the shell crate**

`crates/diskrement-shell/Cargo.toml`:
```toml
[package]
name = "diskrement-shell"
description = "diskrement imperative shell: catalog, scanning, staging, burners"
version.workspace = true
edition.workspace = true
license.workspace = true
publish.workspace = true
repository.workspace = true

[dependencies]
diskrement-core.workspace = true
chrono.workspace = true
rayon.workspace = true
reflink-copy.workspace = true
rusqlite.workspace = true
serde.workspace = true
serde_json.workspace = true
sha2.workspace = true
thiserror.workspace = true
walkdir.workspace = true

[dev-dependencies]
proptest.workspace = true
tempfile.workspace = true
```

`crates/diskrement-shell/src/lib.rs`:
```rust
//! diskrement's imperative shell: every effect lives here — the SQLite
//! catalog, file scanning, CoW snapshots, staging, and the burners
//! (spec §5.3, §5.5).

use diskrement_core::Probe;

/// Temporary P0 scaffolding: proves the `shell -> core` edge links.
/// P2 deletes it when real adapters consume real core types.
pub fn probe() -> Probe {
    Probe
}

#[cfg(test)]
mod tests {
    #[test]
    fn core_edge_links() {
        let _: diskrement_core::Probe = super::probe();
    }
}
```

- [ ] **Step 4: Write the cli crate**

`crates/diskrement-cli/Cargo.toml`:
```toml
[package]
name = "diskrement-cli"
description = "diskrement CLI: argument parsing, rendering, exit-code contract"
version.workspace = true
edition.workspace = true
license.workspace = true
publish.workspace = true
repository.workspace = true

[[bin]]
name = "diskrement"
path = "src/main.rs"

[dependencies]
diskrement-shell.workspace = true
clap.workspace = true

[dev-dependencies]
assert_cmd.workspace = true
insta.workspace = true
predicates.workspace = true
tempfile.workspace = true
```

`crates/diskrement-cli/src/main.rs`:
```rust
//! diskrement CLI: argument parsing, rendering, and the exit-code contract
//! (spec §5.3, §5.5). P3 replaces this stub with the real command surface.

fn main() {
    // Proves the `cli -> shell` edge links at runtime, not just at compile time.
    let _ = diskrement_shell::probe();
    println!("diskrement {}", env!("CARGO_PKG_VERSION"));
}
```

`crates/diskrement-cli/tests/binary.rs`:
```rust
//! The spawned-binary harness. This is the P0 analogue of `BuildSmokeSpec`:
//! every later phase's exit-code assertions run through `Command::cargo_bin`,
//! so prove the mechanism works before anything depends on it.

use assert_cmd::Command;

#[test]
fn binary_runs_and_prints_its_version() {
    Command::cargo_bin("diskrement")
        .expect("binary builds")
        .assert()
        .success()
        .stdout(predicates::str::contains(env!("CARGO_PKG_VERSION")));
}
```

- [ ] **Step 5: Build and test**

```bash
cargo test --workspace
rustc --version
```

Expected: three test binaries, one passing test each — `crate_builds`
(core), `core_edge_links` (shell), and `binary_runs_and_prints_its_version`
(cli's `tests/binary.rs`) — plus empty doc-test runs. Zero warnings; `-D
warnings` would have failed the build otherwise. Record the `rustc --version`
output for the commit message body.

The sbt tree still sits alongside under `core/`, `shell/`, and `cli/`; cargo
never looks there, and sbt never looks at `crates/`. `.gitignore` already has
`target/`, which matches a directory of that name at any depth, so cargo's
`target/` is covered with no edit. `Cargo.lock` **is** committed — this is an
application, and reproducible builds are the point of pinning the toolchain.

- [ ] **Step 6: Verify lint and format are clean**

```bash
cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check
```

Expected: no output from either; exit 0.

- [ ] **Step 7: Commit**

```bash
git add rust-toolchain.toml .cargo Cargo.toml Cargo.lock crates
git commit -m "chore(rewrite): scaffold cargo workspace (core, shell, cli)" \
           -m "rustc <version from Step 5>; edition 2021; -D warnings via .cargo/config.toml"
```

---

## Task 5: Phase verification

**Files:**
- Create: none (verification only; a fix commit only if a check fails and is repaired)

**Interfaces:**
- Consumes: everything above.
- Produces: the P0 exit criteria, with evidence in hand.

- [ ] **Step 1: Clean full Rust run**

```bash
cargo clean && cargo test --workspace \
  && cargo clippy --workspace --all-targets -- -D warnings \
  && cargo fmt --check
```

Expected: 3 tests pass from a cold target dir; clippy and rustfmt silent.

- [ ] **Step 2: Prove the corpus is re-derivable**

A corpus that cannot be regenerated is a corpus nobody can ever safely change.
Re-harvest into a throwaway directory and diff what should be deterministic.

```bash
RECHECK=$(mktemp -d)/fixtures-recheck
bash scripts/harvest-fixtures.sh "$PWD/cli/target/universal/stage/bin/diskrement" "$RECHECK"

# The `#` header records the launcher path, its hash, and a timestamp, all of
# which are volatile by design. Compare only the recorded steps.
diff <(grep -v '^# ' "$RECHECK/run-manifest.txt") \
     <(grep -v '^# ' fixtures/run-manifest.txt) && echo "oracle identical"
diff -r "$RECHECK/source" fixtures/source && echo "source bytes identical"
diff -r --exclude='*.sqlite' "$RECHECK/discs" fixtures/discs && echo "disc trees identical"
( cd fixtures && sha256sum -c SHA256SUMS >/dev/null ) && echo "committed sums OK"
rm -rf "$RECHECK"
```

Expected: all four confirmations. If the oracle diff is non-empty, the Scala
behavior is nondeterministic in a way the corpus must not paper over — **stop
and investigate** before any later phase depends on it. Embedded and top-level
`*.sqlite` files are excluded from the tree diff on purpose (spec §4).

- [ ] **Step 3: Confirm the Scala build is untouched and still green**

```bash
git status --porcelain
sbt core/test
```

Expected: `git status` clean, with no modifications anywhere under `core/`,
`shell/`, `cli/`, `build.sbt`, or `project/` in this phase's commits; `sbt
core/test` reports **51 tests passing**. P0 adds; it removes nothing.

```bash
git log --oneline -4
git diff --stat main -- core shell cli build.sbt project
```

Expected: the four P0 commits, and an **empty** diff against `main` for those
paths.

- [ ] **Step 4: Record completion**

P0's exit criteria (spec §9) are met when: the corpus is committed and proven
re-derivable (Tasks 1–2, Step 2 above), the perf baseline is recorded (Task 3),
the workspace is green under all three checks (Task 4, Step 1 above), and the
Scala build is untouched and passing (Step 3). No further commit is required;
report the evidence — including the fractional-digit shapes observed in Task 2
Step 5, which P2 needs.

---

## Phase P0 acceptance

- [ ] `cargo test --workspace` reports 3 passing tests across the three
      crates, 0 failing.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` is silent.
- [ ] `cargo fmt --check` is silent.
- [ ] `fixtures/` holds 9 catalog snapshots, 3 scancache snapshots, 2 staged work
      dirs, 4 disc trees, the source tree, `run-manifest.txt` with 18 recorded
      `[exit 0]` steps, `stderr.log`, `perf-baseline.txt`, and a verifying
      `SHA256SUMS`.
- [ ] A second harvest reproduces `run-manifest.txt` (minus its header),
      `source/`, and the disc trees (excluding `*.sqlite`) byte-for-byte.
- [ ] `fixtures/README.md` states the comparison rules from spec §4.
- [ ] `git diff main -- core shell cli build.sbt project` is empty and
      `sbt core/test` still reports 51 tests passing.

**Next:** phase P1 (the pure functional core), which gets its own plan document.

---

## Execution deviations (P0, recorded 2026-09-05)

1. **Scenario corrected against the one-disc-per-cycle invariant (Tasks 1–2).**
   The drafted scenario assumed one `burn` could write discs 1 *and* 2; the
   implementation writes exactly one disc per scan→plan→burn cycle (CLAUDE.md
   invariant; `Scenario05Spill` spills "in the next cycle"). The harvest gained
   a cycle 1b (`scan` "1 added" → `plan` → `burn`: b@1 lands on disc 2, left
   OPEN) and an empty `scan` before `plan --finalize` (every plan consumes the
   staged scan; `PipelineLive` fails with "no staged scan" otherwise —
   confirmed by `Scenario03FinalizedNewDisc`, which always scans first).
   Consequences: 22 recorded `[exit 0]` steps (not 18), 11 catalog snapshots
   (not 9), 4 scancache snapshots (not 3); work dirs and disc trees unchanged
   at 2 and 4. All original anchors are preserved; the only new anchor is
   cycle 1b's "1 added".
2. **SC2094 fix in the SHA256SUMS generation (Task 1).** Sums are captured to
   a variable before being written; the drafted one-liner's redirect creates
   the output file before `find` finishes enumerating (shellcheck info-level,
   and a genuine race if the script is ever re-run into a non-empty dir).
3. **Oracle redactions (Task 1).** The scratch path appears in recorded
   command lines (`init`) and `status` output, and `status` prints the last
   burn timestamp; all three are redacted to `<scratch>`/`<timestamp>` so the
   oracle is byte-stable across re-harvests modulo its `#` header. `stderr.log`
   remains unredacted (not an oracle).
4. **`discs/disc-4` holds the seal session, not the recovery session
   (Task 2).** `plan --finalize` re-stages disc 4, and the seal burn
   re-materializes `staging/disc-4`, overwriting the recovery-session tree, so
   the captured tree has an empty `data/` and `sessionOrdinal: 2`. This is
   kept deliberately: it pins the seal-session manifest shape (the only one of
   the four shapes with empty lists *and* session > 1), while a@2's recovery
   bytes are identical (same content hash) to `discs/disc-3/data/a.bin`.
5. **`project.json` excluded from the disc-tree re-derivation diff (Task 5
   Step 2).** It embeds a volatile `createdAt`; consumers compare it
   semantically (fixtures/README.md).
6. **Perf baseline uses a synthetic tree (Task 3).** User-approved substitute
   for a real photo library: 2000 files / 2.072 GB of incompressible urandom
   content on btrfs `/tmp` (compress=zstd:3), source
   `/tmp/diskrement-bench-src`, work `/tmp/diskrement-bench-work`; the exact
   generation recipe is in the committed header, and P9 must re-run against
   the same pair of paths. Run is short (1–3 s/step): wall times are the
   baseline, per-step I/O columns are approximate.
7. **Timestamp shapes observed for P2 (Task 2 Step 5):** 3-digit fractions on
   file mtimes (`...T19:56:11.446Z`) and 9-digit on `createdAt`/catalog
   `first_seen_at` (`...T19:56:28.287125297Z`); **no whole-second example
   occurs in the corpus** — construct those synthetically in the
   `shell::catalog::time` tests.
