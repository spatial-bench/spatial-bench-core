//! The `spatial-bench` CLI.
//!
//! Argument parsing, rendering, and exit codes. The run pipeline itself —
//! toolchain resolution, the fingerprint gate, per-subject dispatch, provenance,
//! the run document — lives in `spatial_bench_core::run`, shared with `conform`
//! (§13) so the two cannot drift.

use clap::{Parser, Subcommand, ValueEnum};
use serde_json::Value;
use spatial_bench_core::case::{Budget, Runner};
use spatial_bench_core::catalog::Catalog;
use spatial_bench_core::picker::Picker;
use spatial_bench_core::run;
use spatial_bench_core::selector::SelectorSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "spatial-bench",
    about = "Tag-addressed, library-agnostic spatial-index benchmarking",
    long_about = None,
    version
)]
struct Cli {
    /// Build a subject from a local directory instead of its pinned ref, as
    /// `NAME=DIR`. Runs produced this way record a checkout rather than a
    /// revision, and are marked so the dataset can reject them.
    #[arg(
        long = "subject-path",
        value_name = "NAME=DIR",
        global = true,
        env = "SPATIAL_BENCH_SUBJECT_PATHS",
        value_delimiter = ','
    )]
    subject_paths: Vec<String>,

    /// Where generated drivers are built. Engine output, never sources.
    #[arg(long, global = true, env = "SPATIAL_BENCH_BUILD_DIR")]
    build_dir: Option<PathBuf>,

    /// Directory of vendored subject manifests. Defaults to `subjects/` beside
    /// the executable, then the source tree, so a dev build works uninstalled.
    #[arg(long, global = true, env = "SPATIAL_BENCH_SUBJECTS")]
    subjects: Option<PathBuf>,

    /// An engine source checkout to build drivers from, for an installed
    /// binary (which has no compile-time tree of its own). Dev builds running
    /// from the source tree find it automatically; this points elsewhere.
    #[arg(
        long,
        value_name = "DIR",
        global = true,
        env = "SPATIAL_BENCH_ENGINE_SRC"
    )]
    engine_src: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Show the cases matching a selector.
    List {
        #[command(flatten)]
        select: Select,
        #[arg(long, value_enum, default_value_t = Format::Table)]
        format: Format,
    },
    /// Execute matching cases.
    Run {
        #[command(flatten)]
        select: Select,
        #[arg(long, value_enum, default_value_t = RunnerArg::Criterion)]
        runner: RunnerArg,
        /// Pin the toolchain every Rust subject is built with. Refused if below
        /// any selected subject's declared floor.
        #[arg(long)]
        rustc: Option<String>,
        /// Print the plan and stop.
        #[arg(long)]
        dry_run: bool,
        /// Proceed with no machine fingerprint. The run's machine hash is
        /// degraded by construction and the dataset should reject it.
        #[arg(long)]
        allow_unfingerprinted: bool,
        /// The random seed for the dataset generator.
        #[arg(long, value_name = "N", default_value_t = 42)]
        random_seed: u64,
    },
    /// List the libraries under test, their pinned refs and case counts.
    Subjects,
    /// Dump the whole catalog as JSON.
    Describe,
    /// Capture this machine's hardware fingerprint (`--write` needs root).
    Fingerprint {
        #[arg(long)]
        write: bool,
    },
    /// Show this host's fingerprint and continuity hash.
    Machine {
        /// Show every component, where it comes from, and whether it was read.
        #[arg(long)]
        explain: bool,
    },
    /// Open a dataset pull request for completed runs.
    Submit {
        #[arg(long)]
        to: Option<PathBuf>,
    },
    /// Collate a results checkout into a SQLite file for the front-end.
    Publish {
        /// Results repo checkout (default: sibling of the engine checkout).
        #[arg(long)]
        results: Option<PathBuf>,
        /// Where to write the database.
        #[arg(long, default_value = "benchmarks.sqlite")]
        out: PathBuf,
        /// The git revision this collation represents; recorded in
        /// latest.json so the front-end can detect a new snapshot.
        #[arg(long)]
        sha: Option<String>,
    },
    /// Check the catalog against what the benchmark binaries really run.
    Conform {
        #[arg(long)]
        subject: Option<String>,
    },
    /// Render charts of recorded runs (delegates to spatial-bench-chart).
    Chart {
        /// Arguments passed through to spatial-bench-chart.
        #[arg(allow_hyphen_values = true, trailing_var_arg = true)]
        args: Vec<String>,
    },
}

#[derive(clap::Args)]
struct Select {
    /// Selector expression; repeat to union.
    ///
    /// `,` = AND, `|` = OR within a key, `..` = inclusive range, `*` = key must
    /// exist, `!k=50` = negate.
    #[arg(long = "select", value_name = "EXPR")]
    exprs: Vec<String>,
}

#[derive(Copy, Clone, ValueEnum)]
enum Format {
    Table,
    Json,
    Tags,
}

#[derive(Copy, Clone, PartialEq, ValueEnum)]
enum RunnerArg {
    Criterion,
    Perf,
}

impl From<RunnerArg> for Runner {
    fn from(value: RunnerArg) -> Self {
        match value {
            RunnerArg::Criterion => Runner::Criterion,
            RunnerArg::Perf => Runner::Perf,
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::from(2)
        }
    }
}

