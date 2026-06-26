# diskrement Plan 3b — CLI Adapter Design

**Date:** 2026-06-21
**Status:** Approved for planning
**Parent design:** `docs/superpowers/specs/2026-05-31-diskrement-core-engine-design.md` (§9 CLI, §9.5 configuration & paths)
**Predecessor:** Plan 3a (orchestration / `Pipeline` service) — done and merged.

---

## 1. Scope

Plan 3b is the **CLI adapter**: the thin imperative veneer that turns a command line into a single `Pipeline` call and renders the structured result back to the terminal. It is the last piece of Cycle 1; once it lands, the tool is runnable end-to-end with simulated burns.

**In scope:**
- A new `cli` sbt module (depends on `shell`) with a `zio-cli` `CliApp` entry point.
- Argument parsing + dispatch for the eight commands `init`/`scan`/`plan`/`burn`/`reset`/`status`/`verify`/`history`, parsing into a plain command ADT.
- Path resolution: data dir from flags/systemd/XDG into a `ProjectPaths`; per-project catalog file `<dataDir>/<name>.sqlite`; `workDir` read from the project row for all commands except `init`.
- Layer wiring: assemble the full ZIO layer stack the `Pipeline` requires, per command.
- Human-readable rendering of every result type, an `init`/`burn` confirmation prompt, and a stable exit-code mapping.

**Out of scope (deferred):**
- **Cycle 2:** real ISO/UDF + hardware burning; a config file under `ConfigurationDirectory`; `--json` machine-readable output; shell-completion install tooling; interactive per-disc insert/label prompts (only meaningful with real hardware).
- **Unchanged from 3a:** the `Pipeline` interface. 3b adds **no** methods and changes **no** signatures — burn labels stay auto-generated (`"<name> disc <N>"`).

---

## 2. Architecture & conventions

Same functional-core / imperative-shell discipline as the earlier plans. The CLI is the outermost imperative shell: it parses, resolves paths, wires layers, calls the `Pipeline`, and renders. It contains **no business rules** and **no orchestration** — those live in `core` and `shell` respectively.

Layering is strict: `cli → shell → core`. The new `zio-cli` dependency is confined to the `cli` module so the testable `shell` core never sees it. ZIO 2.1.19; Scala 3.3.4; `-Wunused:all` (keep imports clean).

### 2.1 Key decisions

1. **New `cli` module, not a package in `shell`.** Isolates the `zio-cli` dependency from the shell core, gives `Main` a clean home, and keeps the layering honest.
2. **`zio-cli` for parsing and dispatch.** ZIO-native declarative commands give typed flags, `--help`, error messages, and completions for free, and `Main` is a `ZIOCliDefault`. Chosen over a hand-rolled parser (more code to own) and decline (pulls cats into a ZIO app).
3. **Parsing and rendering are pure and isolated.** zio-cli parses `argv → DiskrementCommand` (ADT); `Render` turns a `Pipeline` result `→ String`; `handle` turns a `PipelineError → (message, ExitCode)`. None of the three touch I/O, so each is unit-tested directly. The only effectful glue is `Runner`.
4. **Per-project catalog file.** `catalog = <dataDir>/<name>.sqlite`, holding exactly one project. This matches §9.5 naming and keeps `CatalogStore.snapshotTo` (a whole-DB `VACUUM INTO`) clean — each disc embeds only its own project's catalog, with no cross-project leakage or bloat. The multi-project `CatalogStore` API still works; each DB simply holds one project.
5. **`workDir` comes from the project row.** `init` takes `--work-dir` from a flag and stores it. Every other command reads `workDir` from the project row, so the user never re-specifies it, and the resolved `ProjectPaths` is always consistent with what `init` recorded.
6. **Confirm-only burn, auto labels.** `burn` displays the planned discs and asks one `[y/N]` confirmation (skipped by `--yes`), then calls the atomic `Pipeline.burn` unchanged. Per-disc insert/label prompting waits for Cycle 2 hardware.
7. **No env reads in core logic.** `PathResolver` takes its environment as an explicit lookup (provided from `System.getenv` in `Main`), never reading the real environment internally — the same discipline 3a applied to `Pipeline`.

---

## 3. Module / file structure

New module `cli` (`cli/src/main/scala/diskrement/cli/`):

- `Main.scala` — `ZIOCliDefault` entry point; builds the `CliApp` from the command model and runs `Runner`.
- `Commands.scala` — `zio-cli` `Command` definitions for the eight subcommands + global options; parses into `DiskrementCommand`.
- `Args.scala` — the `DiskrementCommand` sealed ADT (one case per command, carrying its parsed, typed arguments).
- `PathResolver.scala` — resolves the data dir + catalog path; reads `workDir` from the project row; produces `ProjectPaths`.
- `Runner.scala` — dispatch: for a `DiskrementCommand`, build the layer stack, call the `Pipeline` op, hand the result to `Render` and the error to `handle`.
- `Render.scala` — pure `Pipeline` result → `String` formatters.
- `Prompt.scala` — `[y/N]` confirmation over zio `Console` (testable with `TestConsole`).

