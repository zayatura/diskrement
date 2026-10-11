//! The stderr progress reporter (`ConsoleProgress.scala`). Hand-rolled rather
//! than `indicatif`: CLI compatibility makes matching the existing behavior
//! worth more than the dependency saves (spec §5.5).
//!
//! Cadence: 1 s on a TTY, 10 s otherwise. Emission happens inside `advance`
//! (no background thread); `done` emits a final snapshot only if something
//! was already emitted. TTY frames are `"\r" + line + ESC[K` (erase to end of
//! line so a shrinking line leaves no residue); non-TTY frames are plain
//! newline-terminated lines so logs stay parseable.

use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use diskrement_shell::progress::ProgressSink;

/// `TERM=dumb` explicitly disables, then the stderr fd decides
/// (`ConsoleProgress.stderrIsTty`).
#[allow(dead_code)]
pub fn stderr_is_tty() -> bool {
    if std::env::var("TERM").as_deref() == Ok("dumb") {
        return false;
    }
    std::io::stderr().is_terminal()
}

/// The pinned line shape (`ConsoleProgress.format`):
/// `label X.X/Y.Y GB · N/M files · R MB/s · ETA …`.
#[allow(dead_code)]
pub fn format(
    label: &str,
    done_bytes: u64,
    total_bytes: u64,
    done_files: usize,
    total_files: usize,
    elapsed: Duration,
) -> String {
    let (rate, eta) = rate_and_eta(done_bytes, total_bytes, elapsed);
    format!(
        "{label} {:.1}/{:.1} GB · {}/{} files · {:.0} MB/s · {}",
        done_bytes as f64 / 1.0e9,
        total_bytes as f64 / 1.0e9,
        done_files,
        total_files,
        rate / 1.0e6,
        eta
    )
}

/// `format` without the files segment, for a phase measured in bytes alone:
/// `label X.X/Y.Y GB · R MB/s · ETA …`.
pub fn format_bytes(label: &str, done_bytes: u64, total_bytes: u64, elapsed: Duration) -> String {
    let (rate, eta) = rate_and_eta(done_bytes, total_bytes, elapsed);
    format!(
        "{label} {:.1}/{:.1} GB · {:.0} MB/s · {}",
        done_bytes as f64 / 1.0e9,
        total_bytes as f64 / 1.0e9,
        rate / 1.0e6,
        eta
    )
}

/// Bytes per second so far, and the ETA text that rate implies.
fn rate_and_eta(done_bytes: u64, total_bytes: u64, elapsed: Duration) -> (f64, String) {
    let seconds = elapsed.as_secs_f64();
    let rate = if seconds > 0.0 {
        done_bytes as f64 / seconds
    } else {
        0.0
    };
    let eta_seconds = if rate > 0.0 {
        (total_bytes.saturating_sub(done_bytes)) as f64 / rate
    } else {
        -1.0
    };
    let eta = if eta_seconds < 0.0 {
        "ETA --".to_string()
    } else {
        let s = eta_seconds as i64;
        format!("ETA {}m{:02}s", s / 60, s % 60)
    };
    (rate, eta)
}

pub struct ConsoleProgress {
    label: Mutex<String>,
    total_bytes: AtomicU64,
    total_files: AtomicU64,
    /// False for a `start_bytes` phase, which has no files segment.
    counts_files: AtomicBool,
    done_bytes: AtomicU64,
    done_files: AtomicU64,
    started: Mutex<Option<Instant>>,
    last_emit: Mutex<Option<Instant>>,
    emitted: AtomicBool,
    tty: bool,
}

impl ConsoleProgress {
    #[allow(dead_code)]
    pub fn new() -> Self {
        ConsoleProgress {
            label: Mutex::new(String::new()),
            total_bytes: AtomicU64::new(0),
            total_files: AtomicU64::new(0),
            counts_files: AtomicBool::new(true),
            done_bytes: AtomicU64::new(0),
            done_files: AtomicU64::new(0),
            started: Mutex::new(None),
            last_emit: Mutex::new(None),
            emitted: AtomicBool::new(false),
            tty: stderr_is_tty(),
        }
    }

    fn interval(&self) -> Duration {
        if self.tty {
            Duration::from_secs(1)
        } else {
            Duration::from_secs(10)
        }
    }

    /// The current phase's line; `None` before any phase has started.
    fn line(&self) -> Option<String> {
        let started = (*self.started.lock().unwrap())?;
        let label = self.label.lock().unwrap();
        let done_bytes = self.done_bytes.load(Ordering::Relaxed);
        let total_bytes = self.total_bytes.load(Ordering::Relaxed);
        Some(if self.counts_files.load(Ordering::Relaxed) {
            format(
                &label,
                done_bytes,
                total_bytes,
                self.done_files.load(Ordering::Relaxed) as usize,
                self.total_files.load(Ordering::Relaxed) as usize,
                started.elapsed(),
            )
        } else {
            format_bytes(&label, done_bytes, total_bytes, started.elapsed())
        })
    }

