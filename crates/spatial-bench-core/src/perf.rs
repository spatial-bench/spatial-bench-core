//! The perf runner (§11): same cases, different measurement.
//!
//! `perf stat` attributes counters to a PROCESS, so a perf run executes ONE
//! case per process: the built driver receives a single-case spec, and the
//! counters cover process start, tree build, warm-up and measurement together.
//! That is the design's stated model — [`crate::case::Case::estimate`] already
//! prices the per-process overhead — and it is why perf points carry the same
//! tags as criterion points with *different metrics* rather than different
//! tags: timing and counters join on the tag tuple.
//!
//! The counters land on the point's open metrics map (`cycles`,
//! `instructions`, `branch_misses`, `task_clock`). The driver still measures
//! latency itself, identically under both runners — the harness contract does
//! not change; perf only adds columns.

use crate::harness::{self, RunSpec};
use crate::schema::{Metric, Point};
use serde_json::Value;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

/// The counters merged onto a perf point, mapped from perf's event names.
///
/// Names vary across perf versions (`cpu-cycles` on this host, `cycles`
/// elsewhere) and acquire a `:u`/`:uP` suffix when the kernel limits
/// measurement to user space, so matching is on the name before the colon.
const COUNTERS: &[(&str, &str, &str)] = &[
    // perf event name, metric name, metric unit
    ("cycles", "cycles", "count"),
    ("cpu-cycles", "cycles", "count"),
    ("instructions", "instructions", "count"),
    ("branch-misses", "branch_misses", "count"),
    ("task-clock", "task_clock", "ms"),
];

/// Measure one point: run the built driver under `perf stat` with a
/// single-case spec on stdin, take the point off the driver's stdout, and
/// merge perf's counters onto it.
///
/// perf's JSON report is kept under the temp dir for post-mortems, mirroring
/// how the criterion measurement keeps its output files.
pub fn measure_point(program: &[String], spec: &RunSpec) -> Result<Point, String> {
    if spec.cases.len() != 1 {
        // The one-point-per-process model is the whole shape of a perf run;
        // a multi-case spec here would mean counters attributed to several
        // measurements at once, which is exactly what §11 forbids.
        return Err(format!(
            "a perf point measures exactly one case, got {}",
            spec.cases.len()
        ));
    }
    let work_dir = std::env::temp_dir().join(format!("spatial-bench-perf-{}", std::process::id()));
    std::fs::create_dir_all(&work_dir).map_err(|e| e.to_string())?;
    let report = work_dir.join(format!("{}.json", safe_id(&spec.cases[0].id)));

    let json = serde_json::to_string(spec).map_err(|e| e.to_string())?;
    let mut child = Command::new("perf")
        .args(["stat", "-j", "-o"])
        .arg(&report)
        .arg("--")
        .args(program)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not run perf (is it installed?): {e}"))?;
    child
        .stdin
        .take()
        .ok_or_else(|| "perf child stdin was not piped".to_owned())?
        .write_all(json.as_bytes())
        .map_err(|e| format!("writing the spec to perf failed: {e}"))?;
    let out = child
        .wait_with_output()
        .map_err(|e| format!("waiting for the perf run failed: {e}"))?;
    if !out.status.success() {
        // perf's own diagnostics are inherited on stderr; the report may or
        // may not exist depending on where it failed.
        return Err(format!(
            "the perf run failed with {:?}; perf's report is at {}",
            out.status.code(),
            report.display()
        ));
    }

    let mut points = harness::read_points(out.stdout.as_slice()).map_err(|e| format!("{e:?}"))?;
    if points.len() != 1 {
        return Err(format!(
            "a single-case spec must yield exactly one point, got {}",
            points.len()
        ));
    }
    let mut point = points.remove(0);
    merge_counters(&mut point, &report)?;
    Ok(point)
}

/// Read perf's JSON report and merge the known counters onto the point.
///
/// Unknown events, and counters perf could not collect (`<not counted>`),
/// are ignored rather than fatal: the metrics map is open, and a
/// partially-instrumented point whose tags still join beats a refused run.
pub fn merge_counters(point: &mut Point, report: &Path) -> Result<(), String> {
    let raw = std::fs::read_to_string(report)
        .map_err(|e| format!("reading perf's report {}: {e}", report.display()))?;
    for (name, value, unit) in counters_from_json(&raw)? {
        point.metrics.insert(
            name.to_owned(),
            Metric {
                point: value,
                lower: None,
                upper: None,
                unit: unit.to_owned(),
            },
        );
    }
    Ok(())
}

