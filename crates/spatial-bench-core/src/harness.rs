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
pub const HARNESS_VERSION: u32 = 2;

/// Fixed seeds, so every subject in a run sees byte-identical data and two
/// runs are comparable. They are part of the contract, not a per-driver choice
/// and not a per-run setting: changing them changes every number ever
/// recorded, so they live here, next to the version that governs them.
pub const POINT_SEED: u64 = 0x5eed_0000_0000_0301;
pub const QUERY_SEED: u64 = 0x5eed_0000_0000_0302;

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
    /// The dataset generator binary path. The driver spawns it to obtain
    /// construction and query points (day-1.5 dataset work: the binary
    /// streams a binary header + raw points to stdout).
    pub dataset_generator: String,
    /// The dataset kind, passed to the generator.
    pub dataset: String,
    /// The random seed, passed to the generator.
    pub random_seed: u64,
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

/// Run a driver binary: the spec as JSON on stdin, points as JSON Lines on
/// stdout, diagnostics inherited on stderr. This is the transport every
/// single-binary adapter uses, so a failed driver's own words reach the
/// operator unfiltered.
pub fn drive(
    command: &mut std::process::Command,
    spec: &RunSpec,
) -> Result<Vec<Point>, HarnessError> {
    use std::io::Write;
    let named = format!("{command:?}");
    command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped());
    let mut child = command.spawn().map_err(|e| HarnessError::Spawn {
        binary: named.clone(),
        err: e.to_string(),
    })?;
    let json = serde_json::to_string(spec).map_err(|e| HarnessError::Malformed(e.to_string()))?;
    child
        .stdin
        .take()
        .ok_or_else(|| HarnessError::Io("driver stdin was not piped".to_owned()))?
        .write_all(json.as_bytes())
        .map_err(|e| HarnessError::Io(e.to_string()))?;
    let out = child
        .wait_with_output()
        .map_err(|e| HarnessError::Io(e.to_string()))?;
    if !out.status.success() {
        return Err(HarnessError::DriverExit {
            binary: named,
            code: out.status.code(),
        });
    }
    read_points(out.stdout.as_slice())
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

/// The driver's self-description mode (§13, `conform`). Invoked with the
/// `--list` argv flag, a driver writes one JSON object per monomorphisation it
/// contains — `{"compile_time": [["key", "value"], ...]}` — and exits without
/// reading stdin. `conform` parses this and asserts set-equality with the
/// manifest: the declared catalog is safe only while the binaries actually
/// agree with it.
pub fn write_registrations(
    mut writer: impl std::io::Write,
    registrations: impl Iterator<Item = Vec<(String, String)>>,
) -> Result<(), HarnessError> {
    for compile_time in registrations {
        let line = serde_json::to_string(&RegistrationListing { compile_time })
            .map_err(|e| HarnessError::Malformed(e.to_string()))?;
        writeln!(writer, "{line}").map_err(|e| HarnessError::Io(e.to_string()))?;
    }
    Ok(())
}

/// Parse a driver's registration list. Same line discipline as
/// [`read_points`]: blank lines are skipped; a parse failure is an error,
/// because a silently dropped registration would read as a conformance match.
pub fn read_registrations(
    reader: impl std::io::BufRead,
) -> Result<Vec<Vec<(String, String)>>, HarnessError> {
    let mut out = Vec::new();
    for (index, line) in reader.lines().enumerate() {
        let line = line.map_err(|e| HarnessError::Io(e.to_string()))?;
        if line.trim().is_empty() {
            continue;
        }
        let listing: RegistrationListing =
            serde_json::from_str(&line).map_err(|e| HarnessError::BadRegistration {
                line: index + 1,
                err: e.to_string(),
            })?;
        out.push(listing.compile_time);
    }
    Ok(out)
}

/// The wire shape of one `--list` entry. Tuples serialise as `[[k, v], ...]`.
#[derive(serde::Serialize, serde::Deserialize)]
struct RegistrationListing {
    compile_time: Vec<(String, String)>,
}

#[derive(Debug)]
pub enum HarnessError {
    Version {
        expected: u32,
        found: u32,
    },
    Malformed(String),
    BadPoint {
        line: usize,
        err: String,
    },
    /// A registration-list line (the driver's `--list` mode) failed to parse.
    BadRegistration {
        line: usize,
        err: String,
    },
    Io(String),
    /// The driver binary could not be spawned at all.
    Spawn {
        binary: String,
        err: String,
    },
    /// The driver exited non-zero; its diagnostics are on stderr, inherited.
    DriverExit {
        binary: String,
        code: Option<i32>,
    },
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
                dataset_generator: String::new(),
                dataset: "uniform".into(),
                random_seed: 42,
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
        assert_eq!(back.cases[0].random_seed, 42);
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
                expected: HARNESS_VERSION,
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

    // ---- the registration listing (§13, conform) --------------------------

    fn reg(k1: &str, v1: &str, k2: &str, v2: &str) -> Vec<(String, String)> {
        vec![
            (k1.to_owned(), v1.to_owned()),
            (k2.to_owned(), v2.to_owned()),
        ]
    }

    #[test]
    fn registrations_round_trip_as_json_lines() {
        let mut buf = Vec::new();
        let a = reg("axis", "f64", "kiddo.stem", "eytzinger");
        let b = reg("axis", "f32", "kiddo.stem", "donnelly");
        write_registrations(&mut buf, [a.clone(), b.clone()].into_iter()).unwrap();
        assert_eq!(read_registrations(buf.as_slice()).unwrap(), vec![a, b]);
    }

    /// Same line discipline as points: blank lines are nothing, a malformed
    /// line is an error rather than a silently dropped registration — a drop
    /// would read as a conformance match.
    #[test]
    fn registration_listing_rejects_malformed_lines() {
        assert_eq!(read_registrations(b"\n".as_slice()).unwrap().len(), 0);
        assert!(matches!(
            read_registrations(b"{\"nope\": 1}\n".as_slice()),
            Err(HarnessError::BadRegistration { line: 1, .. })
        ));
    }
}
