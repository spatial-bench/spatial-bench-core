//! `spatial-bench-chart` — the chart renderer for spatial-bench run
//! documents (: charting).
//!
//! Its own binary so the bench runner never links charting code or its
//! dependencies. The main `spatial-bench chart` subcommand is a thin
//! trampoline that discovers and execs this binary (the cargo
//! external-subcommand pattern).
//!
//! v1 charts: bars (cross-impl comparison with CI whiskers) and lines
//! (scaling across a param axis), log/linear y scales, dark/light themes,
//! rendered to an RGBA buffer via plotters and shown through the kitty
//! graphics protocol or saved as a PNG.

mod kitty;
mod model;
mod render;
mod term;

use clap::Parser;
use spatial_bench_core::selector::SelectorSet;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "spatial-bench-chart",
    about = "Chart spatial-bench run documents (kitty graphics protocol / PNG)",
    version
)]
struct Cli {
    /// Selector expression; repeat to union (same grammar as runs).
    #[arg(long = "select", value_name = "EXPR")]
    selects: Vec<String>,

    /// Which metric to plot.
    #[arg(long, value_name = "NAME", default_value = "latency_ns")]
    metric: String,

    /// The param axis for the x coordinate (enables scaling mode).
    #[arg(long, value_name = "TAG")]
    x: Option<String>,

    /// The tag the series are grouped by.
    #[arg(long, value_name = "TAG", default_value = "impl")]
    series: String,

    /// bars (cross-impl comparison) or lines (scaling).
    #[arg(long, value_enum, default_value_t = Kind::Auto)]
    kind: Kind,

    /// y scale: log or linear. Log is the default — subjects differ by
    /// orders of magnitude.
    #[arg(long, value_enum, default_value_t = Scale::Log)]
    scale: Scale,

    /// Chart title. Default: derived from the selection.
    #[arg(long, value_name = "TEXT")]
    title: Option<String>,

    /// Run-document directories (repeatable). Default: the engine's data dir.
    #[arg(long, value_name = "DIR")]
    from: Vec<PathBuf>,

    /// How many of the newest run documents to consider.
    #[arg(long, value_name = "N", default_value_t = 20)]
    latest: usize,

    /// Canvas width, in terminal cells.
    #[arg(long, value_name = "CELLS", default_value_t = 100)]
    width: u32,

    /// Canvas height, in terminal cells.
    #[arg(long, value_name = "CELLS", default_value_t = 30)]
    height: u32,

    /// Also save the chart as a PNG.
    #[arg(long, value_name = "PATH")]
    save: Option<PathBuf>,

    /// Skip the terminal display (just --save).
    #[arg(long)]
    no_image: bool,

    /// Light theme (default: dark).
    #[arg(long)]
    light: bool,

    /// Skip the kitty-protocol capability probe (display unconditionally).
    #[arg(long)]
    force_image: bool,
}

#[derive(clap::ValueEnum, Clone, Copy, PartialEq)]
enum Kind {
    Auto,
    Bars,
    Lines,
}

#[derive(clap::ValueEnum, Clone, Copy, PartialEq)]
enum Scale {
    Log,
    Linear,
}

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            std::process::ExitCode::from(2)
        }
    }
}

fn run(cli: &Cli) -> Result<(), String> {
    let mut dirs = cli.from.clone();
    if dirs.is_empty() {
        dirs.push(data_dir().join("runs"));
    }
    let docs = model::load_runs(&dirs, cli.latest).map_err(|e| e.to_string())?;
    if docs.is_empty() {
        return Err(format!(
            "no run documents found in {} — run something first, or point \
             --from at a directory that has them",
            dirs.iter()
                .map(|d| d.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    let selector =
        SelectorSet::parse_all(&cli.selects).map_err(|e| format!("bad selector: {e:?}"))?;
    let series = model::build_series(&docs, &selector, &cli.metric, cli.x.as_deref(), &cli.series);
    if series.iter().all(|s| s.points.is_empty()) {
        return Err("that selection matched no data in the loaded runs".to_owned());
    }

    let bars = match cli.kind {
        Kind::Bars => true,
        Kind::Lines => false,
        Kind::Auto => cli.x.is_none(),
    };
    let log_y = cli.scale == Scale::Log;

    let (cell_w, cell_h) = if term::stdin_is_tty() {
        term::cell_size()
    } else {
        (10, 20)
    };
    let width = cli.width * cell_w;
    let height = cli.height * cell_h;

    let title = cli.title.clone().unwrap_or_else(|| {
        let selection = if cli.selects.is_empty() {
            "everything".to_owned()
        } else {
            cli.selects.join(" & ")
        };
        let runner = docs
            .first()
            .map(|d| d.meta.runner.clone())
            .unwrap_or_default();
        render::title_for(
            &cli.metric,
            &cli.series,
            cli.x.as_deref(),
            &runner,
            &selection,
        )
    });

    let spec = render::ChartSpec {
        title,
        y_desc: cli.metric.clone(),
        x_desc: cli.x.as_deref().unwrap_or("case").to_owned(),
        bars,
        log_y,
        width,
        height,
        dark: !cli.light,
    };
    let buf = render::render(&spec, &series)?;

    if let Some(path) = &cli.save {
        render::render_to_png(&spec, &series, path)?;
        println!("saved {}", path.display());
    }

    let supported = cli.force_image || (term::stdin_is_tty() && kitty::likely_supported());
    if cli.no_image {
        if cli.save.is_none() {
            return Err(
                "--no-image without --save has nothing to show; add --save out.png".to_owned(),
            );
        }
        return Ok(());
    }
    if !supported && !cli.force_image {
        return Err(
            "this terminal does not appear to support the kitty graphics \
             protocol; use --save out.png to write the chart to a file"
                .to_owned(),
        );
    }
    let (cell_w, cell_h) = if cli.force_image && cell_w == 0 {
        (10, 20)
    } else {
        (cell_w, cell_h)
    };
    let _ = (cell_w, cell_h);
    kitty::transmit(&kitty::Image {
        width,
        height,
        rgb: buf,
        cell_width_px: cell_w,
        cell_height_px: cell_h,
    })
}

/// The engine's data directory — the same default the runner writes runs to
/// (mirrored here so the chart tool and the runner agree without talking).
fn data_dir() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("spatial-bench")
}
