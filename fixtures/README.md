# Golden fixture corpus (Rust rewrite)

> **Status (2026-10-11):** the tests that compared Rust output against this
> corpus byte for byte or row for row (schema, manifest hash, burn replay,
> catalog replay, mark-dead replay, plan replay, pointers) were retired with
> the multiple-copies change, which deliberately changed the schema (V3) and
> the on-disc JSON. The corpus remains as **input**: V2 catalogs that must
> still migrate and read (`fixture_catalog.rs`), staged discs the verifier
> must accept (`fixture_verify.rs`), and timestamp strings (`timestamps.rs`).

Harvested from the **Scala** implementation by `scripts/harvest-fixtures.sh`,
which lived in this repository until P10 and survives only in its git history
— the `scala` branch preserves the sbt tree it drove, but not the script,
which was born during the rewrite, so `main` never carried it. This corpus
cannot be regenerated from `main`; treat it as frozen evidence. To re-derive
it, recover the script from history:
`git log --diff-filter=A -- scripts/harvest-fixtures.sh` finds the commit that
added it, then `git checkout <that commit> -- scripts/harvest-fixtures.sh`.
Scenario: two 800-byte files spill across discs 1-2 (one disc per burn cycle) ->
modify + delete cycle lands a@2 on disc 3 -> disc 3 marked dead -> recovery burn
onto disc 4 -> `plan --finalize` seals disc 4. Run with
`DISKREMENT_DISC_BYTES=1000` and the simulated burner.

## What each piece is

| path | what | byte-stable across re-harvests? |
|---|---|---|
| `catalog/NN-*.sqlite` | catalog at each recorded state (11 snapshots) | **no** — compare logically (query outputs), never bytes |
| `scancache/NN-*.sqlite` | fingerprint sidecar at each scan (4 snapshots) | **no** — same rule |
| `work/NN-after-planN/` | whole work dir at two staged-plan moments (the recovery unit: staged scan + plan + snapshot + previously burned staging trees). `snapshot/` is byte-stable; `staged/*.json` embed live file mtimes — compare semantically | snapshot yes; staged JSON and sqlite no |
| `discs/disc-N/` | staged disc trees as left by the last burn of each disc: `data/` + `.diskrement/{manifest,project,pointers}.json` + `catalog.sqlite`. `data/`, `manifest.json`, `pointers.json` are byte-stable; `project.json` embeds a volatile `createdAt` — compare semantically | data + manifest + pointers **yes**; `project.json`/`catalog.sqlite` no |
| `source/` | source tree as it stood at the end of the run (`b.bin` was deleted in cycle 2; its bytes live on in `discs/disc-2/data/`) | **yes** |
| `run-manifest.txt` | every command, exit code, and stdout — the **render oracle**. Scratch paths and the `Last burn` timestamp are redacted to `<scratch>`/`<timestamp>`; everything below the `#` header is byte-stable | **yes** |
| `stderr.log` | stderr — reference only, **not** an oracle (progress lines and scratch paths vary) | no |
| `perf-baseline.txt` | `perf-bench.sh` output for the Scala build; the P0 Scala baseline; `perf-rust.txt` is the P10 A/B against it | no (timings) |
| `perf-rust.txt` | the P10 same-host A/B: Rust and Scala on one tree | no (timings) |
| `SHA256SUMS` | integrity pins for what is committed | — |

### Disc-tree notes

- `discs/disc-N` holds the tree of the **last session staged for that disc**,
  not the cumulative disc: `burn` never clears staging, and the final
  `plan --finalize` + burn re-staged disc 4 with the (file-less) seal session.
  That is why `disc-4/data/` is empty — by design, and it is the only example
  of the seal-session manifest shape (`sessionOrdinal` 2, empty lists).
- Every distinct content byte is present: a@1 in `disc-1`, b@1 in `disc-2`,
  a@2 in `disc-3` (the LOST copy; the recovery copy on disc 4 has the same
  content hash).
- The four trees pin the four manifest shapes: initial add (disc 1), add
  session (disc 2), modify+delete session (disc 3), seal session (disc 4).

## Timestamp shapes (test data for `shell::catalog::time`)

The timestamp strings in these files are `java.time.Instant.toString()` output
with variable fractional precision. This corpus contains **3-digit**
(`...T19:56:11.446Z`, file mtimes) and **9-digit** (`...T19:56:28.287125297Z`,
`createdAt`, catalog `first_seen_at`) examples. No whole-second example occurs
here — construct one synthetically in tests (`Instant.parse` accepts it, and
`toString` omits the fraction when it is zero).

## Rules for consumers (P2+ compatibility tests)

- Catalog and scancache comparisons are **logical** — dumped rows or query
  outputs. SQLite file layout is not stable across library builds, and xerial's
  SQLite is not `libsqlite3-sys`'s.
- Manifest and pointer JSON compare **semantically** (parsed-value equality);
  field order is not an oracle, and `project.json`'s `createdAt` is expected to
  differ between the Scala and Rust runs.
- `run-manifest.txt` pins stdout and exit codes byte-for-byte for the
  success-path commands it records (modulo the redactions noted above). It is
  the only cross-implementation render oracle; `insta` snapshots pin Rust
  against Rust and cannot catch a wording drift from Scala.

## Regenerating

Do **not** regenerate casually — corpus changes are spec-level events. To
reproduce: `sbt cli/stage`, then
`scripts/harvest-fixtures.sh <launcher> <fresh-dir>` with `TMPDIR` on a CoW
filesystem, and diff `run-manifest.txt` (minus its `#` header), `source/`, and
the disc trees excluding `*.sqlite` and `project.json`.
