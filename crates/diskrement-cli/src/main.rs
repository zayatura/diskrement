//! The diskrement binary: resolve paths, dispatch, render, map errors to the
//! exit ladder. Path resolution and rendering live here; the pipeline returns
//! data and performs no path resolution (CLAUDE.md invariant).

mod args;
mod backend;
mod paths;
mod progress;
mod prompt;
mod render;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use args::Command;
use clap::Parser as _;
use diskrement_shell::burn::cmd::{self, CmdOutput, Exec};
use diskrement_shell::burn::verifier::{DiscVerifier, StagingVerifier, Verifier};
use diskrement_shell::burn::xorriso::burner::XorrisoBurner;
use diskrement_shell::burn::{simulated::SimulatedBurner, BurnError, Burner};
use diskrement_shell::catalog::model::{Project, ProjectInit};
use diskrement_shell::catalog::Catalog;
use diskrement_shell::error::PipelineError;
use diskrement_shell::pipeline::{Pipeline, PipelinePaths};
use diskrement_shell::progress::NoopSink;

fn main() -> ExitCode {
    // try_parse, never parse(): clap exits 2 on usage errors by default, but
    // the contract's 2 means state/lookup — zio-cli used 1 for argument
    // errors, so the port maps them to 1 itself. Only the *wording* of
    // clap's message may drift (spec §3.2).
    match args::Cli::try_parse() {
        Ok(cli) => match dispatch(cli) {
            Ok(code) => ExitCode::from(code),
            Err(err) => {
                eprintln!("{}", render::error_line(&err));
                ExitCode::from(err.exit_code() as u8)
            }
        },
        Err(e) => {
            let _ = e.print(); // help/version → stdout, errors → stderr
            ExitCode::from(if e.use_stderr() { 1 } else { 0 })
        }
    }
}

fn env_lookup(k: &str) -> Option<String> {
    std::env::var(k).ok()
}

/// The production `Exec`.
///
/// A `fn` pointer, not a closure: the blanket impl in `burn::cmd` is
/// higher-ranked over both reference arguments, and a `fn` pointer type
/// satisfies it for every lifetime with no inference to go wrong.
/// `&SYSTEM_EXEC` then coerces to `&dyn Exec` wherever a burner or a verifier
/// needs one.
type SystemExec = fn(&[String], &mut dyn FnMut(&str)) -> Result<CmdOutput, BurnError>;
const SYSTEM_EXEC: SystemExec = cmd::system;

fn home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/"))
}

fn open_catalog(path: &std::path::Path) -> Result<Catalog, PipelineError> {
    Catalog::open(path).map_err(|e| PipelineError::failed(&e))
}

/// Print one blob of success output.
fn say(text: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{text}");
}