/// The engine source checkout to build drivers and read subjects from, if one
/// is reachable.
///
/// Order: an explicit `--engine-src`/`SPATIAL_BENCH_ENGINE_SRC` checkout, then
/// the source tree this binary was compiled from (a dev build). An installed
/// `cargo install` binary has neither, so it falls back to published crates
/// for drivers — see [`driver_source`] — and still needs a subjects dir, since
/// manifests are data rather than code and are not fetched from a registry.
fn engine_root(cli: &Cli) -> Option<PathBuf> {
    if let Some(raw) = &cli.engine_src {
        return std::fs::canonicalize(raw).ok().filter(|p| p.is_dir());
    }
    let in_source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    std::fs::canonicalize(&in_source)
        .ok()
        .filter(|p| p.is_dir())
}

fn subjects_dir(cli: &Cli) -> Result<PathBuf, String> {
    if let Some(dir) = &cli.subjects {
        return Ok(dir.clone());
    }
    // the catalog lives in the bencher repo — discovered as the
    // conventional sibling checkout of the engine unless pointed elsewhere.
    if let Some(root) = engine_root(cli) {
        if let Some(parent) = root.parent() {
            let sibling = parent.join("spatial-bench-benchers").join("subjects");
            if sibling.is_dir() {
                return Ok(sibling);
            }
        }
    }
    Err(
        "cannot find the subjects directory. The catalog lives in the \
         spatial-bench-benchers repo: pass --subjects <bencher>/subjects \
         or set SPATIAL_BENCH_SUBJECTS"
            .to_owned(),
    )
}

fn load(cli: &Cli) -> Result<Catalog, String> {
    let dir = subjects_dir(cli)?;
    spatial_bench_core::catalog_load::load_dir(&dir)
        .map_err(|e| format!("loading manifests from {}: {e:?}", dir.display()))
}

fn selection(select: &Select) -> Result<SelectorSet, String> {
    SelectorSet::parse_all(&select.exprs).map_err(|e| format!("bad selector: {e:?}"))
}

fn run(cli: &Cli) -> Result<(), String> {
    match &cli.command {
        None => interactive(cli),
        Some(Command::List { select, format }) => cmd_list(cli, select, *format),
        Some(Command::Subjects) => cmd_subjects(cli),
        Some(Command::Describe) => cmd_describe(cli),
        Some(Command::Run {
            select,
            runner,
            rustc,
            dry_run,
            allow_unfingerprinted,
            random_seed,
        }) => cmd_run(
            cli,
            select,
            (*runner).into(),
            rustc.as_deref(),
            *dry_run,
            *allow_unfingerprinted,
            *random_seed,
        ),
        Some(Command::Machine { explain }) => cmd_machine(*explain),
        Some(Command::Fingerprint { write }) => cmd_fingerprint(*write),
        Some(Command::Submit { to }) => cmd_submit(to.as_deref()),
        Some(Command::Publish { results, out, sha }) => {
            cmd_publish(results.as_deref(), &out, sha.as_deref())
        }
        Some(Command::Conform { subject }) => cmd_conform(cli, subject.as_deref()),
        Some(Command::Chart { args }) => cmd_chart(args.clone()),
    }
}

/// The chart trampoline (cargo external-subcommand pattern): discover
/// `spatial-bench-chart`, forward args verbatim with inherited stdio, and
/// propagate exit codes. The parent defines no chart arguments — the child's
/// CLI can evolve without touching this.
fn cmd_chart(args: Vec<String>) -> Result<(), String> {
    let Some(chart_bin) = discover_chart() else {
        return Err(install_guidance());
    };
    let status = std::process::Command::new(&chart_bin)
        .args(&args)
        .status()
        .map_err(|e| format!("could not run {}: {e}", chart_bin.display()))?;
    if !status.success() {
        std::process::exit(status.code().unwrap_or(2));
    }
    Ok(())
}

/// Where `spatial-bench-chart` comes from: beside this binary, the env
/// override, or PATH.
fn discover_chart() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("SPATIAL_BENCH_CHART") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join("spatial-bench-chart");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join("spatial-bench-chart"))
        .find(|p| p.is_file())
}

/// The soft-fail guidance when the charting tool is absent ().
fn install_guidance() -> String {
    let engine_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let crate_dir = engine_root.join("crates/spatial-bench-charting");
    let install_cmd = if crate_dir.is_dir() {
        "cargo install --path crates/spatial-bench-charting".to_owned()
    } else {
        "cargo install spatial-bench-charting".to_owned()
    };
    format!(
        "the charting tool is not installed.\n\n  spatial-bench chart is provided by the `spatial-bench-charting` \\\n  crate, installed separately so the bench runner stays lean.\\n\n  Install it:\n      {install_cmd}\n\n  Or point SPATIAL_BENCH_CHART at an existing binary."
    )
}

/// §13, check 3: build each subject's driver, ask it what it contains, and
/// assert set-equality with the manifest. Rendering only — the check lives in
/// `spatial_bench_core::conform`, on the same seams a run uses.
fn cmd_conform(cli: &Cli, subject: Option<&str>) -> Result<(), String> {
    let catalog = load(cli)?;
    let config = spatial_bench_core::conform::ConformConfig {
        catalog: &catalog,
        subject,
        build_root: cli
            .build_dir
            .clone()
            .unwrap_or_else(|| run::data_dir().join("builds")),
        engine_root: engine_root(cli),
        subject_paths: &subject_paths(cli)?,
    };
    let report = spatial_bench_core::conform::conform(&config)?;
    for s in &report.subjects {
        if !s.checked {
            println!(
                "{:<20} skipped — {}",
                s.subject,
                s.skipped_reason.as_deref().unwrap_or_default()
            );
            continue;
        }
        if s.is_match() {
            println!(
                "{:<20} {} manifest cases, {} driver registrations — match",
                s.subject, s.manifest_cases, s.driver_registrations
            );
        } else {
            println!(
                "{:<20} {} manifest cases, {} driver registrations — DRIFT",
                s.subject, s.manifest_cases, s.driver_registrations
            );
            for missing in &s.missing_in_driver {
                println!("  missing in driver: {}", render_pairs(missing));
            }
            for extra in &s.extra_in_driver {
                println!("  not in manifest:  {}", render_pairs(extra));
            }
        }
    }
    if report.all_match() {
        Ok(())
    } else {
        Err("conform failed: a driver does not match its manifest".to_owned())
    }
}

