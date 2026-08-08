//! The criterion adapter.
//!
//! Drives **stock criterion** through its own filter argument. Because every
//! subject is vendored, no subject links or parses anything of ours to be
//! benchmarked -- which is the property that makes an unwilling subject
//! measurable at all.

use crate::schema::{Metric, Point, Provenance, Stats};
use crate::tag::TagMap;
use serde::Deserialize;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tags;

    /// Real criterion output, captured from a kiddo run on this machine rather
    /// than hand-written, so the shape cannot drift from what criterion emits.
    fn real_estimates() -> Estimates {
        let raw = include_str!("../../tests/fixtures/criterion-estimates.json");
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
}