fn dispatch(cli: args::Cli) -> Result<u8, PipelineError> {
    // The project name the default catalog/data-dir resolution needs.
    let project_name = match &cli.command {
        Command::Init { name, .. } => name,
        Command::Scan { name, .. } => name,
        Command::Plan { name, .. } => name,
        Command::Status { name, .. } => name,
        Command::Burn { name, .. } => name,
        Command::Reset { name, .. } => name,
        Command::Verify { name, .. } => name,
        Command::History { name, .. } => name,
        Command::Disc {
            command: args::DiscCommand::MarkDead { name, .. },
        } => name,
    };
    let resolved = paths::resolve(
        cli.catalog.as_deref(),
        cli.data_dir.as_deref(),
        project_name,
        &env_lookup,
        &home(),
    );
    std::fs::create_dir_all(&resolved.data_dir).map_err(|e| PipelineError::failed(&e))?;

    match cli.command {
        Command::Init {
            name,
            source,
            work_dir,
            disc_type,
            margin,
            copies,
        } => {
            // The work dir exists before anything else (Runner.dispatch).
            std::fs::create_dir_all(&work_dir).map_err(|e| PipelineError::failed(&e))?;
            let mut catalog = open_catalog(&resolved.catalog)?;
            let scan_cache = paths::scan_cache_path(&resolved.catalog, &name);
            let sink = progress::ConsoleProgress::new();
            let mut pipeline = Pipeline::new(
                &mut catalog,
                PipelinePaths {
                    work_dir: work_dir.clone(),
                    scan_cache,
                },
                &sink,
            );
            let pi = ProjectInit {
                name: name.clone(),
                source_root: source.display().to_string(),
                work_dir: work_dir.display().to_string(),
                default_disc_type: disc_type,
                margin_percent: margin,
                copies,
                created_at: chrono::Utc::now(),
            };
            let id = pipeline.init(&pi)?;
            say(&render::init_done(&name, id));
            Ok(0)
        }
        Command::Scan {
            name,
            verify,
            force,
        } => {
            // The work dir comes from the project row, not the CLI
            // (`Runner.loadProject`).
            let mut catalog = open_catalog(&resolved.catalog)?;
            let p = find_or_fail(&catalog, &name)?;
            let scan_cache = paths::scan_cache_path(&resolved.catalog, &name);
            let sink = progress::ConsoleProgress::new();
            let mut pipeline = Pipeline::new(
                &mut catalog,
                PipelinePaths {
                    work_dir: PathBuf::from(&p.work_dir),
                    scan_cache,
                },
                &sink,
            );
            let report = pipeline.scan(&name, verify, force)?;
            say(&render::scan_report(&report));
            Ok(0)
        }
        Command::Plan {
            name,
            disc_type,
            finalize,
        } => {
            // The work dir comes from the project row, not the CLI
            // (`Runner.withProject`). `plan` reports no progress.
            let mut catalog = open_catalog(&resolved.catalog)?;
            let p = find_or_fail(&catalog, &name)?;
            let pipeline = Pipeline::new(
                &mut catalog,
                PipelinePaths {
                    work_dir: PathBuf::from(&p.work_dir),
                    scan_cache: paths::scan_cache_path(&resolved.catalog, &name),
                },
                &NoopSink,
            );
            let report = pipeline.plan(&name, disc_type, finalize)?;
            say(&render::plan_report(&report));
            Ok(0)
        }
        Command::Status { name } => {
            let mut catalog = open_catalog(&resolved.catalog)?;
            let p = find_or_fail(&catalog, &name)?;
            let pipeline = Pipeline::new(
                &mut catalog,
                PipelinePaths {
                    work_dir: PathBuf::from(&p.work_dir),
                    scan_cache: paths::scan_cache_path(&resolved.catalog, &name),
                },
                &NoopSink,
            );
            let report = pipeline.status(&name)?;
            say(&render::status_report(&p, &report));
            Ok(0)
        }
        Command::Burn { name, yes } => {
            // The project lookup comes first (`Runner.withProject` resolves the
            // project before `Prompt.confirm`), so an unknown project fails with
            // exit 2 without asking anything.
            let sink = progress::ConsoleProgress::new();
            let mut catalog = open_catalog(&resolved.catalog)?;
            let p = find_or_fail(&catalog, &name)?;

            // Burn the plan exactly as staged: it carries the disc type chosen
            // at `plan` time, and re-planning here would silently discard a
            // `plan --disc-type ...` choice. `render::burn_report` then reports
            // what was actually written.
            let par = diskrement_shell::fs::scanner::parallelism_from_env();
            for w in &par.warnings {
                eprintln!("{w}");
            }
            let exec = SYSTEM_EXEC;
            let operator = prompt::ConsoleOperator;
            let burner = match backend::resolve(cli.burner, cli.device.as_deref(), &env_lookup) {
                backend::Backend::Simulated => {
                    Burner::Simulated(SimulatedBurner::new(&sink, par.value))
                }
                backend::Backend::Real(device) => Burner::Real(XorrisoBurner::new(
                    device, &exec, &operator, &sink, par.value,
                )),
            };

            if !prompt::confirm(&format!("Burn the staged plan for {name}?"), yes) {
                say("Aborted.");
                return Ok(0);
            }

            let mut pipeline = Pipeline::new(
                &mut catalog,
                PipelinePaths {
                    work_dir: PathBuf::from(&p.work_dir),
                    scan_cache: paths::scan_cache_path(&resolved.catalog, &name),
                },
                &sink,
            );
            let report = pipeline.burn(&name, chrono::Utc::now(), &burner)?;
            say(&render::burn_report(&report));
            Ok(0)
        }
        Command::Reset { name } => {
            let mut catalog = open_catalog(&resolved.catalog)?;
            let p = find_or_fail(&catalog, &name)?;
            let pipeline = Pipeline::new(
                &mut catalog,
                PipelinePaths {
                    work_dir: PathBuf::from(&p.work_dir),
                    scan_cache: paths::scan_cache_path(&resolved.catalog, &name),
                },
                &NoopSink,
            );
            pipeline.reset(&name)?;
            say(&render::reset_done(&name));
            Ok(0)
        }
        Command::Verify { name, disc } => {
            let mut catalog = open_catalog(&resolved.catalog)?;
            let p = find_or_fail(&catalog, &name)?;
            let exec = SYSTEM_EXEC;
            let operator = prompt::ConsoleOperator;
            let verifier = resolve_verifier(
                cli.burner,
                cli.device.as_deref(),
                std::path::Path::new(&p.work_dir),
                &exec,
                &operator,
            );
            let mut pipeline = Pipeline::new(
                &mut catalog,
                PipelinePaths {
                    work_dir: PathBuf::from(&p.work_dir),
                    scan_cache: paths::scan_cache_path(&resolved.catalog, &name),
                },
                &NoopSink,
            );
            let reports = pipeline.verify(&name, disc, &verifier)?;
            say(&render::verify_report(&reports));
            // Result-driven: corruption found is a successful run with a bad
            // outcome, so it does not travel the error ladder.
            Ok(render::verify_exit(&reports))
        }
        Command::History { name, rel_path } => {
            let mut catalog = open_catalog(&resolved.catalog)?;
            let p = find_or_fail(&catalog, &name)?;
            let pipeline = Pipeline::new(
                &mut catalog,
                PipelinePaths {
                    work_dir: PathBuf::from(&p.work_dir),
                    scan_cache: paths::scan_cache_path(&resolved.catalog, &name),
                },
                &NoopSink,
            );
            let rel = diskrement_core::RelPath(rel_path);
            let entries = pipeline.history(&name, &rel)?;
            say(&render::history(&rel, &entries));
            Ok(0)
        }
        Command::Disc {
            command: args::DiscCommand::MarkDead { name, ordinal, yes },
        } => {
            // The project lookup comes first (`Runner.withProject` resolves
            // the project before `Prompt.confirm`), so an unknown project
            // fails with exit 2 without asking anything.
            let mut catalog = open_catalog(&resolved.catalog)?;
            let p = find_or_fail(&catalog, &name)?;

            if !prompt::confirm(
                &format!(
                    "Mark disc {ordinal} of {name} DEAD? Superseded versions on it are \
                     lost forever; its live files will re-plan."
                ),
                yes,
            ) {
                say("Aborted.");
                return Ok(0);
            }

            let mut pipeline = Pipeline::new(
                &mut catalog,
                PipelinePaths {
                    work_dir: PathBuf::from(&p.work_dir),
                    scan_cache: paths::scan_cache_path(&resolved.catalog, &name),
                },
                &NoopSink,
            );
            let report = pipeline.mark_dead(&name, ordinal, chrono::Utc::now())?;
            say(&render::mark_dead(&report));
            Ok(0)
        }
    }
}

/// The verification backend, chosen by the same one decision that chooses the
/// burner (spec §5.2): verifying a simulated burn by mounting a physical disc
/// is meaningless, so the two arms move together.
fn resolve_verifier<'a>(
    burner: Option<args::BurnerKind>,
    device: Option<&str>,
    work_dir: &Path,
    exec: &'a dyn Exec,
    operator: &'a dyn diskrement_shell::burn::BurnOperator,
) -> Verifier<'a> {
    match backend::resolve(burner, device, &env_lookup) {
        backend::Backend::Simulated => {
            Verifier::Staging(StagingVerifier::new(work_dir.join("staging")))
        }
        backend::Backend::Real(device) => Verifier::Disc(DiscVerifier::new(device, exec, operator)),
    }
}

/// Resolve the project row first so the pipeline's work dir comes from the
/// catalog, and produce the contract exit-2 on a miss.
fn find_or_fail(catalog: &Catalog, name: &str) -> Result<Project, PipelineError> {
    catalog
        .find_project(name)
        .map_err(|e| PipelineError::failed(&e))?
        .ok_or_else(|| PipelineError::ProjectNotFound(name.to_string()))
}
