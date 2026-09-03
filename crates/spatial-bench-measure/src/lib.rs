//! Criterion measurement for spatial-bench's Rust drivers.
//!
//! Measurement lives here, not in any subject's driver and not in core:
//! [`measure`] runs a case's timed region under criterion and converts its
//! estimates into a data point. Criterion is used *inside the engine's own
//! driver*, identically for every Rust subject (§11) — warm-up, outlier
//! filtering and confidence intervals are worth not reimplementing — while the
//! harness contract stays independent of it, so criterion can be replaced
//! without changing what a driver must emit.
//!
//! Its own crate rather than part of core, so core stays schema + catalog +
//! selector: a generated driver links core to emit the schema, and criterion
//! only because it measures.

use criterion::Criterion;
use serde::Deserialize;
use spatial_bench_core::harness::{Budget, CaseSpec};
use spatial_bench_core::schema::{Metric, Point, Provenance, Stats};
use spatial_bench_core::tag::TagMap;
use std::path::Path;
use std::time::Duration;

/// Criterion's `estimates.json`. Only the fields that become metrics or stats
/// are modelled; the rest is ignored so a criterion upgrade adding fields does
/// not break loading.
#[derive(Debug, Deserialize)]
pub struct Estimates {
    pub mean: Estimate,
    pub median: Option<Estimate>,
    pub median_abs_dev: Option<Estimate>,
    pub std_dev: Option<Estimate>,
    pub slope: Option<Estimate>,
}

#[derive(Debug, Deserialize)]
pub struct Estimate {
    pub point_estimate: f64,
    pub confidence_interval: ConfidenceInterval,
}

#[derive(Debug, Deserialize)]
pub struct ConfidenceInterval {
    pub confidence_level: f64,
    pub lower_bound: f64,
    pub upper_bound: f64,
}

/// Convert one criterion measurement into a data point.
///
/// `samples` is left absent: criterion records it in `sample.json` rather than
/// `estimates.json`, and reporting 0 would read as a real measurement of
/// nothing once the value reached a chart.
///
/// Criterion times a whole batch, so every metric is divided by the batch size
/// to give per-query figures. Reporting the batch total would make two runs at
/// different `query_count` incomparable, which is precisely what the tag model
/// exists to prevent.
pub fn to_point(
    estimates: &Estimates,
    tags: TagMap,
    batch_size: u64,
    criterion_id: Option<String>,
) -> Result<Point, ConvertError> {
    if batch_size == 0 {
        return Err(ConvertError::ZeroBatch);
    }
    let n = batch_size as f64;
    let mean = &estimates.mean;
    if !mean.point_estimate.is_finite() || mean.point_estimate <= 0.0 {
        return Err(ConvertError::NotAPositiveDuration(mean.point_estimate));
    }

    let per_query = mean.point_estimate / n;
    let mut metrics = std::collections::BTreeMap::new();
    metrics.insert(
        "latency_ns".to_owned(),
        Metric {
            point: per_query,
            lower: Some(mean.confidence_interval.lower_bound / n),
            upper: Some(mean.confidence_interval.upper_bound / n),
            unit: "ns/query".to_owned(),
        },
    );
    metrics.insert(
        "throughput_qps".to_owned(),
        Metric {
            point: 1e9 / per_query,
            lower: None,
            upper: None,
            unit: "queries/s".to_owned(),
        },
    );

    Ok(Point {
        tags,
        metrics,
        stats: Some(Stats {
            samples: None,
            ci: mean.confidence_interval.confidence_level,
            std_dev_ns: estimates.std_dev.as_ref().map(|e| e.point_estimate / n),
            median_ns: estimates.median.as_ref().map(|e| e.point_estimate / n),
            mad_ns: estimates
                .median_abs_dev
                .as_ref()
                .map(|e| e.point_estimate / n),
        }),
        provenance: Some(Provenance { criterion_id }),
    })
}

#[derive(Debug, PartialEq)]
pub enum ConvertError {
    ZeroBatch,
    NotAPositiveDuration(f64),
}

