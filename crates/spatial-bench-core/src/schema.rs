//! The on-disk result schema — see design §6.
//!
//! One document per run. `schema_version` gates every reader; charting tools
//! refuse anything they do not know.

use crate::machine::Machine;
use crate::tag::TagMap;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Document {
    pub schema_version: u32,
    pub run: Run,
    pub points: Vec<Point>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Run {
    /// ULID — sorts by time, unique across machines.
    pub run_id: String,
    pub started_at: String,
    pub finished_at: String,
    pub runner: String,
    /// Canonical selector expressions, exactly as the repro line prints them.
    pub selectors: Vec<String>,
    pub machine_hash: String,
    pub machine: Machine,
    pub context: Context,
    pub toolchain: Toolchain,
    pub source: Source,
}

/// Run-level environment. Deliberately NOT part of point identity — this is what
/// lets one chart overlay the same case across kernels or boot profiles.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Context {
    pub kernel: Option<String>,
    pub os: Option<String>,
    /// From `bench-profile-status`, `None` on an unlabelled boot.
    pub bench_profile: Option<String>,
    pub governor: Option<String>,
    pub smt: Option<bool>,
    pub boost: Option<bool>,
    pub isolated_cpus: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Toolchain {
    pub rustc: String,
    pub host: String,
    pub target_cpu: Option<String>,
    pub rustflags: Option<String>,
    pub features: Vec<String>,
    pub cargo_profile: String,
    pub opt_level: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Source {
    pub git_sha: Option<String>,
    pub git_dirty: bool,
    pub crate_version: String,
}

/// A measured value with an optional confidence interval.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Metric {
    pub point: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lower: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upper: Option<f64>,
    pub unit: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Stats {
    /// Criterion keeps the sample count in `sample.json`, not `estimates.json`,
    /// so it is absent when only estimates were read. `None` rather than 0: a
    /// zero would read as a real measurement of nothing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub samples: Option<u64>,
    pub ci: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub std_dev_ns: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub median_ns: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mad_ns: Option<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Point {
    /// Complete identity. Charts group by this and never parse a name.
    pub tags: TagMap,
    /// Open map so the perf runner can add `cycles`, `instructions`,
    /// `branch_misses` … to a point carrying identical tags, letting timing and
    /// counter data join on the tag tuple.
    pub metrics: BTreeMap<String, Metric>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stats: Option<Stats>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provenance: Option<Provenance>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Provenance {
    /// The criterion id this came from, kept only for traceability.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub criterion_id: Option<String>,
}

/// Where a run's document belongs:
/// `<root>/YYYY-MM/<timestamp>-<machine>-<runner>-<runid>.json`
///
/// The timestamp is ISO basic form (no separators) so it sorts
/// lexicographically by time and contains no colon, which is illegal in Windows
/// filenames -- the charting tools read this dataset from anywhere, even though
/// runs only happen on Linux.
pub fn result_path(root: &std::path::Path, run: &Run) -> Result<std::path::PathBuf, PathError> {
    let (month, stamp) = basic_timestamp(&run.started_at)?;
    let runid: String = run.run_id.chars().take(8).collect();
    if runid.is_empty() {
        return Err(PathError::MissingRunId);
    }
    let runner = slug(&run.runner);
    let machine = slug(&run.machine_hash);
    Ok(root
        .join(month)
        .join(format!("{stamp}-{machine}-{runner}-{runid}.json")))
}

/// `2026-08-04T12:34:56.789Z` -> (`2026-08`, `20260804T123456Z`).
///
/// Sub-second precision is dropped: the run id already guarantees uniqueness,
/// and a millisecond in the name implies the timestamp is more precise about
/// when a multi-minute run happened than it really is.
fn basic_timestamp(iso: &str) -> Result<(String, String), PathError> {
    let bad = || PathError::BadTimestamp(iso.to_owned());
    let (date, rest) = iso.split_once('T').ok_or_else(bad)?;
    let time = rest.split(['.', 'Z', '+']).next().ok_or_else(bad)?;

    let date_digits: String = date.chars().filter(char::is_ascii_digit).collect();
    let time_digits: String = time.chars().filter(char::is_ascii_digit).collect();
    if date_digits.len() != 8 || time_digits.len() != 6 {
        return Err(bad());
    }
    let month = format!("{}-{}", &date_digits[..4], &date_digits[4..6]);
    Ok((month, format!("{date_digits}T{time_digits}Z")))
}

/// Keep a path component to characters that are safe everywhere.
fn slug(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    if cleaned.is_empty() {
        "unknown".to_owned()
    } else {
        cleaned
    }
}

#[derive(Debug, PartialEq)]
pub enum PathError {
    BadTimestamp(String),
    MissingRunId,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run() -> Run {
        Run {
            run_id: "01K2Y7F3M8QWABCD".into(),
            started_at: "2026-08-04T12:34:56.789Z".into(),
            finished_at: "2026-08-04T12:51:02.114Z".into(),
            runner: "criterion".into(),
            selectors: vec![],
            machine_hash: "k7f2qa".into(),
            machine: crate::machine::Machine::unknown(),
            context: Context {
                kernel: None,
                os: None,
                bench_profile: None,
                governor: None,
                smt: None,
                boost: None,
                isolated_cpus: None,
            },
            toolchain: Toolchain {
                rustc: "1.89.0".into(),
                host: "x86_64-unknown-linux-gnu".into(),
                target_cpu: None,
                rustflags: None,
                features: vec![],
                cargo_profile: "bench".into(),
                opt_level: None,
            },
            source: Source {
                git_sha: None,
                git_dirty: false,
                crate_version: "0".into(),
            },
        }
    }

    #[test]
    fn builds_a_sortable_colon_free_path() {
        let path = result_path(std::path::Path::new("datasets"), &run()).unwrap();
        assert_eq!(
            path,
            std::path::Path::new(
                "datasets/2026-08/20260804T123456Z-k7f2qa-criterion-01K2Y7F3.json"
            )
        );
        assert!(
            !path.to_string_lossy().contains(':'),
            "colons are illegal on Windows"
        );
    }

    /// Lexical order must equal chronological order, or a dataset directory
    /// cannot be browsed or bisected by time.
    #[test]
    fn lexical_order_is_chronological() {
        let mut a = run();
        a.started_at = "2026-08-04T09:00:00Z".into();
        let mut b = run();
        b.started_at = "2026-08-04T12:34:56.789Z".into();
        let mut c = run();
        c.started_at = "2026-12-31T23:59:59Z".into();
        let root = std::path::Path::new("d");
        let mut paths: Vec<String> = [&a, &b, &c]
            .iter()
            .map(|r| result_path(root, r).unwrap().to_string_lossy().into_owned())
            .collect();
        let sorted = {
            let mut p = paths.clone();
            p.sort();
            p
        };
        assert_eq!(paths, sorted);
        paths.dedup();
        assert_eq!(paths.len(), 3);
    }

    #[test]
    fn month_directories_bound_directory_size() {
        let mut r = run();
        r.started_at = "2026-12-31T23:59:59Z".into();
        let path = result_path(std::path::Path::new("d"), &r).unwrap();
        assert!(path.starts_with("d/2026-12"));
    }

    #[test]
    fn rejects_a_timestamp_it_cannot_read() {
        for bad in ["not-a-time", "2026-08-04", "2026-08-04T12:34"] {
            let mut r = run();
            r.started_at = bad.into();
            assert!(result_path(std::path::Path::new("d"), &r).is_err(), "{bad}");
        }
    }
}
