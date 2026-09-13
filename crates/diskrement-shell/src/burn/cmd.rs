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
/// stdout is read on this thread and stderr on a second one; the output is
/// stdout's lines followed by stderr's, which stands in for what
/// `ProcessBuilder.redirectErrorStream(true)` merged in one call. The two
/// consumers of the order — `tail` in an error message and
/// `classify_failure`, which concatenates everything — are both insensitive to
/// how the two streams interleave.
pub fn system(command: &[String], on_line: &mut dyn FnMut(&str)) -> Result<CmdOutput, BurnError> {
    let (program, args) = command
        .split_first()
        .expect("an argument vector always names its program");

    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| match e.kind() {
            // `BurnError.ToolMissing`: the Scala matched the IOException's
            // English text for "no such file"; an ErrorKind cannot drift with
            // a locale or a JDK message change.
            std::io::ErrorKind::NotFound => {
                BurnError::ToolMissing(format!("cannot launch {program}: {e}"))
            }
            _ => BurnError::Io(e),
        })?;

    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");

    // stdout is read on this thread, so `on_line` (which the caller may
    // borrow mutably) never crosses a thread boundary. stderr drains
    // concurrently so neither pipe can fill and stall the child.
    let err_thread = std::thread::spawn(move || lossy_lines(stderr).collect::<Vec<_>>());

    let mut lines = Vec::new();
    for line in lossy_lines(stdout) {
        on_line(&line);
        lines.push(line);
    }
    let err_lines = err_thread.join().unwrap_or_default();

    // Replayed, not live: xorriso's `UPDATE` progress is on stdout, so the
    // only caller that streams as it appears loses nothing by the deferral.
    // The Scala runner merged the streams before reading, so its `onLine`
    // saw stderr too.
    for line in &err_lines {
        on_line(line);
    }
    lines.extend(err_lines);

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
