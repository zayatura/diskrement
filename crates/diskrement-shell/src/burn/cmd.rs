//! The one place a child process is spawned (`CmdRunner.scala`).
//!
//! Everything that decides anything lives in `xorriso::args` and
//! `xorriso::parse`; what is left here is the edge (spec §5.2). A nonzero exit
//! is a normal `CmdOutput`, never an error — only a failure to *launch* is an
//! error, because only that means the operator has something to install.

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

use crate::burn::BurnError;

/// One finished command: its exit code and its combined output lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CmdOutput {
    pub exit_code: i32,
    pub lines: Vec<String>,
}

impl CmdOutput {
    /// The last `n` lines — what a `BurnFailed` message quotes
    /// (`CmdResult.tail`, whose Scala default is 10).
    pub fn tail(&self, n: usize) -> Vec<String> {
        self.lines[self.lines.len().saturating_sub(n)..].to_vec()
    }
}

/// Running one external command.
///
/// A trait rather than a bare function so unit tests can substitute canned
/// output for states real `xorriso` cannot be talked into producing — an old
/// version, a permission denial, a post-burn identity mismatch. The blanket
/// impl below means a test writes a closure, never a struct.
pub trait Exec {
    fn run(
        &self,
        command: &[String],
        on_line: &mut dyn FnMut(&str),
    ) -> Result<CmdOutput, BurnError>;

    /// `run` for the callers that have no progress to report.
    fn run_quiet(&self, command: &[String]) -> Result<CmdOutput, BurnError> {
        self.run(command, &mut |_| {})
    }
}

impl<F> Exec for F
where
    F: Fn(&[String], &mut dyn FnMut(&str)) -> Result<CmdOutput, BurnError>,
{
    fn run(
        &self,
        command: &[String],
        on_line: &mut dyn FnMut(&str),
    ) -> Result<CmdOutput, BurnError> {
        self(command, on_line)
    }
}

/// Every line of `reader`, decoded lossily, until EOF or a read error.
///
/// Not `BufRead::lines`, which errors on the first non-UTF-8 line: stopping
/// there would leave the pipe undrained, and a child that keeps writing would
/// block on it forever.
fn lossy_lines(reader: impl std::io::Read) -> impl Iterator<Item = String> {
    BufReader::new(reader)
        .split(b'\n')
        .map_while(Result::ok)
        .map(|mut bytes| {
            if bytes.last() == Some(&b'\r') {
                bytes.pop();
            }
            String::from_utf8_lossy(&bytes).into_owned()
        })
}

