#!/usr/bin/env bash
# perf-bench.sh — end-to-end diskrement performance benchmark.
#
#   scripts/perf-bench.sh SOURCE_DIR WORK_AREA [BIN]
#
# Runs init → scan → plan → burn (simulated) → scan against SOURCE_DIR using a
# throwaway project under WORK_AREA (same filesystem as SOURCE_DIR; needs free
# space for one staged disc, ~25 GB with default BD_R sizing). SOURCE_DIR is
# only ever read. Reports per-step wall time and the JVM's cumulative block
# reads. Cleans up everything it created.
set -euo pipefail

# --- args -------------------------------------------------------------
if [[ $# -lt 2 || $# -gt 3 ]]; then
  echo "usage: $0 SOURCE_DIR WORK_AREA [BIN]" >&2
  exit 2
fi

SOURCE_DIR_ARG=$1
WORK_AREA_ARG=$2

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
DEFAULT_BIN="$SCRIPT_DIR/../cli/target/universal/stage/bin/diskrement"
BIN_ARG="${3:-$DEFAULT_BIN}"

if [[ ! -d "$SOURCE_DIR_ARG" ]]; then
  echo "error: SOURCE_DIR '$SOURCE_DIR_ARG' is not a directory" >&2
  exit 2
fi
SOURCE_DIR="$(cd -- "$SOURCE_DIR_ARG" && pwd)"

if [[ ! -x "$BIN_ARG" || -d "$BIN_ARG" ]]; then
  echo "error: BIN '$BIN_ARG' is not an executable file (did you run 'sbt cli/stage'?)" >&2
  exit 2
fi
BIN="$(cd -- "$(dirname -- "$BIN_ARG")" && pwd)/$(basename -- "$BIN_ARG")"

if [[ ! -d "$WORK_AREA_ARG" ]]; then
  echo "error: WORK_AREA '$WORK_AREA_ARG' is not a directory" >&2
  exit 2
fi
WORK_AREA="$(cd -- "$WORK_AREA_ARG" && pwd)"

# --- throwaway project layout ------------------------------------------
PROJECT="bench"
CATALOG=""
THROWAWAY=""

cleanup() {
  local ec=$?
  if [[ -n "$CATALOG" && -x "$BIN" ]]; then
    "$BIN" --catalog "$CATALOG" reset "$PROJECT" >/dev/null 2>&1 || true
  fi
  if [[ -n "$THROWAWAY" ]]; then
    rm -rf -- "$THROWAWAY"
  fi
  exit "$ec"
}
trap cleanup EXIT

THROWAWAY="$WORK_AREA/perf-bench-$$"
PROJECT_WORK_DIR="$THROWAWAY/work"
CATALOG_DIR="$THROWAWAY/catalog"
CATALOG="$CATALOG_DIR/bench.sqlite"
LOG_DIR="$THROWAWAY/logs"
mkdir -p -- "$PROJECT_WORK_DIR" "$CATALOG_DIR" "$LOG_DIR"

GLOBAL_ARGS=(--burner simulated --catalog "$CATALOG")

# --- helpers -------------------------------------------------------------

# Cumulative bytes read by pid, from /proc/<pid>/io. Empty on any failure
# (permission denied, pid gone, etc.) — callers treat that as "no sample".
get_read_bytes() {
  local pid="$1" f
  f="/proc/$pid/io"
  [[ -r "$f" ]] || return 0
  awk '/^read_bytes:/ {print $2}' "$f" 2>/dev/null || true
}

# Identify the diskrement JVM among $1 (the launcher's pid) and its process
# tree. The staged launcher execs java, so root_pid usually *is* the JVM;
# fall back to a systemwide search by main class if that ever changes.
resolve_jvm_pid() {
  local root_pid="$1"
  if [[ -r "/proc/$root_pid/cmdline" ]] \
      && tr '\0' ' ' < "/proc/$root_pid/cmdline" 2>/dev/null | grep -q 'diskrement\.cli\.Main'; then
    echo "$root_pid"
    return 0
  fi
  local found
  found="$(pgrep -f 'diskrement\.cli\.Main' 2>/dev/null | head -n1)" || true
  if [[ -n "$found" ]]; then
    echo "$found"
    return 0
  fi
  return 1
}

# run_step NAME ARG...  — launches "$BIN" ARG..., polls its cumulative
# read_bytes at ~1 Hz, and prints one aligned report line. On non-zero exit
# it dumps the step's stdout+stderr and aborts the whole script.
run_step() {
  local name="$1"; shift
  local out="$LOG_DIR/$name.stdout" err="$LOG_DIR/$name.stderr"
  : > "$out"; : > "$err"

  local start end elapsed peak=0 jvm_pid="" bytes code=0

  start="$(date +%s.%N)"
  "$BIN" "$@" >"$out" 2>"$err" &
  local pid=$!

  while kill -0 "$pid" 2>/dev/null; do
    if [[ -z "$jvm_pid" ]]; then
      jvm_pid="$(resolve_jvm_pid "$pid" 2>/dev/null)" || jvm_pid=""
    fi
    if [[ -n "$jvm_pid" ]]; then
      bytes="$(get_read_bytes "$jvm_pid")" || bytes=""
      if [[ -n "$bytes" && "$bytes" -gt "$peak" ]]; then
        peak="$bytes"
      fi
    fi
    sleep 1
  done
  wait "$pid" || code=$?
  end="$(date +%s.%N)"

  # Last sample after the process is confirmed gone, in case it wrote its
  # final bytes between our last poll and exit.
  if [[ -n "$jvm_pid" ]]; then
    bytes="$(get_read_bytes "$jvm_pid")" || bytes=""
    if [[ -n "$bytes" && "$bytes" -gt "$peak" ]]; then
      peak="$bytes"
    fi
  fi

  elapsed="$(awk -v a="$start" -v b="$end" 'BEGIN{printf "%.1f", b-a}')"
  local gb; gb="$(awk -v b="$peak" 'BEGIN{printf "%.3f", b/1000000000}')"

  printf '%-8s %8s s   %10s GB read   exit=%s\n' "$name" "$elapsed" "$gb" "$code"

  if [[ "$code" -ne 0 ]]; then
    echo "===== step '$name' FAILED (exit $code) =====" >&2
    echo "--- stdout ---" >&2
    cat -- "$out" >&2
    echo "--- stderr ---" >&2
    cat -- "$err" >&2
    exit "$code"
  fi
}

# --- run the cycle ---------------------------------------------------
echo "diskrement performance benchmark"
echo "  source:  $SOURCE_DIR"
echo "  bin:     $BIN"
echo "  project: $THROWAWAY"
echo

run_step init  "${GLOBAL_ARGS[@]}" init "$PROJECT" --source "$SOURCE_DIR" --work-dir "$PROJECT_WORK_DIR"
run_step scan1 "${GLOBAL_ARGS[@]}" scan "$PROJECT"
run_step plan  "${GLOBAL_ARGS[@]}" plan "$PROJECT"
run_step burn  "${GLOBAL_ARGS[@]}" burn "$PROJECT" --yes
run_step scan2 "${GLOBAL_ARGS[@]}" scan "$PROJECT"

echo
echo "cache evidence (P2: scan2 should show ~0 hashed):"
echo -n "  scan1: "; grep '^Hashed' "$LOG_DIR/scan1.stdout" || echo "(no Hashed line found)"
echo -n "  scan2: "; grep '^Hashed' "$LOG_DIR/scan2.stdout" || echo "(no Hashed line found)"