`build.sbt`: add `lazy val cli = (project in file("cli")).dependsOn(shell)` with the `dev.zio %% zio-cli` dependency and `zio-test`/`zio-test-sbt` for tests; aggregate it under `root`.

Tests under `cli/src/test/scala/diskrement/cli/`.

---

## 4. Command surface

**Global options** (before the subcommand): `--catalog <file>` (use this catalog path directly), `--data-dir <dir>` (override the data dir). zio-cli supplies `--help` and `--version`.

| Command | Arguments / flags | Pipeline call |
|---|---|---|
| `init <name>` | `--source <dir>` (req), `--work-dir <dir>` (req), `--disc-type BD_R\|BD_R_DL\|DVD\|DVD_DL` (default `BD_R`), `--margin <pct>` (default `2.0`), `--copies <n>` (default `1`) | `init(ProjectInit(...))` |
| `scan <name>` | `--verify`, `--force` | `scan(name, verify, force)` |
| `plan <name>` | `--disc-type <T>` (optional override for new discs) | `plan(name, discTypeOverride)` |
| `burn <name>` | `--yes` (skip confirmation) | display plan → confirm → `burn(name)` |
| `reset <name>` | — | `reset(name)` |
| `status <name>` | — | `status(name)` |
| `verify <name>` | `--disc <N>` (optional, verify one disc) | `verify(name, discOrdinal)` |
| `history <name> <relPath>` | — | `history(name, RelPath(relPath))` |

`--disc-type` parses to the `core.DiscType` enum; an unknown value is a zio-cli usage error (exit `1`). `--margin` parses to a `Double` percent; `--copies` to an `Int`.

---

## 5. The flow of one invocation

```
argv ──zio-cli──▶ DiskrementCommand (ADT)
                       │
                       ▼
        PathResolver: dataDir = --data-dir ▸ --catalog(dir) ▸ $STATE_DIRECTORY
                              ▸ $XDG_DATA_HOME/diskrement ▸ ~/.local/share/diskrement
                      catalogPath = --catalog ▸ dataDir/<name>.sqlite
                      (non-init) open catalog, read project row → workDir
                      → ProjectPaths(catalogPath, workDir)
                       │
                       ▼
        Runner: provide layers (DbPath, CatalogStore.live, FileScanner.live,
                SourceSnapshot.live, SimulatedBurner.live, StagedStore.live,
                Hasher.live, ProjectPaths) → call Pipeline.<op>
                       │
              success ▼                         failure │
        Render: result → stdout              handle: PipelineError → stderr + ExitCode
```

- **`init`** is the one command that does not read a project row first: it takes `workDir` from `--work-dir`, builds a `ProjectInit`, and creates the catalog file + row. The data/catalog path is resolved the same way as every other command.
- **`burn`** calls `Pipeline.plan(name, None)` first (cheap, idempotent — it re-derives the already-staged plan) to display disc count, per-disc sizes, and spill, then prompts via `Prompt`. `--yes` bypasses the prompt. A declined burn exits `0` with no catalog change.
- The layer stack is the exact set `Pipeline.live` requires (§3 of the 3a design): `ProjectPaths & CatalogStore & FileScanner & SourceSnapshot & SimulatedBurner & StagedStore & Hasher`, plus the `DbPath` the catalog/snapshot layers need.

---

## 6. Path resolution (`PathResolver`)

Data dir precedence (first match wins):
1. `--data-dir <dir>` flag.
2. `--catalog <file>` flag — used directly as the catalog path; its parent is the effective data dir.
3. `$STATE_DIRECTORY` (set by systemd when running as a service).
4. `$XDG_DATA_HOME/diskrement`.
5. `~/.local/share/diskrement`.

Catalog path = the `--catalog` flag if given, else `dataDir/<name>.sqlite`.

