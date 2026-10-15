# Project roots — design

**Date:** 2026-10-15
**Status:** Draft, awaiting review
**Branch:** `feat/project-roots`

---

## 1. Problem

A project archives one directory: `project.source_root`. The Immich store has one
library directory per user:

```
/data/ext/services/immich/store/library
  388.6 GiB  d30dd4c8-…   (archived as the live project immich-dsz)
   48.1 GiB  65966390-…
   19.7 GiB  4efdf184-…
   14.3 GiB  48199862-…
    6.0 GiB  e966b06b-…
```

The four smaller libraries add up to about 88 GiB, a handful of BD-R discs. As
separate projects, each would need its own disc series, mostly empty. They should
share one series instead. Each library gets a readable top-level directory on the
disc (not an Immich user UUID), and the discs fill in date order across all four
libraries. Disc 1 holds the oldest files from every library, disc 2 the next ones,
and so on.

## 2. Decision

**A project may have named roots instead of a single source.** At `init`, each root
is a `NAME=PATH` pair. `plan` builds the snapshot by reflinking every root to
`<work_dir>/snapshot/<NAME>`. From then on the project behaves exactly as if the
source were one directory laid out that way. Scanning, the catalog, staging, burning,
both verifiers and the disc's embedded catalog work unchanged on `rel_path`s such as
`family/anna/2024/2024-01-15/IMG-20240115-WA0001.jpeg`.

The planner's sort key is the only other behaviour change. A rooted project fills
discs in order of *(path below the root, root name)*, not the full `rel_path`. Immich's
storage template is `YYYY/YYYY-MM-DD/<file>`, so the path below the root is the date
taken, and the libraries interleave by date.

A project without roots (`--source PATH`) is unchanged in every respect, including
its fill order.

### 2.1 Alternatives rejected

- **Map the first path component to a root wherever files are read** (scanner,
  snapshot, stager, staging verifier). The disc layout would be the same, but every
  reader would have to know about the mapping, and the invariant "every disc is
  staged from `work_dir/snapshot`" would gain an exception. Composing the snapshot
  keeps the mapping in one place.
- **Several projects sharing one disc series.** Discs, sessions, coverage and
  mark-dead are all scoped to a project, so the catalog model would change at its
  foundations.
- **Roots as the subdirectories of a user-maintained directory of symlinks.** This
  needs no schema change, but it moves the membership rules onto the filesystem, out
  of the catalog.
- **Fill order by file mtime.** Immich mtimes often reflect the upload or import time,
  not the date taken, and an order that changes when a file is touched is harder to
  reason about.

## 3. Roots

### 3.1 CLI

`init` takes exactly one of:

- `--source PATH`, as today; or
- `--root NAME=PATH`, repeated, at least once.

```
diskrement init immich-family \
  --work-dir /data/ext/services/working-area/diskrement/immich-family --copies 2 \
  --root family/anna=/data/ext/services/immich/store/library/65966390-50d0-4bc1-9eac-f49b1d61f7dd \
  --root family/bela=/data/ext/services/immich/store/library/4efdf184-aa8f-4641-8695-cd74b7b084a5 \
  --root others/cecil=/data/ext/services/immich/store/library/48199862-ef6e-44bf-93a5-fb61f68a3453
```

`--source` and `--root` are mutually exclusive, and `init` requires one of them
(clap `ArgGroup`). A `--root` value splits at its first `=`: a name cannot contain
`=`, but a path can.

### 3.2 Names

A root name is a relative path of one or more components separated by `/`. It
becomes that root's directory on every disc. `RootName` is a validated newtype in
`diskrement-core`. A name is refused if:

- it is empty, or any component is empty (a leading, trailing or doubled `/`);
- any component is `.` or `..`;
- it contains NUL or `=`.

Anything else is accepted, including spaces and non-ASCII characters.

Across the set of roots, `init` also refuses:

- two equal names;
- a name that is a **component** prefix of another (`family` with `family/anna`).
  One root's files would land inside another's directory, and a path could match two
  roots (§5). `family` with `familyx` is fine.
- two roots with the same path, or one root inside another, compared after
  `std::fs::canonicalize`. Either would archive the same files twice.

