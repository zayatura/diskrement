//! The progress-reporting port (`ProgressSink.scala`). The pipeline reports
//! through this trait and stays free of console I/O; the CLI provides the
//! stderr renderer (cli/src/progress.rs) and tests use `NoopSink` or a
//! recording sink.

/// Implementations must be thread-safe and cheap: `advance` is called from
/// parallel hashing workers. `start` opens a phase; one instance may serve
/// several phases in sequence.
pub trait ProgressSink: Send + Sync {
    fn start(&self, label: &str, total_bytes: u64, total_files: usize);
    /// A phase measured in bytes alone, such as a burn: it has no file count
    /// to report.
    fn start_bytes(&self, label: &str, total_bytes: u64) {
        self.start(label, total_bytes, 0);
    }
    fn advance(&self, delta_bytes: u64, delta_files: usize);
    fn done(&self);
}

/// The test sink (`ProgressSink.Noop`).
pub struct NoopSink;

impl ProgressSink for NoopSink {
    fn start(&self, _label: &str, _total_bytes: u64, _total_files: usize) {}
    fn advance(&self, _delta_bytes: u64, _delta_files: usize) {}
    fn done(&self) {}
}

/// `start` … work … `done`, with `done` guaranteed on the failure path too
/// (`ProgressSink.phase`'s `ensuring`). Panics are not covered: a panic during
/// a scan is a bug that fails the process either way.
pub fn phase<T>(
    sink: &dyn ProgressSink,
    label: &str,
    total_bytes: u64,
    total_files: usize,
    work: impl FnOnce() -> T,
) -> T {
    sink.start(label, total_bytes, total_files);
    let out = work();
    sink.done();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Recording(Mutex<Vec<String>>);
    impl Recording {
        fn events(&self) -> Vec<String> {
            self.0.lock().unwrap().clone()
        }
    }
    impl ProgressSink for Recording {
        fn start(&self, label: &str, total_bytes: u64, total_files: usize) {
            self.0
                .lock()
                .unwrap()
                .push(format!("start:{label}:{total_bytes}:{total_files}"));
        }
        fn advance(&self, delta_bytes: u64, delta_files: usize) {
            self.0
                .lock()
                .unwrap()
                .push(format!("advance:{delta_bytes}:{delta_files}"));
        }
        fn done(&self) {
            self.0.lock().unwrap().push("done".to_string());
        }
    }

    #[test]
    fn phase_reports_start_then_done_on_success() {
        let sink = Recording::default();
        let out = phase(&sink, "hashing", 100, 2, || {
            sink.advance(60, 1);
            "ok"
        });
        assert_eq!(out, "ok");
        assert_eq!(
            sink.events(),
            vec!["start:hashing:100:2", "advance:60:1", "done"]
        );
    }

    #[test]
    fn phase_reports_done_even_when_the_work_fails() {
        let sink = Recording::default();
        let out: Result<(), &str> = phase(&sink, "hashing", 10, 1, || Err("boom"));
        assert!(out.is_err());
        assert_eq!(sink.events(), vec!["start:hashing:10:1", "done"]);
    }
}