    fn emit(&self) {
        let Some(line) = self.line() else {
            return;
        };
        let mut err = std::io::stderr().lock();
        if self.tty {
            let _ = write!(err, "\r{line}\x1b[K");
            let _ = err.flush();
        } else {
            let _ = writeln!(err, "{line}");
        }
        *self.last_emit.lock().unwrap() = Some(Instant::now());
        self.emitted.store(true, Ordering::Relaxed);
    }

    /// Whether an `advance` is due to emit: once the phase has been running
    /// longer than the interval since the last emission (or the start).
    fn due(&self) -> bool {
        let last = *self.last_emit.lock().unwrap();
        let anchor = last.or(*self.started.lock().unwrap());
        match anchor {
            Some(t) => t.elapsed() >= self.interval(),
            None => false,
        }
    }
}

impl Default for ConsoleProgress {
    fn default() -> Self {
        Self::new()
    }
}

impl ProgressSink for ConsoleProgress {
    fn start(&self, label: &str, total_bytes: u64, total_files: usize) {
        *self.label.lock().unwrap() = label.to_string();
        self.total_bytes.store(total_bytes, Ordering::Relaxed);
        self.total_files
            .store(total_files as u64, Ordering::Relaxed);
        self.done_bytes.store(0, Ordering::Relaxed);
        self.done_files.store(0, Ordering::Relaxed);
        *self.started.lock().unwrap() = Some(Instant::now());
        *self.last_emit.lock().unwrap() = None;
        self.emitted.store(false, Ordering::Relaxed);
        self.counts_files.store(true, Ordering::Relaxed);
    }

    fn start_bytes(&self, label: &str, total_bytes: u64) {
        self.start(label, total_bytes, 0);
        self.counts_files.store(false, Ordering::Relaxed);
    }

    fn advance(&self, delta_bytes: u64, delta_files: usize) {
        self.done_bytes.fetch_add(delta_bytes, Ordering::Relaxed);
        self.done_files
            .fetch_add(delta_files as u64, Ordering::Relaxed);
        if self.due() {
            self.emit();
        }
    }

    fn done(&self) {
        // A final snapshot only if something was already emitted: done is
        // silent when the phase never crossed the first interval.
        if self.emitted.load(Ordering::Relaxed) {
            self.emit();
            if self.tty {
                let mut err = std::io::stderr().lock();
                let _ = writeln!(err);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn the_line_format_is_the_pinned_shape() {
        // Inputs chosen so every component is exact: rate = 12.3e9 /
        // 14_642_857_142 ns = exactly 840.0 MB/s; ETA = 401.3e9 / 840e6 =
        // 477.7 s -> `7m57s`.
        assert_eq!(
            format(
                "hashing",
                12_300_000_000,
                413_600_000_000,
                3_021,
                75_618,
                Duration::from_nanos(14_642_857_142)
            ),
            "hashing 12.3/413.6 GB · 3021/75618 files · 840 MB/s · ETA 7m57s"
        );
    }

    /// A burn advances by bytes only, so its line drops the files segment.
    #[test]
    fn a_bytes_only_line_has_no_files_segment() {
        assert_eq!(
            format_bytes(
                "burning disc 1A",
                12_300_000_000,
                413_600_000_000,
                Duration::from_nanos(14_642_857_142)
            ),
            "burning disc 1A 12.3/413.6 GB · 840 MB/s · ETA 7m57s"
        );
    }

    #[test]
    fn a_bytes_phase_renders_without_files_and_a_later_phase_with_them() {
        let p = ConsoleProgress::new();
        p.start_bytes("burning disc 1A", 1_000);
        p.advance(10, 0);
        let line = p.line().expect("a started phase renders");
        assert!(line.starts_with("burning disc 1A "), "{line}");
        assert!(!line.contains("files"), "{line}");

        p.start("staging", 1_000, 3);
        let line = p.line().expect("a started phase renders");
        assert!(line.contains("0/3 files"), "{line}");
    }

    #[test]
    fn zero_totals_render_the_parked_string() {
        assert_eq!(
            format("hashing", 0, 0, 0, 0, Duration::from_secs(0)),
            "hashing 0.0/0.0 GB · 0/0 files · 0 MB/s · ETA --"
        );
    }

    #[test]
    fn the_eta_uses_minutes_and_padded_seconds() {
        // rate = 999_000 B/s -> "1 MB/s"; remaining = 999_000 B -> ETA 1.0s
        // -> "0m01s".
        assert_eq!(
            format("hashing", 999_000, 1_998_000, 1, 1, Duration::from_secs(1)),
            "hashing 0.0/0.0 GB · 1/1 files · 1 MB/s · ETA 0m01s"
        );
    }
}
