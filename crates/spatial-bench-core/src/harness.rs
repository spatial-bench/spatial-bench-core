//! The harness contract: what the engine sends a driver, and what a driver must
//! send back.
//!
//! This is the comparability guarantee. Because the engine owns the harness for
//! every subject, every driver receives the same generator seeds, the same tree
//! sizes and the same query counts, and reports the same shape. A number from
//! kiddo and a number from nanoflann mean the same thing only because both went
//! through this.
//!
//! Transport is deliberately dull: a `RunSpec` as JSON on stdin, [`Point`]s as
//! JSON Lines on stdout, diagnostics on stderr. Any language can implement it.

use crate::schema::Point;
use crate::tag::TagMap;
use serde::{Deserialize, Serialize};

/// Bumped when the contract changes shape. A driver refuses a spec it does not
/// recognise rather than guessing.
pub const HARNESS_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunSpec {
    pub harness_version: u32,
    pub budget: Budget,
    pub cases: Vec<CaseSpec>,
}

/// One data point to measure. Its tags are already fully resolved: the compile
/// time axes were baked in when the driver was generated, and the runtime axes
/// are here.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CaseSpec {
    pub id: String,
    pub tags: TagMap,
    /// Seeds are supplied rather than chosen by the driver, so two subjects
    /// measured in the same run see byte-identical data.
    pub point_seed: u64,
    pub query_seed: u64,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Budget {
    pub warm_up_ms: u64,
    pub measurement_ms: u64,
    pub sample_size: u32,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            warm_up_ms: 3_000,
            measurement_ms: 5_000,
            sample_size: 30,
        }
    }
}

impl CaseSpec {
    /// A runtime tag as an integer, e.g. `tree_size`.
    pub fn int(&self, key: &str) -> Option<i64> {
        match self.tags.get(key) {
            Some(crate::tag::TagValue::Int(i)) => Some(*i),
            _ => None,
        }
    }

    /// A runtime tag as a string, e.g. `query`.
    pub fn word(&self, key: &str) -> Option<String> {
        self.tags.get(key).map(ToString::to_string)
    }
}

/// Read a spec from stdin. Drivers call this; it is here so every driver agrees
/// on framing and version checking.
pub fn read_spec(reader: impl std::io::Read) -> Result<RunSpec, HarnessError> {
    let spec: RunSpec =
        serde_json::from_reader(reader).map_err(|e| HarnessError::Malformed(e.to_string()))?;
    if spec.harness_version != HARNESS_VERSION {
        return Err(HarnessError::Version {
            expected: HARNESS_VERSION,
            found: spec.harness_version,
        });
    }
    Ok(spec)
}

/// Write one point as a line of JSON.
pub fn write_point(mut writer: impl std::io::Write, point: &Point) -> Result<(), HarnessError> {
    let line = serde_json::to_string(point).map_err(|e| HarnessError::Malformed(e.to_string()))?;
    writeln!(writer, "{line}").map_err(|e| HarnessError::Io(e.to_string()))
}

/// Parse a driver's JSON Lines output back into points.
///
/// Blank lines are skipped; anything else that fails to parse is an error
/// rather than a silent drop, because a dropped point is a hole in a comparison
/// that nothing else would report.
pub fn read_points(reader: impl std::io::BufRead) -> Result<Vec<Point>, HarnessError> {
    let mut out = Vec::new();
    for (index, line) in reader.lines().enumerate() {
        let line = line.map_err(|e| HarnessError::Io(e.to_string()))?;
        if line.trim().is_empty() {
            continue;
        }
        let point: Point = serde_json::from_str(&line).map_err(|e| HarnessError::BadPoint {
            line: index + 1,
            err: e.to_string(),
        })?;
        out.push(point);
    }
    Ok(out)
}

#[derive(Debug)]
pub enum HarnessError {
    Version { expected: u32, found: u32 },
    Malformed(String),
    BadPoint { line: usize, err: String },
    Io(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::Metric;
    use crate::tags;

    fn spec() -> RunSpec {
        RunSpec {
            harness_version: HARNESS_VERSION,
            budget: Budget::default(),
            cases: vec![CaseSpec {
                id: "kiddo_v6:0".into(),
                tags: tags! {
                    impl_: "kiddo_v6", query: "exact_nn", k: 1, dims: 3, axis: "f64",
                    tree_size: 1_048_576usize, query_count: 1000usize,
                },
                point_seed: 0x5eed_0301,
                query_seed: 0x5eed_0302,
            }],
        }
    }

    #[test]
    fn spec_round_trips() {
        let json = serde_json::to_string(&spec()).unwrap();
        let back = read_spec(json.as_bytes()).unwrap();
        assert_eq!(back.cases[0].id, "kiddo_v6:0");
        assert_eq!(back.cases[0].int("tree_size"), Some(1 << 20));
        assert_eq!(back.cases[0].word("query").as_deref(), Some("exact_nn"));
        assert_eq!(back.cases[0].point_seed, 0x5eed_0301);
    }

    /// A driver built against an older contract must refuse, not guess.
    #[test]
    fn a_mismatched_contract_version_is_refused() {
        let mut s = spec();
        s.harness_version = 99;
        let json = serde_json::to_string(&s).unwrap();
        assert!(matches!(
            read_spec(json.as_bytes()),
            Err(HarnessError::Version {
                expected: 1,
                found: 99
            })
        ));
    }

    fn point() -> Point {
        Point {
            tags: tags! { impl_: "kiddo_v6", k: 1 },
            metrics: [(
                "latency_ns".to_string(),
                Metric {
                    point: 131.2,
                    lower: Some(131.0),
                    upper: Some(131.4),
                    unit: "ns/query".into(),
                },
            )]
            .into_iter()
            .collect(),
            stats: None,
            provenance: None,
        }
    }

    #[test]
    fn points_round_trip_as_json_lines() {
        let mut buf = Vec::new();
        write_point(&mut buf, &point()).unwrap();
        write_point(&mut buf, &point()).unwrap();
        let back = read_points(buf.as_slice()).unwrap();
        assert_eq!(back.len(), 2);
        assert!((back[0].metrics["latency_ns"].point - 131.2).abs() < 1e-9);
    }

    /// A point that fails to parse is an error, not a silent drop: a missing
    /// point is a hole in a comparison that nothing else would report.
    #[test]
    fn an_unparseable_line_is_an_error() {
        let input = b"{\"not\": \"a point\"}\n" as &[u8];
        assert!(matches!(
            read_points(input),
            Err(HarnessError::BadPoint { line: 1, .. })
        ));
    }

    #[test]
    fn blank_lines_are_skipped() {
        let mut buf = Vec::new();
        write_point(&mut buf, &point()).unwrap();
        buf.extend_from_slice(b"\n\n");
        assert_eq!(read_points(buf.as_slice()).unwrap().len(), 1);
    }
}