A single root is allowed; it puts one library under a readable directory name.

### 3.3 Checks at `init`

Each root must exist and be on the same filesystem as the work dir, checked by the
existing `same_filesystem` call, once per root. The CoW probe runs once, against the
work dir, as today. After the checks, the project row and its roots are written in
one transaction.

### 3.4 Catalog: schema V4

```sql
create table project_root (
  project_id integer not null references project(id),
  name       text    not null,
  path       text    not null,
  unique (project_id, name)
)
```

A rooted project stores `''` in `project.source_root`. That column stays `not null`:
relaxing it would mean rebuilding `project`, which every other table references, to
gain nothing. A project with no roots has no `project_root` rows.

`project_root` is written only by `init`. Nothing updates or deletes its rows; adding,
renaming and removing roots are out of scope (§9).

### 3.5 Model

```rust
// diskrement-core
pub struct RootName(String);              // validated per §3.2
pub enum Sources {
    Single(PathBuf),
    Rooted(Vec<(RootName, PathBuf)>),     // sorted by name
}
```

`Sources::rooted(Vec<(RootName, PathBuf)>) -> Result<Sources, RootsError>` enforces
the name rules across the set (equal names, component prefixes) and sorts. It is pure,
so it lives in core and the CLI calls it. The path rules need the filesystem and are
checked by `Pipeline::init` (§3.3, §6).

`ProjectInit` and `Project` replace `source_root: String` with `sources: Sources`.
`Catalog::find_project` assembles `Sources` from the project row and its
`project_root` rows, so nothing outside the catalog store ever sees the `''`.

The disc's embedded `catalog.sqlite` is a copy of the catalog, so it carries
`project_root`. Every disc still describes its own layout.

## 4. Snapshot

`fs::snapshot::create` (and `create_with`) take `&Sources`.

- `Single(path)`: unchanged. `reflink_tree(path, work/snapshot)`.
- `Rooted(roots)`:
  1. if any root does not exist, fail with `SourceMissing(path)` for the first one in
     name order;
  2. remove a stale `work/snapshot`;
  3. create `work/snapshot`;
  4. for each root, in name order: create the parents of `snapshot/<name>` as plain
     directories, then `reflink_tree(path, snapshot/<name>)`.

`reflink_tree` is reused unchanged, so every cloned file still gets its source mtime
back (`File::set_modified`). The parent directories exist only in the snapshot. Their
mtimes and permissions mean nothing, and nothing reads them: the stager creates its
own directories (`burn/stager.rs`), and the scanner looks only at files.

If a clone fails partway through, the snapshot is left half-built. A single-source
clone already behaves this way, and the next `plan` removes it. `plan` maps a
snapshot failure to `Failed` (exit 5), as today.

The scanner is unchanged. It walks `work/snapshot`. Files outside every root (the
`.immich` marker beside the libraries, for instance) are not in the snapshot, so they
are never seen.

A root that disappears after `init` (a deleted Immich account) fails `plan` with
`SourceMissing`, naming it. It is never read as every one of that root's files having
been deleted.

## 5. Fill order

```rust
// diskrement-core
pub enum FillOrder {
    Path,                   // a plain project: the full rel_path
    Rooted(Vec<RootName>),  // (path below the root, root name)
}
impl From<&Sources> for FillOrder { … }
impl FillOrder { pub fn key<'a>(&self, p: &'a RelPath) -> (&'a str, &'a str); }
```

- `Path`: the key is `(rel_path, "")`, which orders exactly like today's
  `rel_path.cmp`.
- `Rooted`: find the root whose name is a component prefix of the path (`name` plus
  `/`). §3.2 guarantees at most one. The key is `(rest of the path, name)`. If no root
  matches, the key is `(rel_path, "")`. A rooted snapshot cannot produce such a
  path, but the order stays total without a panic.

Keys compare byte-wise, like `String`. Two different paths never share a key, so the
order is total and the planner stays deterministic and independent of input order.

`planner::plan_set` takes `&FillOrder` and sorts by `order.key`. The comparator is the
only line in the fill loop that changes. `planning::plan_from` takes the order and
passes the same one to every copy set, so twin discs still get identical contents.
`planner::plan` (set A only) uses `FillOrder::Path`. `Pipeline::plan_scanned` builds
the order from `Project::sources`.

