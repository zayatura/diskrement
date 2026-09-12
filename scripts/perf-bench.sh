#!/usr/bin/env bash
# perf-bench.sh — end-to-end diskrement performance benchmark.
#
#   scripts/perf-bench.sh SOURCE_DIR WORK_AREA [BIN]
#
# Runs init -> scan -> plan -> burn (simulated) -> scan against SOURCE_DIR
# using a throwaway project under WORK_AREA (same filesystem as SOURCE_DIR).
# Staging uses CoW reflinks there -- init already enforces same-filesystem +
# reflink capability -- so the real space requirement is metadata-only.
# ~25 GB is only the worst case if reflinks are unavailable and burn silently
# falls back to a plain copy; a free-space guard below checks for a sane
# minimum, not 25 GB. SOURCE_DIR is only ever read.
#
# Reports per step: wall time, and three I/O columns from /proc/<pid>/io:
#   - rchar:       bytes the process actually read() from files, cache or
#                  not. HasherLive reads via Files.newInputStream (no mmap),
#                  so this is an exact, cache-independent count of file bytes
#                  the hasher pulled -- the metric that proves "we did not
#                  read the files". It has a nonzero floor (jars, the sqlite
#                  catalog, the staged scan.json, the fingerprint sidecar),
#                  so a near-zero scan2 should read as "<< source size", not
#                  "== 0".
#   - read_bytes:  actual block-device reads only; 0 on a page-cache hit.
#                  Kept for comparability with the recorded 388.6 GB rescan
#                  baseline, but uninterpretable on its own when the source
#                  tree is already warm in cache (e.g. burn re-hashing what
#                  scan1 just read).
#   - write_bytes: near-zero with reflink staging; ~= source size if burn
#                  silently fell back to a plain copy.
# Also records page-cache state (Cached, from /proc/meminfo) per step and
# total RAM once, so a reader can judge cache influence. Set DROP_CACHES=1
# to attempt `sync` + drop_caches via passwordless sudo before scan1; if
# that's unavailable the script warns and continues without dropping.
#
# Cleans up the throwaway project it created. Per-step stdout/stderr logs
# are kept (outside the throwaway dir) and their path is printed at the end.
#
# Dependencies beyond bash itself: coreutils (date, mv, df, cat, rm, mkdir),
# awk, grep, pgrep (procps), findmnt (util-linux), and the GNU-specific
# `find -printf`, `df --output`, and `date +%s.%N` -- this script has not
# been tested against BSD/macOS equivalents of these tools.
set -euo pipefail

