//! The `spatial-bench` CLI.

use clap::{Parser, Subcommand, ValueEnum};
use spatial_bench_core::case::{Budget, Runner};
use spatial_bench_core::catalog::Catalog;
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
        }) => cmd_run(cli, select, (*runner).into(), rustc.as_deref(), *dry_run),
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

fn cmd_run(
    cli: &Cli,
    select: &Select,
    runner: Runner,
    _rustc: Option<&str>,
    dry_run: bool,
) -> Result<(), String> {
    let catalog = load(cli)?;
    let selection = selection(select)?;
    let points = catalog.points(&selection);
    let estimate = catalog.estimate(&selection, runner, &Budget::default());

    println!(
        "{} cases, {} points, ~{}",
        catalog.matching(&selection).len(),
        points.len(),
        human(estimate)
    );
    report_unsatisfied(&catalog, &selection);

    if !dry_run {
        return Err(
            "not implemented: executing a run needs each subject built from its pinned ref"
                .to_owned(),
        );
    }
    Ok(())
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

    println!(
        "\nReproduce this selection:\n\n  {}\n",
        picker.command(runner)
    );
    Err("not implemented: executing a run needs each subject built from its pinned ref".to_owned())
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