The `oversized` and `unplaceable_catch_up` reports stay sorted by `rel_path`. They are
lists for people to read, not a fill order.

What the order controls: each cycle writes at most one disc per set, so the order
decides which unburned files reach that disc first. On the first cycle, those are the
oldest files across all roots. Later, new files top off the open disc and spill onto
the next one in the same order. Files already burned never move.

Grouping in names (`family/…`, `others/…`) shapes the directory tree only. All roots
interleave into one date sequence regardless of group.

## 6. Errors

Exit codes stay within the contract in `error.rs`.

| Case | Caught by | Exit |
|---|---|---|
| `--source` with `--root`, neither given, `--root` value without `=` | clap | 1 |
| Bad name syntax (§3.2) | clap `value_parser` on `--root` | 1 |
| Duplicate names; a name a component prefix of another | CLI after parsing, a clap `ValueValidation` error, before the catalog opens | 1 |
| Missing root at `init` | `same_filesystem` → `Failed`, as `--source` today | 5 |
| Root on a different filesystem from the work dir | `NotSameFilesystem { src: <that root>, work }` | 3 |
| Two roots with the same canonical path, or nested | new `PipelineError::OverlappingRoots { a, b }` | 3 |
| Missing root at `plan` | `SourceMissing(path)` → `Failed`, as today | 5 |

Name problems are mistakes in the command line, so they are usage errors.
Overlapping roots depend on the filesystem (symlinks, bind mounts), so they are
environment errors and are checked in `Pipeline::init`.

## 7. Output and docs

- `init` prints a rooted project's roots as `name → path` lines in place of the
  single source line.
- `--help` documents `--root NAME=PATH` and says it is exclusive with `--source`.
- CLAUDE.md gains:
  - an invariant: a rooted project's snapshot is composed from its roots under their
    names, and the names are fixed at `init`;
  - a gotcha: never order files for placement by `rel_path` alone; use
    `FillOrder::key`.

## 8. Tests

**core**
- `RootName`: every refusal in §3.2, and acceptance of spaces, non-ASCII and
  multi-component names.
- `FillOrder::key`: four roots interleave by date; roots at different depths
  (`family/anna` with `cecil`); a tie on the path below the root falls back to root
  name; a path under no root; `family` does not match `familyx/…`.
- Planner: the existing example tests run unchanged with `FillOrder::Path`. The
  proptest "placed files are in lexicographic order across discs" becomes "…in
  `order.key` order" and runs under both orders. A new proptest checks that
  shuffling the input under `Rooted` gives the same plan.

**shell**
- Catalog: V4 migrates a copy of each frozen V2 fixture (copied to a `TempDir`, never
  opened in place) and a V3 catalog. `project_root` rows round-trip through
  `find_project`. A plain project reads back as `Sources::Single`.
- Snapshot: a composed snapshot with nested names creates the parent directories,
  places each root's files under its name, and keeps file mtimes. A missing root fails
  with its path.
- Pipeline: `init` refuses overlapping roots (`OverlappingRoots`) and a root on
  another filesystem. A rooted `copies = 2` project of three roots runs
  `plan → burn → verify` on the simulated burner. Disc 1A's tree has the root
  directories at its top level and the oldest files across roots, in key order. 1B's
  files equal 1A's. A missing root at `plan` names its path.

**cli**
- `binary.rs`: every row of §6's table, and `init` output listing the roots.
- `tests/scenarios/`: one end-to-end scenario for a rooted project, gated on a CoW
  `TMPDIR` like the others.

## 9. Out of scope

- Adding, renaming or removing roots after `init`. A rename would read as deleting
  and adding every file under the old name, so it needs a "this is a move" path of
  its own.
- Fill order by mtime, or any order configurable per project.
- Any change to projects created with `--source`.

## 10. Rollout

`immich-dsz` stays a plain project. Its catalog migrates to V4 the next time a
command opens it; the migration adds an empty table. Its next plan is the plan it
would have produced before. The family libraries become a new project with its own
catalog and work dir.