/// A compile-time key as it would appear in a selector, for drift reports.
fn render_pairs(pairs: &[(String, String)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(",")
}

fn cmd_list(cli: &Cli, select: &Select, format: Format) -> Result<(), String> {
    let catalog = load(cli)?;
    let selection = selection(select)?;
    let points = catalog.points(&selection);

    match format {
        Format::Json => {
            let rows: Vec<_> = points.iter().map(|(_, tags)| tags).collect();
            println!(
                "{}",
                serde_json::to_string_pretty(&rows).map_err(|e| e.to_string())?
            );
        }
        Format::Tags => {
            for (_, tags) in &points {
                let rendered: Vec<String> = tags.iter().map(|(k, v)| format!("{k}={v}")).collect();
                println!("{}", rendered.join(","));
            }
        }
        Format::Table => {
            // Only the tags that actually differ across matching cases; printing
            // the ones every row shares would bury the distinctions.
            let columns = catalog.varying_keys(&selection);
            println!(
                "{:<20} {}  {:>7}",
                "subject",
                columns
                    .iter()
                    .map(|c| format!("{c:<12}"))
                    .collect::<String>(),
                "points"
            );
            for case in catalog.matching(&selection) {
                let n = points.iter().filter(|(c, _)| c.id == case.id).count();
                let cells: String = columns
                    .iter()
                    .map(|key| {
                        let v = case
                            .tags
                            .get(key.as_str())
                            .map(ToString::to_string)
                            .unwrap_or_else(|| "-".to_owned());
                        format!("{v:<12}")
                    })
                    .collect();
                println!("{:<20} {cells}  {n:>7}", case.subject);
            }
            println!(
                "\n{} cases, {} points",
                catalog.matching(&selection).len(),
                points.len()
            );
        }
    }

    report_unsatisfied(&catalog, &selection);
    Ok(())
}

fn cmd_subjects(cli: &Cli) -> Result<(), String> {
    let catalog = load(cli)?;
    let mut names: Vec<&str> = catalog.cases().iter().map(|c| c.subject.as_str()).collect();
    names.sort_unstable();
    names.dedup();
    for name in names {
        let count = catalog.for_subject(name).count();
        let pin = catalog.pinned_ref(name).unwrap_or_default();
        println!("{name:<20} {pin:<20} {count:>3} cases");
    }
    Ok(())
}

fn cmd_describe(cli: &Cli) -> Result<(), String> {
    let catalog = load(cli)?;
    println!(
        "{}",
        serde_json::to_string_pretty(catalog.cases()).map_err(|e| e.to_string())?
    );
    Ok(())
}

/// The run command: rendering around the core pipeline. The summary before,
/// the plan or outcome after — everything between lives in
/// `spatial_bench_core::run`, which `conform` (§13) will share.
fn cmd_run(
    cli: &Cli,
    select: &Select,
    runner: Runner,
    rustc: Option<&str>,
    dry_run: bool,
    allow_unfingerprinted: bool,
    random_seed: u64,
) -> Result<(), String> {
    let catalog = load(cli)?;
    let selection = selection(select)?;
    run_selection(
        cli,
        &catalog,
        &selection,
        runner,
        rustc,
        dry_run,
        allow_unfingerprinted,
        random_seed,
    )
}

#[allow(clippy::too_many_arguments)]
fn run_selection(
    cli: &Cli,
    catalog: &Catalog,
    selection: &SelectorSet,
    runner: Runner,
    rustc: Option<&str>,
    dry_run: bool,
    allow_unfingerprinted: bool,
    random_seed: u64,
) -> Result<(), String> {
    let budget = Budget::default();
    let points = catalog.points(selection);
    if points.is_empty() {
        return Err("that selection matches no data points".to_owned());
    }
    println!(
        "{} cases, {} points, {} build(s), ~{}",
        catalog.matching(selection).len(),
        points.len(),
        catalog.build_units(selection).len(),
        human(catalog.estimate(selection, runner, &budget))
    );
    report_unsatisfied(catalog, selection);

    let config = run::RunConfig {
        catalog,
        selection,
        runner,
        rustc,
        allow_unfingerprinted,
        random_seed,
        build_root: cli
            .build_dir
            .clone()
            .unwrap_or_else(|| run::data_dir().join("builds")),
        engine_root: engine_root(cli),
        subject_paths: &subject_paths(cli)?,
    };

    if dry_run {
        let plan = run::plan(&config)?;
        println!(
            "toolchain: {}",
            plan.toolchain
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_else(|| "none (no rust subjects)".to_owned())
        );
        for subject in &plan.subjects {
            println!(
                "  {}: {} combination(s), key {}",
                subject.subject, subject.combinations, subject.cache_key
            );
        }
        return Ok(());
    }

    println!(
        "toolchain: {}",
        run::resolve_toolchain(&config)?
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_else(|| "none (no rust subjects)".to_owned())
    );
    let outcome = run::execute(&config)?;
    println!("wrote {}", outcome.path.display());
    Ok(())
}

fn subject_paths(cli: &Cli) -> Result<std::collections::BTreeMap<String, PathBuf>, String> {
    let mut out = std::collections::BTreeMap::new();
    for raw in &cli.subject_paths {
        let (name, dir) = raw
            .split_once('=')
            .ok_or_else(|| format!("--subject-path wants NAME=DIR, got `{raw}`"))?;
        let dir =
            std::fs::canonicalize(dir).map_err(|e| format!("--subject-path {name}: {dir}: {e}"))?;
        // the path is interpolated into a generated Cargo.toml; a quote,
        // backslash or newline would inject or corrupt keys there.
        if !spatial_bench_core::build::toml_safe_path(&dir) {
            return Err(format!(
                "--subject-path {name}: the directory path contains a quote, \
                 backslash or newline, which cannot be written safely into a \
                 generated manifest"
            ));
        }
        out.insert(name.to_owned(), dir);
    }
    Ok(out)
}

/// Submit completed run documents to the dataset repo (§8, §10).
///
/// Copies submittable run documents into a checkout of the results repo,
/// extracts machine fingerprints to `machines/<hash>.toml` (deduplicated),
/// creates a branch, commits, pushes, and opens a PR via `gh`.
fn cmd_submit(to: Option<&Path>) -> Result<(), String> {
    let runs_dir = run::data_dir().join("runs");
    if !runs_dir.is_dir() {
        return Err(format!(
            "no runs directory at {} — run something first",
            runs_dir.display()
        ));
    }

    // Resolve the results repo checkout.
    let results_dir = match to {
        Some(dir) => dir.to_path_buf(),
        None => {
            // Default: a sibling checkout of the results repo.
            let engine_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
            let sibling = engine_root
                .parent()
                .expect("engine root has a parent")
                .join("spatial-bench-results");
            if sibling.is_dir() {
                sibling
            } else {
                return Err(format!(
                    "no results repo checkout at {}\nClone it first:\n    git clone https://github.com/spatial-bench/spatial-bench-results.git {}",
                    sibling.display(),
                    sibling.display()
                ));
            }
        }
    };

    // Load and filter submittable runs.
    let runs_dir_scan = |d: &Path, files: &mut Vec<(String, PathBuf)>| {
        let Ok(entries) = std::fs::read_dir(d) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "json") {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                files.push((name, path));
            }
        }
    };
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    runs_dir_scan(&runs_dir, &mut files);
    let Ok(months) = std::fs::read_dir(&runs_dir) else {
        return Err("cannot read the runs directory".to_owned());
    };
    for month in months.flatten() {
        let month_dir = month.path();
        if month_dir.is_dir() {
            runs_dir_scan(&month_dir, &mut files);
        }
    }
    files.sort_by(|a, b| b.0.cmp(&a.0));

    // Load each document and check submittability.
    let mut submittable: Vec<(String, PathBuf, Value)> = Vec::new();
    let mut skipped = 0;
    for (_, path) in &files {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let Ok(doc) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        if doc.get("schema_version").and_then(Value::as_u64) != Some(1) {
            skipped += 1;
            continue;
        }
        let Some(run) = doc.get("run") else { continue };
        let git_dirty = run
            .get("source")
            .and_then(|s| s.get("git_dirty"))
            .and_then(Value::as_bool)
            .unwrap_or(true);
        if git_dirty {
            skipped += 1;
            continue;
        }
        // Every subject must have sha provenance (S1).
        let subjects = run.get("subjects").and_then(Value::as_object);
        let all_pinned = subjects
            .map(|s| {
                s.values().all(|v| {
                    v.get("sha")
                        .and_then(Value::as_str)
                        .is_some_and(|s| !s.is_empty())
                })
            })
            .unwrap_or(true);
        if !all_pinned {
            skipped += 1;
            continue;
        }
        submittable.push((
            path.file_name().unwrap().to_string_lossy().into_owned(),
            path.clone(),
            doc,
        ));
    }
    if submittable.is_empty() {
        return Err(format!(
            "no submittable runs found ({skipped} skipped: worktree builds or missing provenance)"
        ));
    }
    println!(
        "{} submittable run(s), {skipped} skipped",
        submittable.len()
    );

    // Copy run documents into the results repo's datasets/ dir.
    let datasets_dir = results_dir.join("datasets");
    std::fs::create_dir_all(&datasets_dir).map_err(|e| e.to_string())?;
    for (name, src_path, _) in &submittable {
        // The month subdirectory comes from the filename (YYYYMMDD... → YYYY-MM).
        let month_dir = format!("{}-{}", &name[..4], &name[4..6]);
        let dst = datasets_dir.join(&month_dir);
        std::fs::create_dir_all(&dst).map_err(|e| e.to_string())?;
        std::fs::copy(src_path, dst.join(name)).map_err(|e| format!("copying {name}: {e}"))?;
    }

    // Extract machine fingerprints to machines/<hash>.toml (deduplicated).
    let machines_dir = results_dir.join("machines");
    std::fs::create_dir_all(&machines_dir).map_err(|e| e.to_string())?;
    for (_, _, doc) in &submittable {
        let machine_hash = doc
            .get("run")
            .and_then(|r| r.get("machine_hash"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let machine_file = machines_dir.join(format!("{machine_hash}.toml"));
        if machine_file.exists() {
            continue;
        }
        let Some(machine) = doc.get("run").and_then(|r| r.get("machine")) else {
            continue;
        };
        // Serialize the machine block as TOML (§10: machine detail extracted
        // once per machine, not repeated in every run).
        let toml_text = json_to_toml(machine)?;
        std::fs::write(&machine_file, toml_text).map_err(|e| e.to_string())?;
    }

    // Create a branch, commit, push, open PR via gh.
    let branch = format!("submit/{}", chrono_like_timestamp());
    let git = |args: &[&str]| -> Result<String, String> {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(&results_dir)
            .output()
            .map_err(|e| format!("git: {e}"))?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).into_owned());
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    };
    git(&["checkout", "-B", &branch, "main"])?;
    git(&["add", "-A"])?;
    let status = git(&["status", "--short"])?;
    if status.trim().is_empty() {
        return Ok(()); // nothing new to submit
    }
    git(&[
        "commit",
        "-m",
        &format!("submit: {} run document(s)", submittable.len()),
    ])?;
    git(&["push", "-u", "origin", &branch])?;

    // Open the PR via gh.
    let pr_body = format!(
        "Submits {} run document(s) from {}.\n\nGenerated by `spatial-bench submit`.",
        submittable.len(),
        runs_dir.display()
    );
    let gh_out = std::process::Command::new("gh")
        .args([
            "pr",
            "create",
            "--repo",
            "spatial-bench/spatial-bench-results",
            "--base",
            "main",
            "--head",
            &branch,
            "--title",
            &format!("submit: {} run document(s)", submittable.len()),
            "--body",
            &pr_body,
        ])
        .output()
        .map_err(|e| format!("could not run gh: {e}"))?;
    if !gh_out.status.success() {
        eprintln!(
            "warning: could not open the PR (the branch is pushed):\n{}",
            String::from_utf8_lossy(&gh_out.stderr)
        );
    }

    println!("submitted {} run document(s)", submittable.len());
    Ok(())
}

