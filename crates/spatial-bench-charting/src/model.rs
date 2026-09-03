//! The chart model: run documents → plottable series.
//!
//! Pure data logic, no IO and no rendering — the same shape a future
//! web front-end would consume. A chart is one or more series of (x, y, CI)
//! points; the selector language picks the points, the series tag groups
//! them, and latest-wins dedupe keeps one point per (series, x).

use serde_json::Value;
use spatial_bench_core::selector::SelectorSet;
use spatial_bench_core::tag::{TagMap, TagValue};
use std::collections::BTreeMap;
use std::path::Path;

/// One run document's metadata, as far as the chart cares.
#[derive(Debug, Clone)]
#[allow(dead_code)] // the front-end contract: the fields are the schema
pub struct RunMeta {
    pub run_id: String,
    pub started_at: String,
    pub runner: String,
    pub machine_hash: String,
}

/// A selected measurement: which series it belongs to, its x position, the
/// metric value, and the CI bounds when the document carried them.
#[derive(Debug, Clone)]
#[allow(dead_code)] // series/x/run_id are the front-end contract
pub struct Measurement {
    pub series: String,
    pub x: f64,
    pub y: f64,
    pub lower: Option<f64>,
    pub upper: Option<f64>,
    pub run_id: String,
    /// Which metric this measurement is — the chart plots one.
    pub metric_name: String,
    /// The point's full tag map, for selector matching and series/x lookup.
    pub tags: TagMap,
}

/// What one run document contributed, as far as loading cares.
#[derive(Debug, Clone)]
pub struct LoadedDoc {
    pub meta: RunMeta,
    pub measurements: Vec<Measurement>,
}

#[derive(Debug)]
pub struct LoadError {
    pub path: std::path::PathBuf,
    pub message: String,
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path.display(), self.message)
    }
}

/// Parse one run document. The document's own `schema_version` gate refuses
/// anything this chart renderer does not know.
pub fn load_document(path: &Path) -> Result<LoadedDoc, LoadError> {
    let bad = |message: String| LoadError {
        path: path.to_owned(),
        message,
    };
    let text = std::fs::read_to_string(path).map_err(|e| bad(format!("cannot read: {e}")))?;
    let doc: Value = serde_json::from_str(&text).map_err(|e| bad(format!("bad JSON: {e}")))?;
    let schema = doc
        .get("schema_version")
        .and_then(Value::as_u64)
        .ok_or_else(|| bad("missing schema_version".to_owned()))?;
    if schema != u64::from(spatial_bench_core::schema::SCHEMA_VERSION) {
        return Err(bad(format!("schema {schema} is not ours")));
    }
    let run = doc
        .get("run")
        .ok_or_else(|| bad("missing run".to_owned()))?;
    let meta = RunMeta {
        run_id: run
            .get("run_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        started_at: run
            .get("started_at")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        runner: run
            .get("runner")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        machine_hash: run
            .get("machine_hash")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
    };

    let mut measurements = Vec::new();
    let points = doc
        .get("points")
        .and_then(Value::as_array)
        .ok_or_else(|| bad("missing points".to_owned()))?;
    for point in points {
        let Some(tags_value) = point.get("tags") else {
            continue;
        };
        let Ok(tags) = serde_json::from_value::<TagMap>(tags_value.clone()) else {
            continue;
        };
        let Some(metrics) = point.get("metrics").and_then(Value::as_object) else {
            continue;
        };
        for (name, m) in metrics {
            measurements.push(Measurement {
                series: tags
                    .get("impl")
                    .map(ToString::to_string)
                    .unwrap_or_default(),
                x: 0.0,
                y: m.get("point").and_then(Value::as_f64).unwrap_or(f64::NAN),
                lower: m.get("lower").and_then(Value::as_f64),
                upper: m.get("upper").and_then(Value::as_f64),
                run_id: meta.run_id.clone(),
                metric_name: name.clone(),
                tags: tags.clone(),
            });
        }
    }
    Ok(LoadedDoc { meta, measurements })
}

/// Load every run document under `dirs`, newest first, capped at `latest`.
///
/// Individual unreadable documents are skipped with a warning — a half-finished
/// file in the runs directory must not poison the chart. Only when nothing at
/// all loads does the caller see an error.
pub fn load_runs(dirs: &[std::path::PathBuf], latest: usize) -> Result<Vec<LoadedDoc>, LoadError> {
    let mut files: Vec<(String, std::path::PathBuf)> = Vec::new();
    for dir in dirs {
        // §10's layout is `<root>/YYYY-MM/<doc>.json` — scan one level deep.
        let scan = |d: &Path, files: &mut Vec<(String, std::path::PathBuf)>| {
            let Ok(entries) = std::fs::read_dir(d) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "json") {
                    // The filename embeds the UTC timestamp as its first field, so
                    // sorting by name sorts by time (§10's layout).
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    files.push((name, path));
                }
            }
        };
        scan(dir, &mut files);
        let Ok(months) = std::fs::read_dir(dir) else {
            continue;
        };
        for month in months.flatten() {
            let month_dir = month.path();
            if month_dir.is_dir() {
                scan(&month_dir, &mut files);
            }
        }
    }
    files.sort_by(|a, b| b.0.cmp(&a.0));
    files.truncate(latest);

    let mut docs = Vec::new();
    let mut errors = Vec::new();
    for (_, path) in files {
        match load_document(&path) {
            Ok(doc) => docs.push(doc),
            Err(e) => errors.push(e),
        }
    }
    if docs.is_empty() && !errors.is_empty() {
        return Err(errors.remove(0));
    }
    for e in errors {
        eprintln!("warning: skipped a run document: {e}");
    }
    Ok(docs)
}

