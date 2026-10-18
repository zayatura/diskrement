# diskrement

**Incremental backups to Blu-ray, with a memory.**

Optical discs are a good home for data you want to keep forever: they need no power,
nothing can rewrite them, and they outlast most hard drives. But burning tools treat
each disc as a one-off. They don't remember what you burned last month, so backing up a
growing directory (a photo library, say) across dozens of discs is left to you.

diskrement does that bookkeeping. It keeps a catalog of every file it has burned, and
each cycle it writes only what changed since the last one, topping off the current
multisession disc before starting a new one.

## Features

- **Incremental.** Each cycle scans the source, burns new and changed files, and records
  deletions. Files already on disc are not burned again.
- **Full history.** Every version of every file is kept in an append-only catalog, and
  `history` shows which disc holds each one.
- **Multiple copies.** `--copies 2` keeps every file on two independent disc series
  (1A/1B, 2A/2B, …), so you can store one set off-site.
- **Self-describing discs.** Files sit under `/data` on a plain ISO 9660 + Rock Ridge
  filesystem that any computer can read. Beside them, `/.diskrement` holds the disc's
  manifest and a snapshot of the catalog, so a single disc is enough to see what the
  whole archive held when it was burned.
- **Verified.** `burn --verify` reads each disc back before recording it, and `verify`
  re-hashes burned discs against the catalog at any time.
- **Unattended runs.** `run` works through a large backlog disc after disc: it waits for
  the right blank disc to be inserted, burns it, verifies it and ejects it.
- **Lost-disc recovery.** `disc mark-dead` writes off a broken or lost disc, reports what
  was lost for good, and re-burns everything that is still current on the next cycle.
- **Crash-safe.** The catalog is updated only after a disc is burned. An interrupted
  burn leaves it untouched, and running `burn` again resumes from the staged plan.

## Requirements

- Linux.
- [xorriso](https://www.gnu.org/software/xorriso/), and udisks2 for `verify`.
- A copy-on-write filesystem (btrfs, XFS with reflink, or bcachefs) holding both the
  source and the work directory. Each cycle takes an instant reflink snapshot of the
  source, so you can keep using your files while a burn runs.
- Permission to use the drive: on most distributions, membership in the `optical` group.
- A Blu-ray burner. BD-R (25 GB) is the default disc type; `BD_R_DL`, `DVD` and
  `DVD_DL` are also available via `--disc-type`.

## Installation

You need a Rust toolchain ([rustup](https://rustup.rs) picks up the pinned version) and a
C compiler, since SQLite is built from source.

```sh
git clone https://github.com/zayatura/diskrement
cd diskrement
cargo build --release
ln -s "$PWD/target/release/diskrement" ~/.local/bin/diskrement
```

The result is a single self-contained binary.

## Quick start

```sh
# Create a project: what to archive, and where to stage discs.
diskrement init photos --source /srv/photos --work-dir /srv/work

# Scan the source and plan the next disc.
diskrement plan photos

# Burn it, reading it back before it is recorded.
diskrement burn photos --verify

# See what is on disc and what is left.
diskrement status photos
```

Repeat `plan` and `burn` whenever the source changes. To burn a large backlog, use
`diskrement run photos` instead: it plans and burns cycle after cycle, starting each burn
as soon as the expected disc is in the drive, and stops once everything is on disc.

## Commands

| Command | What it does |
|---|---|
| `init NAME` | Create a project. `--source DIR` or `--root NAME=DIR` (repeatable), `--work-dir DIR`, `--copies N`, `--disc-type`, `--margin PERCENT` |
| `plan NAME` | Scan the source and stage the next disc(s). `--finalize` seals the open disc; `--rehash` ignores the fingerprint cache |
| `burn NAME` | Burn the staged plan. `--verify` reads each disc back first; `--yes` skips the confirmation |
| `run NAME` | Plan and burn repeatedly until the backlog is on disc, waiting for each disc |
| `status NAME` | Show discs, totals and the last burn |
| `verify NAME` | Mount burned discs and re-hash every file. `--disc N` checks just one |
| `history NAME PATH` | Show every version of a file and where each copy lives |
| `disc mark-dead NAME DISC` | Write off a lost or unreadable disc; its current files are re-burned |
| `reset NAME` | Discard the staged plan |

Run `diskrement <command> --help` for every option.

## Archiving several directories

A project can archive several directories onto one disc series, each under a name of
your choice:

```sh
diskrement init family --work-dir /srv/work \
  --root family/anna=/srv/immich/library/anna \
  --root family/bela=/srv/immich/library/bela
```

Files appear on disc under their root's name (`family/anna/2024/…`). Discs fill in order
of the path below each root, so libraries organized by date interleave: the first disc
holds the oldest files from every root. Roots are fixed at `init`, because renaming one
would look like deleting and re-adding everything under it.

## Lost or damaged discs

```sh
diskrement disc mark-dead photos 3
```

This records the disc as dead (the catalog keeps its history) and prints what that cost:
old versions that lived only on that disc are reported as lost, and files that are still
current are queued for re-burning. Then run `plan` and `burn` as usual. A dead disc is
never topped off and is skipped by `verify`.

## Configuration

| Variable | Purpose |
|---|---|
| `DISKREMENT_DEVICE` | Burner device, default `/dev/sr0`; a stable `/dev/disk/by-id/…` path also works (also `--device`) |
| `DISKREMENT_BURNER` | `real` (default) or `simulated`, which stages discs to directories instead of burning them (also `--burner`) |
| `DISKREMENT_HASH_PARALLELISM` | Hashing workers, default 6 |

Each project's catalog lives at `$XDG_DATA_HOME/diskrement/<name>.sqlite` (or under
systemd's `$STATE_DIRECTORY`); override it with `--catalog` or `--data-dir`. Next to it, a
disposable fingerprint cache lets later scans re-hash only files whose size or mtime
changed. Deleting the cache is always safe.

Progress and diagnostics go to stderr. Exit codes: `0` success (or a burn you declined),
`1` usage, `2` project state, `3` environment, `4` verification mismatch, `5` unexpected
error, `6` media or burner.

## License

[MIT](LICENSE)