- **Non-`init` commands:** open the catalog, `findProject(name)`. Missing file or missing row → `ProjectNotFound` (exit `2`). On success, take `workDir` from the row → `ProjectPaths`.
- **`init`:** `workDir` comes from `--work-dir`; the catalog file and row are created by `Pipeline.init`. (`init` still runs 3a's same-filesystem and CoW checks, which surface as `NotSameFilesystem`/`NotCowCapable`.)

`PathResolver` receives its environment as an explicit `String => Option[String]` lookup (wired to `System.getenv` in `Main`) and the user-home/`XDG` values likewise, so resolution is a pure function of its inputs and fully table-testable.

---

## 7. Rendering (`Render`)

Pure functions from each `Pipeline` result type to a terminal string, scaled to the command. Representative output:

```
$ diskrement scan photos --verify
Scanned: 12 added, 3 modified, 1 deleted, 540 unchanged.

$ diskrement plan photos --disc-type BD_R_DL
Plan: 2 discs (BD_R_DL)
  disc 1   48.6 GB   (top-off)
  disc 2    3.2 GB   (new)

$ diskrement burn photos
Plan: 2 discs (BD_R)
  disc 1   24.1 GB   (top-off)
  disc 2    3.2 GB   (new)
Proceed? [y/N] y
Burned disc 1 ✓   disc 2 ✓

$ diskrement status photos
Project photos  (source /srv/photos, BD_R, margin 2.0%)
Discs:
  #1  FINALIZED  24.8 / 25.0 GB   (full)
  #2  OPEN        3.2 / 25.0 GB   ← open
Current files: 556      Last burn: 2026-06-21 14:03 UTC

$ diskrement history photos vacation/img_001.jpg
vacation/img_001.jpg
  v1  disc 1 / session 1   3.4 MB  a1b2c3…  SUPERSEDED
  v2  disc 2 / session 2   3.5 MB  d4e5f6…  CURRENT
```

`plan`/`burn` surface `BurnPlan.oversized` / `BurnReport.oversized` as an explicit warning block listing the offending paths. `verify` prints each disc's `filesChecked` and any `mismatches`.

---

## 8. Error handling & exit codes

`Runner` folds the `PipelineError` channel through a single pure `handle(PipelineError): (String, ExitCode)`, so messages and codes stay consistent and unit-testable. Known errors print a concise stderr message (no stack trace); `Failed`/defects print a short message plus a "this is a bug" note.

| Code | Meaning | Sources |
|---|---|---|
| `0` | Success (including a declined `burn`) | — |
| `1` | Usage error | zio-cli parse failure (handled by zio-cli) |
| `2` | Project / staged state not found | `ProjectNotFound`, `NoStagedScan`, `NoStagedPlan`, `SnapshotMissing` |
| `3` | Precondition failed | `NotSameFilesystem`, `NotCowCapable`, `OversizedFiles` |
| `4` | Verify mismatch | `verify` returned non-empty `mismatches` |
| `5` | Internal failure | `Failed` / unexpected defects |

Exit `4` is **result-driven**, not error-driven: a `verify` that finds corruption is a successful run that reports a bad outcome. `Runner` inspects the returned `List[VerifyReport]`, renders the full report to stdout, and selects exit `4` when any disc has mismatches.

---

## 9. Testing strategy

Mirrors the project's discipline: pure units for everything that can be pure, plus one thin end-to-end seam.

1. **Arg parsing (`Commands`/`Args`)** — feed argv arrays through the zio-cli command; assert the resulting `DiskrementCommand`: every command, every flag, the disc-type/margin/copies defaults, and that bad input (unknown disc-type, missing required `--source`) is rejected. Pure.
2. **Rendering (`Render`)** — each `Pipeline` result type → expected string: `status` table, `scan` summary, `plan`/`burn` report (incl. oversized block), `verify` report, `history` list. Pure, golden-style.
3. **Error / exit mapping** — `handle(PipelineError)` for every case, plus the verify-mismatch path → expected `(message, ExitCode)`. Pure.
4. **`PathResolver`** — table-driven over env combinations (`STATE_DIRECTORY` set, `XDG_DATA_HOME` set, neither → `~/.local/share`), `--catalog`/`--data-dir` overrides, and catalog-filename derivation, via the injected env lookup (no real env mutation).
5. **`Prompt`** — `y` / `yes` / `N` / empty / EOF over `TestConsole`, and the `--yes` bypass.
6. **End-to-end smoke test** — drive `Main`'s `CliApp` through `init → scan → plan → burn → status` against a real temp dir + real per-project catalog, reusing 3a's CoW-aware integration pattern (auto-skip on non-CoW). Asserts exit codes and a couple of stdout lines. This is the only test that wires the full layer stack.

---

## 10. Mapping back to the engine design

| Engine design | Plan 3b delivery |
|---|---|
| §9.2 commands `init/scan/plan/burn/reset/status/verify/history` | §4 command surface → `Pipeline` calls |
| §9.1 staged pipeline, `--force`/`reset` restage policy | §4 flags (`scan --force`, `reset`), delegated to 3a |
| §9.2 `burn [--yes]` interactive confirmation | §5 + §2.1 decision 6 (confirm-only, auto labels) |
| §9.5 configuration & paths (systemd `StateDirectory` / XDG) | §6 `PathResolver` (config file deferred to Cycle 2) |
| §9.5 per-project `<project>.sqlite`, overridable | §2.1 decision 4 + §6 (`--catalog` override) |
| §9.2 `status`/`verify`/`history` output, integrity report | §7 rendering + §8 exit `4` for mismatch |

3b adds no `Pipeline` methods and changes no signatures; it is purely the terminal-facing adapter over the Cycle-1 orchestration.