/// A chart's data: one series of (x, y, CI lower, CI upper) points, sorted
/// by x, ready to plot.
#[derive(Debug, Clone)]
pub struct Series {
    pub name: String,
    pub points: Vec<(f64, f64, Option<f64>, Option<f64>)>,
}

/// Build the chart data from loaded runs.
///
/// - `selector` filters points by their tag maps (the same grammar as runs).
/// - `metric` picks which metric to plot.
/// - `x_tag` is the param axis; `None` gives every point x = 0 (bars mode).
/// - `series_tag` groups the points (default `impl`).
/// - Documents arrive newest-first and the first point seen for a
///   (series, x) pair wins — latest-wins dedupe.
pub fn build_series(
    docs: &[LoadedDoc],
    selector: &SelectorSet,
    metric: &str,
    x_tag: Option<&str>,
    series_tag: &str,
) -> Vec<Series> {
    let mut latest: BTreeMap<(String, u64), Measurement> = BTreeMap::new();
    for doc in docs {
        for m in &doc.measurements {
            if m.metric_name != metric {
                continue;
            }
            if !selector.admits_point(&m.tags) {
                continue;
            }
            let series = m
                .tags
                .get(series_tag)
                .map(ToString::to_string)
                .unwrap_or_default();
            let x = match x_tag {
                Some(tag) => m.tags.get(tag).and_then(tag_as_f64).unwrap_or(0.0),
                None => 0.0,
            };
            // f64 has no Ord; the dedupe key orders on the bit pattern
            // (total_cmp's ordering), which is all the latest-wins rule needs.
            let x_key = x.to_bits();
            latest.entry((series, x_key)).or_insert_with(|| m.clone());
        }
    }

    type PointList = Vec<(f64, f64, Option<f64>, Option<f64>)>;
    let mut by_series: BTreeMap<String, PointList> = BTreeMap::new();
    for ((series, x_bits), m) in latest {
        let x = f64::from_bits(x_bits);
        by_series
            .entry(series)
            .or_default()
            .push((x, m.y, m.lower, m.upper));
    }
    let mut out: Vec<Series> = by_series
        .into_iter()
        .map(|(name, mut points)| {
            points.sort_by(|a, b| a.0.total_cmp(&b.0));
            Series { name, points }
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn tag_as_f64(value: &TagValue) -> Option<f64> {
    match value {
        TagValue::Int(i) => Some(*i as f64),
        TagValue::Float(f) => Some(f.0),
        _ => None,
    }
}