/// The production `Exec`: spawn, stream, wait.
///
/// stdout and stderr share one pipe, the port of
/// `ProcessBuilder.redirectErrorStream(true)`: every line reaches `on_line` as
/// the child writes it, in the child's order. That matters because xorriso
/// writes its `UPDATE` progress to stderr.
pub fn system(command: &[String], on_line: &mut dyn FnMut(&str)) -> Result<CmdOutput, BurnError> {
    let (program, args) = command
        .split_first()
        .expect("an argument vector always names its program");

    let (reader, writer) = std::io::pipe().map_err(BurnError::Io)?;
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(writer.try_clone().map_err(BurnError::Io)?)
        .stderr(writer);
    let spawned = cmd.spawn();
    // The `Command` holds the parent's copies of the write end; until they
    // close, the reader never sees EOF.
    drop(cmd);
    let mut child = spawned.map_err(|e| match e.kind() {
        // `BurnError.ToolMissing`: the Scala matched the IOException's
        // English text for "no such file"; an ErrorKind cannot drift with
        // a locale or a JDK message change.
        std::io::ErrorKind::NotFound => {
            BurnError::ToolMissing(format!("cannot launch {program}: {e}"))
        }
        _ => BurnError::Io(e),
    })?;

    // Read on this thread, so `on_line` (which the caller may borrow mutably)
    // never crosses a thread boundary.
    let mut lines = Vec::new();
    for line in lossy_lines(reader) {
        on_line(&line);
        lines.push(line);
    }

    let status = child.wait().map_err(BurnError::Io)?;
    // A signal-killed child has no code. -1 is not a valid exit code, so it
    // can never collide with one xorriso actually returned.
    Ok(CmdOutput {
        exit_code: status.code().unwrap_or(-1),
        lines,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both streams are captured, a nonzero exit is a value and not an error,
    /// and every line reaches the callback as well as the buffer
    /// (`redirectErrorStream(true)` + the `onLine` hook).
    #[test]
    fn captures_both_streams_and_returns_a_nonzero_exit_as_a_value() {
        let mut seen = Vec::new();
        let out = system(
            &[
                "/bin/sh".to_string(),
                "-c".to_string(),
                "echo to-stdout; echo to-stderr 1>&2; exit 3".to_string(),
            ],
            &mut |line| seen.push(line.to_string()),
        )
        .expect("the process launched");

        assert_eq!(out.exit_code, 3);
        let mut lines = out.lines.clone();
        lines.sort();
        assert_eq!(
            lines,
            vec!["to-stderr".to_string(), "to-stdout".to_string()]
        );
        seen.sort();
        assert_eq!(seen, lines, "every captured line also reached the callback");
    }

    /// A line that is not UTF-8 (a file name xorriso echoes, say) is decoded
    /// lossily and reading carries on. Stopping at it would leave the pipe
    /// undrained, and a child that keeps writing would block on it forever.
    /// xorriso writes its `UPDATE` progress to stderr, so a stderr line has to
    /// reach the callback while the child is still running, not after it
    /// exits.
    #[test]
    fn a_stderr_line_reaches_the_callback_while_the_child_still_runs() {
        let started = std::time::Instant::now();
        let mut first_seen = None;
        system(
            &[
                "/bin/sh".to_string(),
                "-c".to_string(),
                "echo early 1>&2; sleep 3".to_string(),
            ],
            &mut |_| {
                first_seen.get_or_insert_with(|| started.elapsed());
            },
        )
        .expect("the process launched");
        let first_seen = first_seen.expect("the stderr line reached the callback");
        assert!(
            first_seen < std::time::Duration::from_secs(2),
            "the stderr line arrived only after the child exited ({first_seen:?})"
        );
    }

    /// One merged stream, in the order the child wrote it: `tail` in an error
    /// message then quotes the last lines the child actually printed.
    #[test]
    fn the_two_streams_arrive_in_the_order_the_child_wrote_them() {
        let mut seen = Vec::new();
        let out = system(
            &[
                "/bin/sh".to_string(),
                "-c".to_string(),
                "echo a; echo b 1>&2; echo c".to_string(),
            ],
            &mut |line| seen.push(line.to_string()),
        )
        .expect("the process launched");
        assert_eq!(out.lines, vec!["a", "b", "c"]);
        assert_eq!(seen, out.lines, "the callback saw the same order");
    }

    #[test]
    fn a_non_utf8_line_is_decoded_lossily_and_both_streams_keep_draining() {
        let out = system(
            &[
                "/bin/sh".to_string(),
                "-c".to_string(),
                r"printf 'a\n\377\nc\n'; printf 'x\n\376\nz\n' 1>&2".to_string(),
            ],
            &mut |_| {},
        )
        .expect("the process launched");
        let mut lines = out.lines.clone();
        lines.sort();
        assert_eq!(lines, vec!["a", "c", "x", "z", "\u{FFFD}", "\u{FFFD}"]);
    }

    /// A missing binary is `ToolMissing`, not an I/O error: the message tells
    /// the operator what to install (`BurnError.ToolMissing` in Burn.scala).
    #[test]
    fn a_missing_binary_is_tool_missing_and_names_the_command() {
        let err = system(
            &["definitely-not-a-real-binary-xyz".to_string()],
            &mut |_| {},
        )
        .expect_err("cannot launch");
        let msg = err.to_string();
        assert!(msg.contains("xorriso is not available"), "{msg}");
        assert!(
            msg.contains("cannot launch definitely-not-a-real-binary-xyz"),
            "{msg}"
        );
        assert!(msg.contains("libisoburn/xorriso"), "{msg}");
    }

    /// `CmdResult.tail(10)`: the last ten lines, or all of them when there are
    /// fewer. It is what every `BurnFailed` message quotes.
    #[test]
    fn tail_returns_the_last_lines_in_order() {
        let out = CmdOutput {
            exit_code: 5,
            lines: (0..12).map(|i| format!("line {i}")).collect(),
        };
        assert_eq!(out.tail(10).len(), 10);
        assert_eq!(out.tail(10)[0], "line 2");
        assert_eq!(out.tail(10)[9], "line 11");
        let short = CmdOutput {
            exit_code: 0,
            lines: vec!["only".to_string()],
        };
        assert_eq!(short.tail(10), vec!["only".to_string()]);
    }

    /// The blanket impl: any closure of the right shape is an `Exec`, which is
    /// what lets a test substitute canned output without a hand-written type.
    #[test]
    fn a_closure_is_an_exec() {
        let fake = |cmd: &[String], _: &mut dyn FnMut(&str)| {
            Ok(CmdOutput {
                exit_code: 0,
                lines: vec![cmd.join(" ")],
            })
        };
        let exec: &dyn Exec = &fake;
        let out = exec.run_quiet(&["xorriso".to_string(), "--version".to_string()]);
        assert_eq!(out.unwrap().lines, vec!["xorriso --version".to_string()]);
    }
}