/// Convert a JSON value to TOML text. Used for machine fingerprint
/// extraction (§10: machine detail is extracted once to
/// `machines/<hash>.toml`, not repeated in every run).
fn json_to_toml(value: &Value) -> Result<String, String> {
    // TOML has no null — strip null fields before conversion (the machine
    // block has null for unreadable components like cpu_base_mhz).
    let cleaned = strip_nulls(value);
    let toml_value: toml::Value =
        serde_json::from_value(cleaned).map_err(|e| format!("JSON to TOML conversion: {e}"))?;
    toml::to_string_pretty(&toml_value).map_err(|e| format!("TOML serialization: {e}"))
}

/// Recursively remove null values from a JSON tree — TOML has no null.
fn strip_nulls(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let cleaned: serde_json::Map<String, Value> = map
                .iter()
                .filter(|(_, v)| !v.is_null())
                .map(|(k, v)| (k.clone(), strip_nulls(v)))
                .collect();
            Value::Object(cleaned)
        }
        Value::Array(arr) => Value::Array(
            arr.iter()
                .filter(|v| !v.is_null())
                .map(strip_nulls)
                .collect(),
        ),
        other => other.clone(),
    }
}

/// A timestamp suitable for a branch name: YYYYMMDD-HHMMSS.
fn chrono_like_timestamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = secs / 86_400;
    let (h, m, s) = ((secs % 86_400) / 3600, (secs % 3600) / 60, secs % 60);
    // Hinnant's civil-from-days.
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let mth = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let yr = if mth <= 2 { y + 1 } else { y };
    format!("{yr:04}{mth:02}{d:02}-{h:02}{m:02}{s:02}")
}

