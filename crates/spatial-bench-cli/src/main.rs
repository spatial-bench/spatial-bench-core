//! The `spatial-bench` CLI.

use clap::{Parser, Subcommand, ValueEnum};
use spatial_bench_core::build::{self, BuildRequest, SubjectSource};
use spatial_bench_core::case::{Budget, Runner};
use spatial_bench_core::catalog::Catalog;
use spatial_bench_core::codegen;
use spatial_bench_core::harness::{CaseSpec, RunSpec};
use spatial_bench_core::picker::Picker;
use spatial_bench_core::selector::SelectorSet;
use std::path::PathBuf;
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
    /// Check the catalog against what the benchmark binaries really run.
    Conform {
        #[arg(long)]
        subject: Option<String>,
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

fn subjects_dir(cli: &Cli) -> Result<PathBuf, String> {
    if let Some(dir) = &cli.subjects {
        return Ok(dir.clone());
    }
    let beside_exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("subjects")));
    if let Some(dir) = beside_exe.filter(|d| d.is_dir()) {
        return Ok(dir);
    }
    let in_source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../subjects");
    if in_source.is_dir() {
        return Ok(in_source);
    }
    Err("cannot find the subjects directory; pass --subjects".to_owned())
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
        }) => execute(
            cli,
            &selection(select)?,
            (*runner).into(),
            rustc.as_deref(),
            *dry_run,
        ),
        Some(Command::Machine { explain }) => cmd_machine(*explain),
        Some(Command::Fingerprint { .. }) => {
            Err("not implemented: needs root to read memory timings".to_owned())
        }
        Some(Command::Submit { .. }) => {
            Err("not implemented: the results repository does not exist yet".to_owned())
        }
        Some(Command::Conform { .. }) => {
            Err("not implemented: needs each subject built from its pinned ref".to_owned())
        }
    }
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
        println!("{name:<20} {count:>3} cases");
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

fn execute(
    cli: &Cli,
    selection: &SelectorSet,
    runner: Runner,
    rustc: Option<&str>,
    dry_run: bool,
) -> Result<(), String> {
    let catalog = load(cli)?;
    let selection = selection.clone();
    let points = catalog.points(&selection);
    if points.is_empty() {
        return Err("that selection matches no data points".to_owned());
    }

    let budget = Budget::default();
    let groups = codegen::by_subject(&catalog, &selection);
    let overrides = subject_paths(cli)?;

    println!(
        "{} cases, {} points, {} build(s), ~{}",
        catalog.matching(&selection).len(),
        points.len(),
        catalog.build_units(&selection).len(),
        human(catalog.estimate(&selection, runner, &budget))
    );
    report_unsatisfied(&catalog, &selection);

    let floors: Vec<(String, Option<String>)> = groups
        .iter()
        .map(|(subject, _)| (subject.clone(), catalog.min_rustc(subject)))
        .collect();
    let toolchain =
        spatial_bench_core::toolchain::resolve(&floors, rustc).map_err(|e| format!("{e:?}"))?;
    println!("toolchain: {toolchain}");

    if dry_run {
        for (subject, cases) in &groups {
            let g = generate_for(&catalog, subject, cases, &toolchain, &overrides)?;
            println!(
                "  {subject}: {} combination(s), key {}",
                g.combinations, g.cache_key
            );
        }
        return Ok(());
    }

    let build_root = cli
        .build_dir
        .clone()
        .unwrap_or_else(|| data_dir().join("builds"));
    let mut collected: Vec<spatial_bench_core::schema::Point> = Vec::new();

    for (subject, cases) in &groups {
        let generated = generate_for(&catalog, subject, cases, &toolchain, &overrides)?;
        let source = overrides
            .get(subject)
            .cloned()
            .map(SubjectSource::Path)
            .ok_or_else(|| {
                format!(
                    "no source for {subject}.\n\
                     Building from a pinned ref is not implemented yet, so point it at a \
                     checkout:\n    --subject-path {subject}=/path/to/{}\n\
                     or set SPATIAL_BENCH_SUBJECT_PATHS={subject}=/path/to/{}",
                    subject_crate_name(subject),
                    subject_crate_name(subject),
                )
            })?;

        let request = BuildRequest {
            subject: subject.clone(),
            driver_crate: cases[0].driver_crate.clone(),
            driver_crate_path: engine_crate_path(&cases[0].driver_crate)?,
            subject_crate: subject_crate_name(subject),
            subject_source: source,
            features: catalog.features(subject),
            generated,
        };
        let dir = build::materialise(&build_root, &request).map_err(|e| e.to_string())?;

        eprintln!("building {subject} in {}", dir.display());
        let built = std::process::Command::new("cargo")
            .args(["build", "--release", "--bin", "driver"])
            .current_dir(&dir)
            .status()
            .map_err(|e| format!("could not run cargo: {e}"))?;
        if !built.success() {
            return Err(format!("building the {subject} driver failed"));
        }

        let spec = RunSpec {
            harness_version: spatial_bench_core::harness::HARNESS_VERSION,
            budget,
            cases: points
                .iter()
                .filter(|(c, _)| &c.subject == subject)
                .map(|(c, tags)| CaseSpec {
                    id: c.id.clone(),
                    tags: tags.clone(),
                    point_seed: POINT_SEED,
                    query_seed: QUERY_SEED,
                })
                .collect(),
        };
        collected.extend(drive(&dir.join("target/release/driver"), &spec)?);
    }

    let path = write_run(&selection, runner, &toolchain, &overrides, collected)?;
    println!("wrote {}", path.display());
    Ok(())
}