/// Measure one case under criterion and convert the result into a point.
///
/// `body` should run the whole query batch — criterion times the invocation,
/// and [`to_point`] divides the estimates by `batch` so two runs at different
/// `query_count` stay comparable. The return value of `body` (a checksum) is
/// black-boxed so the loop cannot be optimised away.
///
/// Criterion writes `<tmp>/spatial-bench-driver-<pid>/<id>/new/` and the
/// directory is left for post-mortems; it is under the temp dir, so the OS
/// reclaims it.
pub fn measure(
    case: &CaseSpec,
    budget: &Budget,
    batch: u64,
    mut body: impl FnMut() -> u64,
) -> Result<Point, String> {
    if batch == 0 {
        return Err("a case must run at least one query per sample".to_owned());
    }
    // Refused, not clamped: the budget is the harness contract, and a driver
    // that quietly measured with a different sample size than it was told
    // would put a number in the dataset that nothing explains . Criterion
    // panics on a zero duration and a sample size below 10; we refuse with
    // the reason instead.
    if budget.sample_size < 10 {
        return Err(format!(
            "budget.sample_size {} is below criterion's minimum of 10",
            budget.sample_size
        ));
    }
    if budget.warm_up_ms == 0 || budget.measurement_ms == 0 {
        return Err("budget warm-up and measurement times must be non-zero".to_owned());
    }
    let out_dir = std::env::temp_dir().join(format!("spatial-bench-driver-{}", std::process::id()));
    let id = safe_id(&case.id);

    let mut criterion = Criterion::default()
        .warm_up_time(Duration::from_millis(budget.warm_up_ms))
        .measurement_time(Duration::from_millis(budget.measurement_ms))
        .sample_size(budget.sample_size as usize)
        .output_directory(&out_dir);

    // Criterion's analysis prints to stdout — the stream that carries the
    // JSON-lines contract — so fd 1 goes to stderr for the duration.
    #[cfg(unix)]
    let _silence = StdoutToStderr::redirect();
    criterion.bench_function(&id, |b| b.iter(|| std::hint::black_box(body())));
    drop(criterion);
    #[cfg(unix)]
    drop(_silence);

    let dir = out_dir.join(&id).join("new");
    let estimates: Estimates = read_json(&dir.join("estimates.json"))?;
    let samples = sample_count(&dir.join("sample.json"));
    let mut point =
        to_point(&estimates, case.tags.clone(), batch, Some(id)).map_err(|e| format!("{e:?}"))?;
    if let Some(stats) = &mut point.stats {
        stats.samples = samples;
    }
    Ok(point)
}

/// Criterion directory names are the bench id verbatim, and the id also lands
/// in provenance, so both sides use the same sanitised form.
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

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("criterion wrote no {}: {e}", path.display()))?;
    serde_json::from_str(&raw).map_err(|e| format!("{}: {e}", path.display()))
}

/// The sample count lives in `sample.json` (`estimates.json` does not carry
/// it); absent, not zero — see [`to_point`].
fn sample_count(path: &Path) -> Option<u64> {
    let raw = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    value.get("times")?.as_array().map(|a| a.len() as u64)
}

/// Point fd 1 at fd 2 for the guard's lifetime.
///
/// **Invariants :** fd 1 always refers to either the harness pipe or fd 2 —
/// never closed, never left dangling; `saved` is moved into the guard and
/// closed exactly once, in `Drop`, which runs even if a benchmark panics
/// mid-measurement. If `dup` fails the guard does not engage and measurement
/// continues, only noisier; if `dup2` fails the saved fd is closed and fd 1
/// was never touched.
///
/// The swap is **process-global** and unprotected by any lock: any other
/// thread writing to stdout while the guard is held lands on stderr. Criterion
/// is single-threaded here, but if that ever changes, points and prose could
/// interleave across the fd boundary — the guard must move to a stricter
/// mechanism (or measurement move to its own process) before any concurrency
/// is introduced around it.
///
/// Rust's own buffered handle is line-buffered and flushed around the swap, so
/// a point emitted before the guard and one after cannot interleave with the
/// redirected output.
#[cfg(unix)]
struct StdoutToStderr {
    saved: i32,
}

#[cfg(unix)]
impl StdoutToStderr {
    fn redirect() -> Option<Self> {
        use std::io::Write;
        let _ = std::io::stdout().flush();
        // SAFETY: dup/dup2/close on well-known fds, restored in Drop even on
        // panic. Failure means the guard simply does not engage; measurement
        // continues, only noisier.
        unsafe {
            let saved = libc::dup(libc::STDOUT_FILENO);
            if saved < 0 {
                return None;
            }
            if libc::dup2(libc::STDERR_FILENO, libc::STDOUT_FILENO) < 0 {
                libc::close(saved);
                return None;
            }
            Some(Self { saved })
        }
    }
}