fn cmd_machine(explain: bool) -> Result<(), String> {
    use spatial_bench_core::machine::Machine;

    // The unprivileged view: what a run re-probes. When we happen to have
    // root, the privileged block is included opportunistically so the hash
    // shown matches what a capture would record.
    let mut machine = Machine::probe();
    if spatial_bench_core::machine::is_root() {
        let _ = machine.add_privileged();
    }
    if explain {
        println!("{}", machine.explain());
    } else {
        println!(
            "machine {} ({})",
            machine.hash(),
            if machine.is_degraded() {
                "degraded"
            } else {
                "complete"
            }
        );
        if machine.is_degraded() {
            println!(
                "  some components are unreadable; a full hash needs `{}`",
                spatial_bench_core::fingerprint::capture_command()
            );
        }
    }

    // When a fingerprint exists, say whether this host still matches it — the
    // same check every run performs.
    let path = spatial_bench_core::fingerprint::path();
    if path.exists() {
        match spatial_bench_core::fingerprint::Fingerprint::load(&path).and_then(|f| f.validate()) {
            Ok(recorded) => println!(
                "fingerprint {}: machine {} matches this host",
                path.display(),
                recorded.hash()
            ),
            Err(e) => println!("fingerprint {}: {e}", path.display()),
        }
    }
    Ok(())
}

fn cmd_fingerprint(write: bool) -> Result<(), String> {
    use spatial_bench_core::fingerprint::Fingerprint;

    let path = spatial_bench_core::fingerprint::path();
    if write {
        if let Some(raw) = std::env::var_os("SPATIAL_BENCH_FINGERPRINT") {
            // sudo often resets the environment, so the path the user set
            // may not be the path this process writes. Say both out loud.
            eprintln!(
                "note: SPATIAL_BENCH_FINGERPRINT is set to {}; under sudo this \
                 variable may be dropped and the capture writes to {}. \
                 To keep the env path: sudo --preserve-env=SPATIAL_BENCH_FINGERPRINT …",
                PathBuf::from(&raw).display(),
                path.display()
            );
        }
    }
    if !write {
        let file = Fingerprint::capture(false).map_err(|e| e.to_string())?;
        println!("would write {} (machine {})", path.display(), file.machine);
        println!(
            "capture the root-only block once per machine with `{}`",
            spatial_bench_core::fingerprint::capture_command()
        );
        return Ok(());
    }

    let file = Fingerprint::capture(true).map_err(|e| e.to_string())?;
    file.write(&path).map_err(|e| e.to_string())?;
    println!(
        "wrote {} (machine {}, taken {})",
        path.display(),
        file.machine,
        file.taken
    );
    if file.privileged.mem_timings.is_none() {
        println!(
            "note: memory timings were unreadable (decode-dimms absent or the\
             eeprom modules are not loaded), so the hash is degraded.\
             This is normal on many boards."
        );
    }
    Ok(())
}