/// Fixed seeds, so every subject in a run sees byte-identical data and two runs
/// are comparable. They are part of the contract, not a per-driver choice.
const POINT_SEED: u64 = 0x5eed_0000_0000_0301;
const QUERY_SEED: u64 = 0x5eed_0000_0000_0302;

fn generate_for(
    catalog: &Catalog,
    subject: &str,
    cases: &[&spatial_bench_core::case::Case],
    toolchain: &spatial_bench_core::toolchain::Version,
    overrides: &std::collections::BTreeMap<String, PathBuf>,
) -> Result<spatial_bench_core::codegen::Generated, String> {
    let rev = match overrides.get(subject) {
        // A working tree has no revision. Naming it as such keeps two builds
        // from sharing a cache key across an edit.
        Some(path) => format!("worktree:{}", path.display()),
        None => catalog.pinned_ref(subject).unwrap_or_default(),
    };
    let macro_name = cases[0]
        .driver_macro
        .clone()
        .ok_or_else(|| format!("{subject} declares no driver macro"))?;
    Ok(codegen::generate(
        &cases[0].driver_crate,
        &macro_name,
        subject,
        cases,
        &codegen::BuildInputs {
            toolchain: toolchain.to_string(),
            subject_rev: rev,
            features: catalog.features(subject),
        },
    ))
}

fn drive(
    binary: &std::path::Path,
    spec: &RunSpec,
) -> Result<Vec<spatial_bench_core::schema::Point>, String> {
    use std::io::Write;
    let mut child = std::process::Command::new(binary)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not run {}: {e}", binary.display()))?;
    let json = serde_json::to_string(spec).map_err(|e| e.to_string())?;
    child
        .stdin
        .take()
        .ok_or("driver stdin was not piped")?
        .write_all(json.as_bytes())
        .map_err(|e| format!("writing the spec failed: {e}"))?;
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!("the driver exited with {:?}", out.status.code()));
    }
    spatial_bench_core::harness::read_points(out.stdout.as_slice()).map_err(|e| format!("{e:?}"))
}

fn subject_paths(cli: &Cli) -> Result<std::collections::BTreeMap<String, PathBuf>, String> {
    let mut out = std::collections::BTreeMap::new();
    for raw in &cli.subject_paths {
        let (name, dir) = raw
            .split_once('=')
            .ok_or_else(|| format!("--subject-path wants NAME=DIR, got `{raw}`"))?;
        let dir =
            std::fs::canonicalize(dir).map_err(|e| format!("--subject-path {name}: {dir}: {e}"))?;
        out.insert(name.to_owned(), dir);
    }
    Ok(out)
}

