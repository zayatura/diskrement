//! `burn --verify`: read each disc back while it is still in the drive,
//! before it is recorded (burn-verify spec §5.1).

use diskrement_core::RelPath;

use crate::burn::verifier::{DiscExpectation, ReadBack, Verifier};
use crate::burn::{BurnError, BurnOperator, BurnResult, Burner, DiscImage};
use crate::error::PipelineError;

use super::{burner_failure, DiscFailure};

/// `burn --verify`'s collaborators: the verifier that matches the burner,
/// and the operator who reinserts or replaces a disc.
pub struct VerifyAfterBurn<'a> {
    pub verifier: &'a Verifier<'a>,
    pub operator: &'a dyn BurnOperator,
}

/// A disc written and read back, ready to be recorded.
pub(super) struct ReadBackBurn {
    pub burned: BurnResult,
    /// `Some(files)` when the read-back passed; `None` when the operator
    /// chose to record the disc unverified.
    pub verified_files: Option<usize>,
    /// Whether the disc has already left the drive.
    pub ejected: bool,
}

enum Settled {
    Passed(usize),
    Failed {
        files: usize,
        mismatches: Vec<RelPath>,
    },
    Unverified,
}

/// How many bad paths a failure line names before `…`.
const SHOWN_MISMATCHES: usize = 5;

/// Burn `image` and read it back until it passes, is left unverified, or
/// fails. A failed first session is offered again on a new blank disc,
/// through the burner's own prompt (declining stops with `VerifyFailed`). A
/// failed append stops at once: its session is on the disc for good.
pub(super) fn burn_and_read_back(
    burner: &Burner<'_>,
    step: &VerifyAfterBurn<'_>,
    image: &DiscImage,
    exp: &DiscExpectation,
) -> Result<ReadBackBurn, DiscFailure> {
    let session = image.metadata.identity.session_ordinal;
    let name = &exp.disc_name;
    let mut failed: Option<PipelineError> = None;
    loop {
        let burned = match (burner.burn(image), failed.take()) {
            (Ok(burned), _) => burned,
            // Declining the retry: the failed disc is the outcome.
            (Err(BurnError::Aborted), Some(err)) => return Err(DiscFailure::Unwritten(err)),
            (Err(e), _) => return Err(burner_failure(e)),
        };
        step.operator
            .progress(&format!("Reading disc {name} back before it is recorded."));
        match settle(burner, step, exp, session) {
            Settled::Passed(files) => {
                return Ok(ReadBackBurn {
                    burned,
                    verified_files: Some(files),
                    ejected: false,
                })
            }
            Settled::Unverified => {
                return Ok(ReadBackBurn {
                    burned,
                    verified_files: None,
                    ejected: true,
                })
            }
            Settled::Failed { files, mismatches } => {
                burner.eject();
                step.operator
                    .progress(&failure_line(name, files, &mismatches));
                let err = PipelineError::VerifyFailed {
                    disc: name.clone(),
                    session,
                    bad: mismatches.len(),
                    checked: files,
                    appended: session > 1,
                };
                if session > 1 {
                    return Err(DiscFailure::Unwritten(err));
                }
                step.operator.progress(&format!(
                    "To retry disc {name}, insert a new blank disc when asked; decline to stop."
                ));
                failed = Some(err);
            }
        }
    }
}

/// Read the disc back, asking for it again for as long as nothing can be
/// learned from it. It is ejected before every such request.
fn settle(
    burner: &Burner<'_>,
    step: &VerifyAfterBurn<'_>,
    exp: &DiscExpectation,
    session: u32,
) -> Settled {
    let name = &exp.disc_name;
    loop {
        let reason = match step.verifier.read_back(exp, session) {
            Ok(ReadBack::Passed { files }) => return Settled::Passed(files),
            Ok(ReadBack::Failed { files, mismatches }) => {
                return Settled::Failed { files, mismatches }
            }
            Ok(ReadBack::Unverifiable(reason)) => reason,
            // Not about the disc (spec §4.2): never a reason to discard it.
            Err(e) => crate::error::with_causes(&e),
        };
        burner.eject();
        if !step.operator.confirm_insert(&format!(
            "Couldn't verify disc {name}: {reason}. Reinsert disc {name} to verify it, \
             or decline to leave it unverified."
        )) {
            return Settled::Unverified;
        }
    }
}

/// `Disc 1A FAILED verification: 3 of 9042 files bad (a.jpg, b.mp4, …). It is
/// not recorded; discard it.`
fn failure_line(name: &str, files: usize, mismatches: &[RelPath]) -> String {
    let mut shown: Vec<&str> = mismatches
        .iter()
        .take(SHOWN_MISMATCHES)
        .map(RelPath::as_str)
        .collect();
    if mismatches.len() > SHOWN_MISMATCHES {
        shown.push("…");
    }
    format!(
        "Disc {name} FAILED verification: {} of {files} files bad ({}). \
         It is not recorded; discard it.",
        mismatches.len(),
        shown.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failure_line_names_at_most_five_paths() {
        let paths: Vec<RelPath> = (1..=7).map(|i| RelPath(format!("f{i}"))).collect();
        assert_eq!(
            failure_line("1A", 9042, &paths),
            "Disc 1A FAILED verification: 7 of 9042 files bad (f1, f2, f3, f4, f5, …). \
             It is not recorded; discard it."
        );
        assert_eq!(
            failure_line("2", 3, &paths[..1]),
            "Disc 2 FAILED verification: 1 of 3 files bad (f1). It is not recorded; discard it."
        );
    }
}