#[cfg(unix)]
impl Drop for StdoutToStderr {
    fn drop(&mut self) {
        use std::io::Write;
        unsafe {
            libc::dup2(self.saved, libc::STDOUT_FILENO);
            libc::close(self.saved);
        }
        let _ = std::io::stdout().flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spatial_bench_core::tags;

    /// Real criterion output, captured from a kiddo run on this machine rather
    /// than hand-written, so the shape cannot drift from what criterion emits.
    fn real_estimates() -> Estimates {
        let raw = include_str!("../tests/fixtures/criterion-estimates.json");
        serde_json::from_str(raw).expect("fixture should parse")
    }

    fn point_tags() -> TagMap {
        tags! { impl_: "kiddo_v6", query: "exact_nn", k: 20, dims: 3, axis: "f64" }
    }

    #[test]
    fn parses_real_criterion_estimates() {
        let e = real_estimates();
        assert!((e.mean.point_estimate - 1_054_172.84).abs() < 0.01);
        assert_eq!(e.mean.confidence_interval.confidence_level, 0.95);
        assert!(e.slope.is_some() && e.std_dev.is_some());
    }

    /// The batch total is divided out, so runs at different query counts stay
    /// comparable. 1054172.8ns over 1000 queries is ~1054ns each.
    #[test]
    fn reports_per_query_not_per_batch() {
        let p = to_point(&real_estimates(), point_tags(), 1000, None).unwrap();
        let latency = &p.metrics["latency_ns"];
        assert!(
            (latency.point - 1054.17).abs() < 0.01,
            "got {}",
            latency.point
        );
        assert!(latency.lower.unwrap() < latency.point);
        assert!(latency.upper.unwrap() > latency.point);
        assert_eq!(latency.unit, "ns/query");
    }

    #[test]
    fn throughput_is_the_reciprocal() {
        let p = to_point(&real_estimates(), point_tags(), 1000, None).unwrap();
        let qps = p.metrics["throughput_qps"].point;
        let latency = p.metrics["latency_ns"].point;
        assert!((qps - 1e9 / latency).abs() < 1.0);
        assert!((948_000.0..950_000.0).contains(&qps), "got {qps}");
    }

    #[test]
    fn stats_are_carried_through_per_query() {
        let p = to_point(&real_estimates(), point_tags(), 1000, None).unwrap();
        let stats = p.stats.unwrap();
        assert_eq!(stats.ci, 0.95);
        assert_eq!(stats.samples, None, "absent, not zero — see to_point");
        assert!((stats.median_ns.unwrap() - 1053.98).abs() < 0.01);
        assert!((stats.std_dev_ns.unwrap() - 0.8388).abs() < 0.001);
        assert!((stats.mad_ns.unwrap() - 0.9574).abs() < 0.001);
    }

    #[test]
    fn rejects_impossible_measurements() {
        // Point carries an open metrics map and is not PartialEq, so match on
        // the error rather than comparing Results.
        assert!(matches!(
            to_point(&real_estimates(), point_tags(), 0, None),
            Err(ConvertError::ZeroBatch)
        ));
    }

    /// Provenance is kept for traceability but is never the identity: the tags
    /// are.
    #[test]
    fn criterion_id_is_provenance_only() {
        let id = "profile_kiddo_vs_pkdtree/f64/kiddo_nearest_n_k20/1048576".to_owned();
        let p = to_point(&real_estimates(), point_tags(), 1000, Some(id.clone())).unwrap();
        assert_eq!(p.provenance.unwrap().criterion_id, Some(id));
        assert!(p.tags.contains_key("impl"), "identity lives in tags");
    }

    #[test]
    fn ids_are_sanitised_for_directory_names() {
        assert_eq!(safe_id("kiddo_v6:0"), "kiddo_v6_0");
        assert_eq!(safe_id("a/b c"), "a_b_c");
        assert_eq!(safe_id(""), "case");
    }

    /// The full path: a case measured under criterion becomes a point carrying
    /// per-query metrics, a confidence interval, the sample count, and the
    /// criterion id as provenance. A tiny budget keeps it quick; the numbers
    /// are asserted for shape, not value.
    #[test]
    fn measure_produces_a_complete_point() {
        use spatial_bench_core::harness::CaseSpec;

        let spec = CaseSpec {
            id: "kiddo_v6:0".into(),
            tags: tags! { impl_: "kiddo_v6", k: 1usize, query_count: 1000usize },
            point_seed: 1,
            query_seed: 2,
        };
        let budget = Budget {
            warm_up_ms: 10,
            measurement_ms: 100,
            sample_size: 10,
        };
        let point = measure(&spec, &budget, 1000, || 42).unwrap();

        let latency = &point.metrics["latency_ns"];
        assert!(latency.point > 0.0 && latency.point.is_finite());
        assert_eq!(latency.unit, "ns/query");
        assert!(latency.lower.unwrap() <= latency.point);
        assert!(latency.upper.unwrap() >= latency.point);
        let stats = point.stats.unwrap();
        assert_eq!(stats.ci, 0.95);
        assert!(
            stats.samples.unwrap() >= 10,
            "sample.json carries the count"
        );
        assert_eq!(
            point.provenance.unwrap().criterion_id,
            Some("kiddo_v6_0".to_owned())
        );

        // A zero batch is refused rather than dividing estimates by nothing.
        assert!(measure(&spec, &budget, 0, || 42).is_err());
    }

    /// an impossible budget is refused with the reason, not silently
    /// clamped into a measurement the contract did not ask for.
    #[test]
    fn an_impossible_budget_is_refused_not_clamped() {
        use spatial_bench_core::harness::CaseSpec;

        let spec = CaseSpec {
            id: "kiddo_v6:0".into(),
            tags: tags! { impl_: "kiddo_v6" },
            point_seed: 1,
            query_seed: 2,
        };
        let too_few_samples = Budget {
            warm_up_ms: 10,
            measurement_ms: 100,
            sample_size: 5,
        };
        let err = measure(&spec, &too_few_samples, 1000, || 42).unwrap_err();
        assert!(err.contains("sample_size") && err.contains("10"), "{err}");

        let zero_warm_up = Budget {
            warm_up_ms: 0,
            measurement_ms: 100,
            sample_size: 10,
        };
        assert!(measure(&spec, &zero_warm_up, 1000, || 42)
            .unwrap_err()
            .contains("non-zero"));
    }
}