/// The engine's own crates, relative to this executable's source tree.
fn engine_crate_path(name: &str) -> Result<PathBuf, String> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let path = root.join("crates").join(name);
    std::fs::canonicalize(&path)
        .map_err(|e| format!("cannot find the {name} crate at {}: {e}", path.display()))
}

/// kiddo_v6 -> kiddo. The subject name carries a major version for the dataset;
/// the crate it builds does not.
fn subject_crate_name(subject: &str) -> String {
    spatial_bench_core::vocab::namespace_of(subject).to_owned()
}

fn data_dir() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("spatial-bench")
}

fn write_run(
    selection: &SelectorSet,
    runner: Runner,
    toolchain: &spatial_bench_core::toolchain::Version,
    overrides: &std::collections::BTreeMap<String, PathBuf>,
    points: Vec<spatial_bench_core::schema::Point>,
) -> Result<PathBuf, String> {
    use spatial_bench_core::machine::Machine;
    use spatial_bench_core::schema::{result_path, Context, Document, Run, Source, Toolchain};

    let now = timestamp();
    let machine = Machine::unknown();
    let run = Run {
        run_id: format!("{:016x}", blake3_of(&now)),
        started_at: now.clone(),
        finished_at: now,
        runner: match runner {
            Runner::Perf => "perf",
            _ => "criterion",
        }
        .to_owned(),
        selectors: selection.to_exprs(),
        machine_hash: machine.hash(),
        machine,
        context: Context {
            kernel: None,
            os: None,
            bench_profile: std::env::var("BENCH_PROFILE").ok(),
            governor: None,
            smt: None,
            boost: None,
            isolated_cpus: None,
        },
        toolchain: Toolchain {
            rustc: toolchain.to_string(),
            host: std::env::consts::ARCH.to_owned(),
            target_cpu: None,
            rustflags: std::env::var("RUSTFLAGS").ok(),
            features: Vec::new(),
            cargo_profile: "release".to_owned(),
            opt_level: Some("3".to_owned()),
        },
        // A working-tree build has no revision to record. Marked here so the
        // dataset can reject it rather than treating it as reproducible.
        source: Source {
            git_sha: None,
            git_dirty: !overrides.is_empty(),
            crate_version: env!("CARGO_PKG_VERSION").to_owned(),
        },
    };

    let document = Document {
        schema_version: spatial_bench_core::schema::SCHEMA_VERSION,
        run,
        points,
    };
    let path =
        result_path(&data_dir().join("runs"), &document.run).map_err(|e| format!("{e:?}"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&document).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(path)
}

fn blake3_of(s: &str) -> u64 {
    let bytes = *blake3_hash(s.as_bytes());
    u64::from_be_bytes(bytes[..8].try_into().unwrap())
}

fn blake3_hash(bytes: &[u8]) -> Box<[u8; 32]> {
    Box::new(*spatial_bench_core::machine::hash_bytes(bytes).as_bytes())
}

/// UTC in the ISO extended form the schema expects.
fn timestamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = secs / 86_400;
    let (h, m, s) = ((secs % 86_400) / 3600, (secs % 3600) / 60, secs % 60);
    let (y, mo, d) = civil_from_days(days as i64);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

/// Howard Hinnant's days-from-civil, inverted. Avoids a date dependency for the
/// one timestamp this binary needs.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn cmd_machine(explain: bool) -> Result<(), String> {
    if explain {
        return Err("not implemented: probing this host needs root for memory timings".to_owned());
    }
    Err("not implemented: probing this host needs root for memory timings".to_owned())
}

fn render(key: &str, value: &spatial_bench_core::tag::TagValue) -> String {
    let pow2 = spatial_bench_core::vocab::lookup(key).is_some_and(|d| d.pow2);
    value.to_expr(pow2)
}

fn show(catalog: &Catalog, picker: &Picker, runner: Runner, budget: &Budget) -> Vec<String> {
    let facets = picker.facets(catalog);
    let mut keys = Vec::new();
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
        println!("{:>3}. {:<22} {}{tail}", i + 1, facet.key, shown.join("  "));
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
    let runner = Runner::Criterion;
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
        _ => execute(cli, &selection, runner, None, false),
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
