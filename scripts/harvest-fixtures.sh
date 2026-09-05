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
# Sums are captured to a variable before being written so the pipeline never
# reads and writes the same tree (shellcheck SC2094); `! -name SHA256SUMS`
# would already exclude the output, but the redirect creates the file before
# find finishes enumerating.
sums=$( cd "$OUTDIR" && find . -type f ! -name SHA256SUMS -print0 \
    | sort -z | xargs -0 sha256sum )
printf '%s\n' "$sums" > "$OUTDIR/SHA256SUMS"

echo "ok: harvested $STEP steps into $OUTDIR"