fn render(key: &str, value: &spatial_bench_core::tag::TagValue) -> String {
    let pow2 = spatial_bench_core::vocab::lookup(key).is_some_and(|d| d.pow2);
    value.to_expr(pow2)
}

fn show(catalog: &Catalog, picker: &Picker, runner: Runner, budget: &Budget) -> Vec<String> {
    // The runner is the list's first row: it is a choice like any other, and
    // threads straight into run_selection . Only implemented runners are
    // offered — a menu item that ends in a refusal is not a choice.
    let available: Vec<Runner> = catalog
        .runners_for(&picker.selection())
        .iter()
        .filter_map(|r| Runner::parse(r))
        .filter(|r| r.implemented())
        .collect();
    let marked: Vec<String> = available
        .iter()
        .map(|r| {
            if *r == runner {
                format!("[{r}]")
            } else {
                r.to_string()
            }
        })
        .collect();
    println!("{:>3}. {:<22} {}", 1, "runner", marked.join("  "));

    let facets = picker.facets(catalog);
    let mut keys = vec!["runner".to_owned()];
    for (i, facet) in facets.iter().enumerate() {
        let shown: Vec<String> = facet
            .values
            .iter()
            .take(8)
            .map(|v| {
                let marked = facet.chosen.contains(v);
                let text = render(&facet.key, v);
                if marked {
                    format!("[{text}]")
                } else {
                    text
                }
            })
            .collect();
        let more = facet.values.len().saturating_sub(8);
        let tail = if more > 0 {
            format!("   … {more} more")
        } else {
            String::new()
        };
        println!("{:>3}. {:<22} {}{tail}", i + 2, facet.key, shown.join("  "));
        keys.push(facet.key.clone());
    }

    let preview = picker.preview(catalog, runner, budget);
    println!(
        "\n     {} cases · {} points · {} · est. {}",
        preview.cases,
        preview.points,
        preview.runners.join("/"),
        human(preview.estimate)
    );
    if !preview.unsatisfied.is_empty() {
        println!(
            "     warning: reached nothing: {}",
            preview.unsatisfied.join(", ")
        );
    }
    keys
}

fn prompt(text: &str) -> Option<String> {
    use std::io::Write;
    print!("{text}");
    std::io::stdout().flush().ok()?;
    let mut line = String::new();
    // EOF (piped or closed stdin) ends the loop rather than spinning.
    match std::io::stdin().read_line(&mut line) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(line.trim().to_owned()),
    }
}

fn interactive(cli: &Cli) -> Result<(), String> {
    let catalog = load(cli)?;
    let budget = Budget::default();
    let mut runner = Runner::Criterion;
    let mut picker = Picker::new();

    loop {
        println!();
        let keys = show(&catalog, &picker, runner, &budget);
        let Some(input) = prompt("\n  number to refine, `r` reset, `d` done, `q` quit: ") else {
            println!();
            break;
        };
        match input.as_str() {
            "q" => return Ok(()),
            "d" | "" => break,
            "r" => {
                picker = Picker::new();
                continue;
            }
            other => {
                let Ok(index) = other.parse::<usize>() else {
                    println!("  not a number: {other}");
                    continue;
                };
                let Some(key) = keys.get(index.wrapping_sub(1)) else {
                    println!("  no such row: {index}");
                    continue;
                };
                if key == "runner" {
                    // choose from what the selection supports and the
                    // engine implements; the rest are named so their absence
                    // is explained rather than silent.
                    let parsed: Vec<Runner> = catalog
                        .runners_for(&picker.selection())
                        .iter()
                        .filter_map(|r| Runner::parse(r))
                        .collect();
                    let offered: Vec<Runner> =
                        parsed.iter().copied().filter(|r| r.implemented()).collect();
                    for (i, r) in offered.iter().enumerate() {
                        let marker = if *r == runner { "[x]" } else { "[ ]" };
                        println!("{:>3}. {} {}", i + 1, marker, r);
                    }
                    let later: Vec<String> = parsed
                        .iter()
                        .copied()
                        .filter(|r| !r.implemented())
                        .map(|r| r.to_string())
                        .collect();
                    if !later.is_empty() {
                        println!("      (not implemented yet: {})", later.join(", "));
                    }
                    let Some(choice) = prompt("  number, blank to keep: ") else {
                        break;
                    };
                    if let Some(r) = choice
                        .trim()
                        .parse::<usize>()
                        .ok()
                        .and_then(|n| offered.get(n - 1))
                    {
                        runner = *r;
                    }
                    continue;
                }
                let facet = picker
                    .facets(&catalog)
                    .into_iter()
                    .find(|f| &f.key == key)
                    .ok_or("facet vanished between renders")?;
                for (i, value) in facet.values.iter().enumerate() {
                    println!("{:>3}. {}", i + 1, render(key, value));
                }
                let Some(choice) = prompt("  numbers separated by spaces, blank to clear: ") else {
                    break;
                };
                let mut chosen = Vec::new();
                for token in choice.split_whitespace() {
                    match token
                        .parse::<usize>()
                        .ok()
                        .and_then(|n| facet.values.get(n - 1))
                    {
                        Some(v) => chosen.push(v.clone()),
                        None => println!("  ignoring `{token}`"),
                    }
                }
                picker.choose(key, chosen);
            }
        }
    }

    // Minimised: walking the whole menu pins every facet, and echoing all of
    // them gives a line too long to read or paste.
    let selection = picker.minimal(&catalog);
    println!(
        "\nReproduce this selection:\n\n  {}\n",
        picker.command_for(runner, &catalog)
    );

    match prompt("Run now? [Y/n]: ").as_deref() {
        Some("n") | Some("N") => Ok(()),
        // EOF counts as yes so a piped session still runs; anything else too.
        _ => run_selection(cli, &catalog, &selection, runner, None, false, false, 42),
    }
}