/// The counters a perf JSON report carries: metric name, value, unit — the
/// unit comes from [`COUNTERS`], which is why this returns the triple rather
/// than a bare value.
///
/// `perf stat -j` emits JSON **lines**: one object per event, one per line —
/// not a wrapped array. A line that does not parse is an error, not a skipped
/// counter: an unparsed counter is a hole in the measurement that nothing
/// downstream would report.
fn counters_from_json(raw: &str) -> Result<Vec<(&'static str, f64, &'static str)>, String> {
    let mut out = Vec::new();
    for (index, line) in raw.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let entry: Value = serde_json::from_str(line)
            .map_err(|e| format!("perf's report line {} is not JSON: {e}", index + 1))?;
        let Some(event) = entry.get("event").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(value) = entry.get("counter-value").and_then(counter_value) else {
            // `<not counted>` and friends: the event never ran.
            continue;
        };
        let base = event.split(':').next().unwrap_or(event);
        for (perf_name, metric, unit) in COUNTERS {
            if base == *perf_name {
                out.push((*metric, value, *unit));
            }
        }
    }
    Ok(out)
}

/// perf writes counter values as JSON strings, thousands separators included
/// on some versions, and `<not counted>` for events that never ran.
fn counter_value(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.replace(',', "").trim().parse::<f64>().ok(),
        _ => None,
    }
}

/// Criterion-style ids contain `:`, which no filename wants.
fn safe_id(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "case".to_owned()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tag::TagMap;
    use crate::tags;
    use std::collections::BTreeMap;

    /// Real perf output, captured on this machine (`perf stat -j -o …`), so
    /// the parser cannot drift from what this perf actually emits: string
    /// counter values, `:u` event suffixes, `cpu-cycles` naming, and a
    /// `<not counted>` entry.
    fn fixture() -> String {
        include_str!("../tests/fixtures/perf-stat.json").to_owned()
    }

    #[test]
    fn parses_the_known_counters_from_a_real_report() {
        let counters = counters_from_json(&fixture()).unwrap();
        let find = |name: &str| {
            counters
                .iter()
                .find(|(n, _, _)| *n == name)
                .copied()
                .unwrap_or_else(|| panic!("no {name} in {counters:?}"))
        };
        let (cycles, cycles_value, cycles_unit) = find("cycles");
        assert_eq!((cycles, cycles_unit), ("cycles", "count"));
        assert!(cycles_value > 0.0, "a real workload has cycles");

        assert!(find("instructions").1 > 0.0);
        assert!(find("task_clock").1 > 0.0);
        assert!(find("branch_misses").1 >= 0.0);

        // The mapped metric names only: no perf spellings leak through.
        assert!(!counters.iter().any(|(n, _, _)| *n == "cpu-cycles"));
    }

    /// `merge_counters` adds the counters to a point's open metrics map
    /// without touching the latency the driver measured — that join on the
    /// tag tuple is §11's whole point.
    #[test]
    fn counters_merge_onto_the_point_without_disturbing_latency() {
        let tmp = std::env::temp_dir().join(format!("sb-perf-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let report = tmp.join("perf-stat.json");
        std::fs::write(&report, fixture()).unwrap();

        let mut metrics: BTreeMap<String, Metric> = BTreeMap::new();
        metrics.insert(
            "latency_ns".to_owned(),
            Metric {
                point: 77.5,
                lower: Some(77.0),
                upper: Some(78.0),
                unit: "ns/query".to_owned(),
            },
        );
        let mut point = Point {
            tags: tags! { impl_: "kiddo", k: 1usize },
            metrics,
            stats: None,
            provenance: None,
        };

        merge_counters(&mut point, &report).unwrap();
        assert_eq!(point.metrics["latency_ns"].point, 77.5);
        assert!(point.metrics["cycles"].point > 0.0);
        assert!(point.metrics["instructions"].point > 0.0);
        assert_eq!(point.metrics["cycles"].unit, "count");
        assert_eq!(point.metrics["task_clock"].unit, "ms");
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// An unparseable report is an error, not a silently empty point — but it
    /// errors at the report read, leaving the point's own metrics intact.
    #[test]
    fn an_unreadable_report_is_an_error() {
        let tmp = std::env::temp_dir().join(format!("sb-perf-bad-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let report = tmp.join("perf-stat.json");
        std::fs::write(&report, "not json").unwrap();
        let mut point = Point {
            tags: TagMap::new(),
            metrics: BTreeMap::new(),
            stats: None,
            provenance: None,
        };
        assert!(merge_counters(&mut point, &report).is_err());
        std::fs::remove_dir_all(&tmp).ok();
    }
}