# --- args -------------------------------------------------------------
if [[ $# -lt 2 || $# -gt 3 ]]; then
  echo "usage: $0 SOURCE_DIR WORK_AREA [BIN]" >&2
  exit 2
fi

SOURCE_DIR_ARG=$1
WORK_AREA_ARG=$2

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
DEFAULT_BIN="$SCRIPT_DIR/../target/release/diskrement"
BIN_ARG="${3:-$DEFAULT_BIN}"

if [[ ! -d "$SOURCE_DIR_ARG" ]]; then
  echo "error: SOURCE_DIR '$SOURCE_DIR_ARG' is not a directory" >&2
  exit 2
fi
SOURCE_DIR="$(cd -- "$SOURCE_DIR_ARG" && pwd)"

if [[ ! -x "$BIN_ARG" || -d "$BIN_ARG" ]]; then
  echo "error: BIN '$BIN_ARG' is not an executable file (did you run 'cargo build --release -p diskrement-cli'?)" >&2
  exit 2
fi
BIN="$(cd -- "$(dirname -- "$BIN_ARG")" && pwd)/$(basename -- "$BIN_ARG")"

if [[ ! -d "$WORK_AREA_ARG" ]]; then
  echo "error: WORK_AREA '$WORK_AREA_ARG' is not a directory" >&2
  exit 2
fi
WORK_AREA="$(cd -- "$WORK_AREA_ARG" && pwd)"

# --- I5: reject equal/overlapping SOURCE_DIR and WORK_AREA ---------------
# WORK_AREA inside SOURCE_DIR would put the throwaway dir, snapshot clone,
# and staged disc inside the photo library -- and then `rm -rf` there.
if [[ "$SOURCE_DIR" == "$WORK_AREA" ]]; then
  echo "error: SOURCE_DIR and WORK_AREA must not be the same directory ('$SOURCE_DIR')" >&2
  exit 2
fi
if [[ "$WORK_AREA/" == "$SOURCE_DIR"/* ]]; then
  echo "error: WORK_AREA ('$WORK_AREA') is inside SOURCE_DIR ('$SOURCE_DIR'); refusing (rm -rf would run inside the source tree)" >&2
  exit 2
fi
if [[ "$SOURCE_DIR/" == "$WORK_AREA"/* ]]; then
  echo "error: SOURCE_DIR ('$SOURCE_DIR') is inside WORK_AREA ('$WORK_AREA'); refusing" >&2
  exit 2
fi

# --- I4: free-space sanity guard ------------------------------------------
# Reflink staging needs metadata space only (init enforces same-fs + CoW);
# this is a floor for the throwaway project's *metadata* -- snapshot
# records, scan.json, and the sqlite catalog (plus its burn-catalog.sqlite
# copy) for every file in SOURCE_DIR, which adds up on a 75k-file tree --
# not a reservation for the staged disc itself (which reflinks) or for the
# 25 GB plain-copy-fallback worst case.
MIN_FREE_MB=2000
avail_kb="$(df --output=avail -k "$WORK_AREA" 2>/dev/null | tail -n1 | tr -d '[:space:]')" || avail_kb=""
if [[ -z "$avail_kb" || ! "$avail_kb" =~ ^[0-9]+$ ]]; then
  echo "warning: could not determine free space on WORK_AREA ('$WORK_AREA'); continuing" >&2
else
  avail_mb=$(( avail_kb / 1024 ))
  if (( avail_mb < MIN_FREE_MB )); then
    echo "error: WORK_AREA ('$WORK_AREA') has only ${avail_mb} MB free; need at least ${MIN_FREE_MB} MB (floor for project metadata -- snapshot records, scan.json, and the sqlite catalog for every source file, not the staged disc itself, which reflinks; ~25 GB would only matter on a silent plain-copy fallback)" >&2
    exit 2
  fi
fi

# --- throwaway project layout ------------------------------------------
PROJECT="bench"
CATALOG=""
THROWAWAY=""
CHILD_PID=""

# M3: cleanup runs under `set -e`; every removal below must be tolerant of
# failure so it never masks the real exit code by dying before `exit "$ec"`.
cleanup() {
  local ec=$?
  # I3: never leave an orphaned JVM writing into a tree we're about to rm -rf.
  if [[ -n "$CHILD_PID" ]]; then
    kill "$CHILD_PID" 2>/dev/null || true
    wait "$CHILD_PID" 2>/dev/null || true
    CHILD_PID=""
  fi
  if [[ -n "$CATALOG" && -x "$BIN" ]]; then
    "$BIN" --catalog "$CATALOG" reset "$PROJECT" >/dev/null 2>&1 || true
  fi
  if [[ -n "$THROWAWAY" ]]; then
    rm -rf -- "$THROWAWAY" || true
  fi
  if [[ -n "${LOG_DIR:-}" ]]; then
    echo "logs retained at: $LOG_DIR" >&2
  fi
  exit "$ec"
}
trap cleanup EXIT
# I3: SIGTERM/SIGHUP don't propagate to the child the way SIGINT (process
# group) does; route them through `exit` so the EXIT trap's kill+wait runs.
trap 'exit 130' INT
trap 'exit 143' TERM
trap 'exit 129' HUP

THROWAWAY="$WORK_AREA/perf-bench-$$"
PROJECT_WORK_DIR="$THROWAWAY/work"
CATALOG_DIR="$THROWAWAY/catalog"
CATALOG="$CATALOG_DIR/bench.sqlite"
# I2: logs are the only phase breakdown available; keep them outside the
# throwaway dir and never delete them.
LOG_DIR="$WORK_AREA/perf-bench-$$-logs"
mkdir -p -- "$PROJECT_WORK_DIR" "$CATALOG_DIR" "$LOG_DIR"

GLOBAL_ARGS=(--burner simulated --catalog "$CATALOG")

# --- helpers -------------------------------------------------------------

bytes_to_gb() {
  awk -v b="$1" 'BEGIN{printf "%.3f", b/1000000000}'
}

# Current page-cache size in kB, from /proc/meminfo. Always prints a number.
get_cached_kb() {
  awk '/^Cached:/ {c=$2} END {print c+0}' /proc/meminfo 2>/dev/null || echo 0
}

# rchar/read_bytes/write_bytes for pid, from /proc/<pid>/io, as one
# space-separated line. Empty (and non-zero return) on any failure
# (permission denied, pid gone, partial read) -- callers treat that as
# "no sample".
get_io_snapshot() {
  local pid="$1" f
  f="/proc/$pid/io"
  [[ -r "$f" ]] || return 1
  awk '
    /^rchar:/       {r=$2}
    /^read_bytes:/  {rb=$2}
    /^write_bytes:/ {wb=$2}
    END { if (r=="" || rb=="" || wb=="") exit 1; print r, rb, wb }
  ' "$f" 2>/dev/null
}

# Identify the process to sample among $1 (the launcher's pid) and its
# process tree. Two shapes: the Scala launcher execs java, so root_pid
# usually *is* the JVM and its cmdline names the main class; the Rust
# binary is root_pid itself and its cmdline names $BIN. Match either.
resolve_target_pid() {
  local root_pid="$1" cmdline
  if [[ -r "/proc/$root_pid/cmdline" ]]; then
    cmdline="$(tr '\0' ' ' < "/proc/$root_pid/cmdline" 2>/dev/null)" || cmdline=""
    if grep -qF -e 'diskrement.cli.Main' -e "$BIN" <<< "$cmdline"; then
      echo "$root_pid"
      return 0
    fi
  fi
  local found
  found="$(pgrep -f 'diskrement\.cli\.Main' 2>/dev/null | head -n1)" || true
  if [[ -n "$found" ]]; then
    echo "$found"
    return 0
  fi
  return 1
}

# sample_io PID OUTFILE — background sampler (C1+C3). Polls PID's I/O
# counters every 0.2s and OVERWRITES outfile with the latest single-line
# "rchar read_bytes write_bytes" triple. These counters are monotonically
# non-decreasing, so the last sample *taken* is the largest sample seen --
# but it is NOT the process's final value: whatever the process reads or
# writes after the last poll (up to one full 0.2s interval, in bytes) is
# never captured, because no sample is attempted after the pid exits
# (reading /proc/<pid>/io after the pid is gone reliably fails, which was
# the source of the old dead final-sample code). On a real run (tens of
# thousands of files spread over minutes) that residual final-interval gap
# is a tiny fraction of the total and is acceptable. On a small/fast
# fixture -- anything finishing in a second or two -- the last 0.2s can be
# a large fraction of the whole run, and rchar/write_bytes will badly
# under-report; do not use a small fixture to validate this instrument or
# to draw conclusions from its numbers.
sample_io() {
  local pid="$1" outfile="$2" target_pid="" snap
  : > "$outfile"
  while kill -0 "$pid" 2>/dev/null; do
    if [[ -z "$target_pid" ]]; then
      target_pid="$(resolve_target_pid "$pid" 2>/dev/null)" || target_pid=""
    fi
    if [[ -n "$target_pid" ]]; then
      snap="$(get_io_snapshot "$target_pid")" && [[ -n "$snap" ]] \
        && printf '%s\n' "$snap" > "$outfile.tmp" && mv -f "$outfile.tmp" "$outfile"
    fi
    sleep 0.2
  done
}

# run_step NAME ARG...  — launches "$BIN" ARG... in the background, waits on
# it in the foreground (so wall time is timestamped immediately on exit, not
# quantized to a polling interval -- C3), while a background sampler (C1)
# tracks I/O independently. Prints one aligned report line with all three
# I/O columns (C2+M4) plus page-cache state (I1). On non-zero exit it dumps
# the step's stdout+stderr and aborts the whole script.
run_step() {
  local name="$1"; shift
  local out="$LOG_DIR/$name.stdout" err="$LOG_DIR/$name.stderr" io_file="$LOG_DIR/$name.io"
  : > "$out"; : > "$err"

  local start end elapsed code=0 cache_after_kb
  local rchar=0 read_bytes=0 write_bytes=0

  start="$(date +%s.%N)"
  "$BIN" "$@" >"$out" 2>"$err" &
  local child=$!
  CHILD_PID=$child

  sample_io "$child" "$io_file" &
  local sampler=$!

  wait "$child" || code=$?
  end="$(date +%s.%N)"

  kill "$sampler" 2>/dev/null || true
  wait "$sampler" 2>/dev/null || true
  CHILD_PID=""

  cache_after_kb="$(get_cached_kb)"

  if [[ -s "$io_file" ]]; then
    read -r rchar read_bytes write_bytes < "$io_file" || true
  fi

  elapsed="$(awk -v a="$start" -v b="$end" 'BEGIN{printf "%.3f", b-a}')"
  local rchar_gb read_gb write_gb cache_gb
  rchar_gb="$(bytes_to_gb "$rchar")"
  read_gb="$(bytes_to_gb "$read_bytes")"
  write_gb="$(bytes_to_gb "$write_bytes")"
  cache_gb="$(bytes_to_gb "$(( cache_after_kb * 1024 ))")"

  printf '%-8s %9s s   rchar=%9s GB   read_bytes=%9s GB   write_bytes=%9s GB   cached=%9s GB   exit=%s\n' \
    "$name" "$elapsed" "$rchar_gb" "$read_gb" "$write_gb" "$cache_gb" "$code"

  if [[ "$code" -ne 0 ]]; then
    echo "===== step '$name' FAILED (exit $code) =====" >&2
    echo "--- stdout ---" >&2
    cat -- "$out" >&2
    echo "--- stderr ---" >&2
    cat -- "$err" >&2
    exit "$code"
  fi
}

# I1: attempt to drop the page cache before scan1, if requested.
CACHE_DROP_STATUS="not requested (DROP_CACHES not set)"
drop_caches_if_requested() {
  if [[ "${DROP_CACHES:-0}" != "1" ]]; then
    return 0
  fi
  echo "DROP_CACHES=1: syncing and attempting to drop the page cache before scan1..." >&2
  sync
  if sudo -n sh -c 'echo 3 > /proc/sys/vm/drop_caches' 2>/dev/null; then
    CACHE_DROP_STATUS="dropped (sudo -n echo 3 > /proc/sys/vm/drop_caches succeeded)"
    echo "page cache dropped." >&2
  else
    CACHE_DROP_STATUS="requested but NOT dropped (no passwordless sudo for drop_caches)"
    echo "warning: DROP_CACHES=1 requested but dropping caches failed (no passwordless sudo?); continuing without dropping" >&2
  fi
}

# --- M6: provenance header -------------------------------------------------
print_provenance() {
  local repo_dir commit kernel ram_kb ram_gb fs_info jvm_ver rustc_ver
  repo_dir="$(cd -- "$SCRIPT_DIR/.." && pwd)"
  commit="$(git -C "$repo_dir" rev-parse --short HEAD 2>/dev/null)" || commit="unknown"
  kernel="$(uname -r)"
  ram_kb="$(awk '/^MemTotal:/ {print $2}' /proc/meminfo)" || true
  ram_gb="$(awk -v kb="${ram_kb:-0}" 'BEGIN{printf "%.1f", kb/1024/1024}')"
  fs_info="$(findmnt -no FSTYPE,OPTIONS -T "$WORK_AREA" 2>/dev/null)" || fs_info="unknown"
  if command -v java >/dev/null 2>&1; then
    jvm_ver="$(java -version 2>&1 | head -n1)"
  else
    jvm_ver="unknown (no 'java' on PATH)"
  fi

  echo "provenance:"
  echo "  commit:       $commit"
  echo "  kernel:       $kernel"
  echo "  ram:          ${ram_gb} GB"
  echo "  work area fs: $fs_info"
  echo "  jvm:          $jvm_ver"
  if command -v rustc >/dev/null 2>&1; then
    rustc_ver="$(rustc --version 2>&1 | head -n1)"
  else
    rustc_ver="unknown (no 'rustc' on PATH)"
  fi
  echo "  rustc:        $rustc_ver"

  echo "  source tree:  counting files (single find+awk pass; may take a few seconds on large trees)..." >&2
  local file_count total_bytes total_gb
  read -r file_count total_bytes < <(find "$SOURCE_DIR" -type f -printf '%s\n' | awk '{n++; s+=$1} END {print n+0, s+0}')
  total_gb="$(bytes_to_gb "$total_bytes")"
  echo "  source tree:  $file_count files, ${total_gb} GB"
}

# --- run the cycle ---------------------------------------------------
echo "diskrement performance benchmark"
echo "  source:  $SOURCE_DIR"
echo "  bin:     $BIN"
echo "  project: $THROWAWAY"
echo "  logs:    $LOG_DIR"
echo
print_provenance
echo

run_step init  "${GLOBAL_ARGS[@]}" init "$PROJECT" --source "$SOURCE_DIR" --work-dir "$PROJECT_WORK_DIR"
drop_caches_if_requested
run_step scan1 "${GLOBAL_ARGS[@]}" scan "$PROJECT"
run_step plan  "${GLOBAL_ARGS[@]}" plan "$PROJECT"
run_step burn  "${GLOBAL_ARGS[@]}" burn "$PROJECT" --yes
run_step scan2 "${GLOBAL_ARGS[@]}" scan "$PROJECT"

echo
echo "page cache: $CACHE_DROP_STATUS"
echo
echo "cache evidence (P2: scan2 should show ~0 hashed):"
echo -n "  scan1: "; grep '^Hashed' "$LOG_DIR/scan1.stdout" || echo "(no Hashed line found)"
echo -n "  scan2: "; grep '^Hashed' "$LOG_DIR/scan2.stdout" || echo "(no Hashed line found)"
echo
echo "note: rchar has a nonzero floor (jars, sqlite catalog, staged scan.json,"
echo "      fingerprint sidecar) -- read scan2's rchar as '<< source size', not '== 0'."
echo "note: the I/O sampler polls every 0.2s and cannot see any I/O the process"
echo "      does after its last poll before exiting -- on a small/fast fixture"
echo "      (a run finishing in a second or two) that residual can be a large"
echo "      fraction of the total and rchar/write_bytes will badly under-report;"
echo "      trust these numbers only on runs long enough to span many polls."