fn report_unsatisfied(catalog: &Catalog, selection: &SelectorSet) {
    let missing = catalog.unsatisfied(selection);
    if !missing.is_empty() {
        eprintln!(
            "\nwarning: these reached no case at all: {}",
            missing.join(", ")
        );
    }
}

fn human(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    match secs {
        0 => "0s".to_owned(),
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m{:02}s", s / 60, s % 60),
        s => format!("{}h{:02}m", s / 3600, (s % 3600) / 60),
    }
}

/// Collate a results checkout into one SQLite file: the front-end's entire
/// data source. The repo stays the source of truth and the audit trail; this
/// is a deterministic rebuild over it, so publishing can never drift from
/// what the PRs reviewed.
///
/// Core identity keys become real columns (they are a closed vocabulary —
/// that is the design); every other tag lands in `point_tags`, so a subject's
/// own namespace (`kiddo.stem`) is queryable without schema surgery.
fn cmd_publish(results: Option<&Path>, out: &Path, sha: Option<&str>) -> Result<(), String> {
    use serde_json::Value;
    use std::collections::BTreeSet;

    let results_dir = match results {
        Some(dir) => dir.to_path_buf(),
        None => {
            let engine_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
            engine_root
                .parent()
                .expect("engine root has a parent")
                .join("spatial-bench-results")
        }
    };
    if !results_dir.is_dir() {
        return Err(format!(
            "no results checkout at {} — clone it or pass --results",
            results_dir.display()
        ));
    }

    if out.exists() {
        std::fs::remove_file(out)
            .map_err(|e| format!("could not replace {}: {e}", out.display()))?;
    }
    let conn = rusqlite::Connection::open(out)
        .map_err(|e| format!("could not open {}: {e}", out.display()))?;
    conn.execute_batch(
        r#"
        PRAGMA journal_mode = OFF;
        CREATE TABLE runs (
            id TEXT PRIMARY KEY,
            started_at TEXT,
            finished_at TEXT,
            runner TEXT,
            machine_hash TEXT,
            toolchain TEXT,
            engine_version TEXT,
            git_dirty INTEGER,
            fingerprint TEXT
        );
        CREATE TABLE machines (
            hash TEXT PRIMARY KEY,
            cpu_model TEXT,
            cores_physical INTEGER,
            threads_online INTEGER,
            mem_total_bytes INTEGER,
            board TEXT,
            chipset TEXT,
            kernel TEXT,
            os TEXT,
            governor TEXT,
            smt INTEGER,
            boost INTEGER
        );
        CREATE TABLE points (
            id INTEGER PRIMARY KEY,
            run_id TEXT REFERENCES runs(id),
            impl TEXT NOT NULL,
            version TEXT,
            pinned_ref TEXT,
            sha TEXT,
            axis TEXT, query TEXT, k INTEGER, dims INTEGER,
            metric TEXT, dataset TEXT,
            parallelism TEXT, query_batching TEXT, isa TEXT, config TEXT,
            tree_size INTEGER, query_count INTEGER, query_batch_size INTEGER,
            latency_ns REAL, latency_ns_lower REAL, latency_ns_upper REAL,
            throughput_qps REAL,
            median_ns REAL, mad_ns REAL, std_dev_ns REAL,
            samples INTEGER, ci REAL
        );
        CREATE TABLE point_tags (
            point_id INTEGER REFERENCES points(id),
            key TEXT NOT NULL,
            value TEXT NOT NULL
        );
        CREATE INDEX idx_points_impl ON points(impl, version, axis, query, config);
        CREATE INDEX idx_points_run ON points(run_id);
        CREATE INDEX idx_point_tags ON point_tags(key, value);
        "#,
    )
    .map_err(|e| format!("schema: {e}"))?;

    // Core identity keys: columns on `points`. Everything else is a tag.
    let core_keys: BTreeSet<&str> = [
        "impl",
        "axis",
        "query",
        "k",
        "dims",
        "metric",
        "dataset",
        "parallelism",
        "query_batching",
        "isa",
        "defaults_or_tuned",
        "tree_size",
        "query_count",
        "query_batch_size",
    ]
    .into_iter()
    .collect();

    let mut machine_stmt = conn
        .prepare("INSERT OR REPLACE INTO machines VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)")
        .map_err(|e| e.to_string())?;
    let mut machines = 0usize;
    let machine_dir = results_dir.join("machines");
    let mut entries: Vec<_> = std::fs::read_dir(&machine_dir)
        .map_err(|e| format!("{}: {e}", machine_dir.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .collect();
    entries.sort();
    for path in &entries {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        // machines/*.toml are TOML; serde_json cannot read them.
        let v: Value = {
            let tv: toml::Value =
                toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
            serde_json::to_value(tv).map_err(|e| e.to_string())?
        };
        let hash = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_owned);
        let i = |k: &str| v.get(k).and_then(|x| x.as_i64());
        let b = |k: &str| v.get(k).and_then(|x| x.as_bool());
        machine_stmt
            .execute(rusqlite::params![
                hash,
                s("cpu_model"),
                i("cores_physical"),
                i("threads_online"),
                i("mem_total_bytes"),
                s("board"),
                s("chipset"),
                s("kernel"),
                s("os"),
                s("governor"),
                b("smt"),
                b("boost"),
            ])
            .map_err(|e| e.to_string())?;
        machines += 1;
    }

    let mut run_stmt = conn
        .prepare("INSERT INTO runs VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)")
        .map_err(|e| e.to_string())?;
    let mut point_stmt = conn
        .prepare(
            "INSERT INTO points (run_id, impl, version, pinned_ref, sha, axis, query, k, \
             dims, metric, dataset, parallelism, query_batching, isa, config, \
             tree_size, query_count, query_batch_size, latency_ns, latency_ns_lower, \
             latency_ns_upper, throughput_qps, median_ns, mad_ns, std_dev_ns, samples, ci) \
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26,?27)",
        )
        .map_err(|e| e.to_string())?;
    let mut tag_stmt = conn
        .prepare("INSERT INTO point_tags VALUES (?1,?2,?3)")
        .map_err(|e| e.to_string())?;

    let datasets = results_dir.join("datasets");
    let mut docs: Vec<_> = walk_json(&datasets);
    docs.sort();
    let (mut runs, mut points) = (0usize, 0usize);
    for path in &docs {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let doc: Value =
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        let run = &doc["run"];
        let run_id = run["run_id"].as_str().unwrap_or_default().to_owned();
        run_stmt
            .execute(rusqlite::params![
                run_id,
                run["started_at"].as_str(),
                run["finished_at"].as_str(),
                run["runner"].as_str(),
                run["machine_hash"].as_str(),
                run["toolchain"].as_str(),
                run["source"]["crate_version"].as_str(),
                run["source"]["git_dirty"].as_bool().unwrap_or(false) as i64,
                run["context"]["fingerprint"].as_str(),
            ])
            .map_err(|e| format!("{}: {e}", path.display()))?;
        runs += 1;

        let subjects = &run["subjects"];
        for point in doc["points"].as_array().ok_or("points is not an array")? {
            let tags = &point["tags"];
            let impl_tag = tags["impl"].as_str().unwrap_or_default().to_owned();
            let subject = &subjects[&impl_tag];
            let num = |k: &str| tags[k].as_i64();
            let word = |k: &str| tags[k].as_str().map(str::to_owned);
            let metric = |k: &str| point["metrics"][k]["point"].as_f64();
            point_stmt
                .execute(rusqlite::params![
                    run_id,
                    impl_tag,
                    subject["version"].as_str(),
                    subject["pinned_ref"].as_str(),
                    subject["sha"].as_str(),
                    word("axis"),
                    word("query"),
                    num("k"),
                    num("dims"),
                    word("metric"),
                    word("dataset"),
                    word("parallelism"),
                    word("query_batching"),
                    word("isa"),
                    word("defaults_or_tuned"),
                    num("tree_size"),
                    num("query_count"),
                    num("query_batch_size"),
                    metric("latency_ns"),
                    point["metrics"]["latency_ns"]["lower"].as_f64(),
                    point["metrics"]["latency_ns"]["upper"].as_f64(),
                    metric("throughput_qps"),
                    point["stats"]["median_ns"].as_f64(),
                    point["stats"]["mad_ns"].as_f64(),
                    point["stats"]["std_dev_ns"].as_f64(),
                    point["stats"]["samples"].as_i64(),
                    point["stats"]["ci"].as_f64(),
                ])
                .map_err(|e| format!("{}: {e}", path.display()))?;
            let point_id = conn.last_insert_rowid();
            points += 1;
            if let Some(map) = tags.as_object() {
                for (key, value) in map {
                    if core_keys.contains(key.as_str()) {
                        continue;
                    }
                    let value = match value {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    tag_stmt
                        .execute(rusqlite::params![point_id, key, value])
                        .map_err(|e| e.to_string())?;
                }
            }
        }
    }

    // latest.json: the only mutable artifact the front-end polls.
    if let Some(parent) = out.parent() {
        let manifest = format!(
            "{{\"db\": {:?}, \"sha\": {}, \"runs\": {}, \"points\": {}, \
             \"machines\": {}, \"generated_at\": {:?}}}\n",
            out.file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("benchmarks.sqlite"),
            sha.map(|s| format!("{s:?}")).unwrap_or("null".to_owned()),
            runs,
            points,
            machines,
            chrono_now(),
        );
        let latest = parent.join("latest.json");
        std::fs::write(&latest, manifest).map_err(|e| format!("{}: {e}", latest.display()))?;
    }

    println!(
        "published {}: {runs} runs, {points} points, {machines} machines",
        out.display()
    );
    Ok(())
}

fn chrono_now() -> String {
    // RFC 3339 from the system clock without a chrono dependency: the run
    // documents already carry precise timestamps; this is a build stamp.
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

/// Every `*.json` under a directory, recursively (datasets/YYYY-MM/…).
fn walk_json(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk_json(&path));
        } else if path.extension().is_some_and(|x| x == "json") {
            out.push(path);
        }
    }
    out
}
