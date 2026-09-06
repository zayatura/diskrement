//! The diskrement binary: resolve paths, dispatch, render, map errors to the
//! exit ladder. Path resolution and rendering live here; the pipeline returns
//! data and performs no path resolution (CLAUDE.md invariant).

mod args;
mod paths;
mod progress;
mod render;

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use args::Command;
use clap::Parser as _;
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
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("{err}");
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

fn home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/"))
}

fn open_catalog(path: &std::path::Path) -> Result<Catalog, PipelineError> {
    Catalog::open(path).map_err(|e| PipelineError::Failed(e.to_string()))
}

/// Print one blob of success output.
fn say(text: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{text}");
}

fn dispatch(cli: args::Cli) -> Result<(), PipelineError> {
    // The project name the default catalog/data-dir resolution needs.
    let project_name = match &cli.command {
        Command::Init { name, .. } => name,
        Command::Scan { name, .. } => name,
        Command::Status { name, .. } => name,
    };
    let resolved = paths::resolve(
        cli.catalog.as_deref(),
        cli.data_dir.as_deref(),
        project_name,
        &env_lookup,
        &home(),
    );
    std::fs::create_dir_all(&resolved.data_dir)
        .map_err(|e| PipelineError::Failed(e.to_string()))?;

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
            std::fs::create_dir_all(&work_dir).map_err(|e| PipelineError::Failed(e.to_string()))?;
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
            Ok(())
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
            Ok(())
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
            Ok(())
        }
    }
}

/// Resolve the project row first so the pipeline's work dir comes from the
/// catalog, and produce the contract exit-2 on a miss.
fn find_or_fail(catalog: &Catalog, name: &str) -> Result<Project, PipelineError> {
    catalog
        .find_project(name)
        .map_err(|e| PipelineError::Failed(e.to_string()))?
        .ok_or_else(|| PipelineError::ProjectNotFound(name.to_string()))
}
