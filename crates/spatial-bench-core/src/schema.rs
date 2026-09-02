//! The on-disk result schema — see design §6.
//!
//! One document per run. `schema_version` gates every reader; charting tools
//! refuse anything they do not know.

use crate::machine::Machine;
use crate::tag::TagMap;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const SCHEMA_VERSION: u32 = 1;

/// UTC now, in the ISO extended form the schema and the fingerprint file
/// expect. Hand-rolled (Hinnant's civil-from-days, inverted) so the core does
/// not grow a time dependency for the one timestamp it needs.
pub fn utc_timestamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = secs / 86_400;
    let (h, m, s) = ((secs % 86_400) / 3600, (secs % 3600) / 60, secs % 60);
    let (y, mo, d) = civil_from_days(days as i64);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

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

/// A run id: a ULID — 26 Crockford-base32 characters carrying a 48-bit Unix
/// millisecond timestamp above 80 bits of randomness, so ids sort
/// lexicographically by time and stay unique across machines without
/// coordination. `result_path` keeps the first 8 characters, which are
/// timestamp-derived and therefore meaningful in a filename.
///
/// The randomness is blake3 over the sub-millisecond clock, the process id and
/// a process-wide counter — no new dependency — and a monotonic guard makes
/// consecutive ids within one process strictly increasing, so two ids in the
/// same millisecond never collide and never sort backwards. Uniqueness across
/// machines rests on the random bits, per the ULID spec.
pub fn ulid() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let ms = now.as_millis() & 0xffff_ffff_ffff; // 48 bits, per the spec

    static COUNTER: AtomicU64 = AtomicU64::new(0);
    static LAST: std::sync::Mutex<u128> = std::sync::Mutex::new(0);
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);

    let mut material = Vec::with_capacity(24);
    material.extend_from_slice(&now.subsec_nanos().to_be_bytes());
    material.extend_from_slice(&std::process::id().to_be_bytes());
    material.extend_from_slice(&count.to_be_bytes());
    let mut rand: u128 = 0;
    for byte in blake3::hash(&material).as_bytes().iter().take(10) {
        rand = (rand << 8) | u128::from(*byte);
    }

    // Monotonic within the process: two ids a microsecond apart must not sort
    // backwards, which bare randomness would allow inside one millisecond.
    let mut last = LAST.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let value = (ms << 80) | rand;
    if value <= *last {
        *last += 1; // Bumping carries into the time bits; it can never wrap.
    } else {
        *last = value;
    }
    encode(*last)
}

/// Encode a ULID value: MSB-first over 2 pad bits + 48 time bits + 80 random
/// bits, so the first character carries the 3 surviving time bits and the
/// last sixteen carry exactly the random bits. Split out so the monotonic
/// guard operates on the value while the rendering stays pure.
fn encode(value: u128) -> String {
    const ALPHABET: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let mut out = String::with_capacity(26);
    out.push(ALPHABET[(value >> 125) as usize] as char);
    for shift in (0..=120u32).rev().step_by(5) {
        out.push(ALPHABET[((value >> shift) & 0x1f) as usize] as char);
    }
    out
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Document {
    pub schema_version: u32,
    pub run: Run,
    pub points: Vec<Point>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Run {
    /// ULID — sorts by time, unique across machines; see [`ulid`].
    pub run_id: String,
    pub started_at: String,
    pub finished_at: String,
    pub runner: String,
    /// Canonical selector expressions, exactly as the repro line prints them.
    pub selectors: Vec<String>,
    /// The full continuity hash, `<unprivileged>-<privileged>` (or
    /// `<unprivileged>-UNKNOWN`). The dataset's join key: charts join on the
    /// whole string, never the prefix alone — the prefix alone would merge
    /// unverified runs into verified history. See `machine::Machine::hash`.
    pub machine_hash: String,
    pub machine: Machine,
    pub context: Context,
    pub toolchain: Toolchain,
    pub source: Source,
    /// What each subject actually was when this run built it (§4): without
    /// this, results separated by months are not comparable and nothing says
    /// why.
    #[serde(default)]
    pub subjects: BTreeMap<String, SubjectProvenance>,
}

/// A subject's build identity: the declared pin, the version cargo resolved,
/// and the revision it actually built.
///
/// A `--subject-path` build has no revision to record: it records the pin it
/// ignored and marks the version as a working tree, so the dataset can reject
/// it rather than treating it as reproducible.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SubjectProvenance {
    pub version: String,
    pub pinned_ref: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha: Option<String>,
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
    /// The fingerprint file this run validated against , when one
    /// existed. `None` — the run proceeded unverified, and the dataset should
    /// treat the machine hash accordingly.
    #[serde(default)]
    pub fingerprint: Option<String>,
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
                fingerprint: None,
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
            subjects: BTreeMap::new(),
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

    // ---- run ids ----------------------------------------------------------

    const CROCKFORD: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

    /// The first 10 characters carry the 48-bit millisecond timestamp: digit 0
    /// occupies bits 47..=45, digit i occupies 45-5i down.
    fn decode_ms(id: &str) -> u128 {
        id.chars()
            .take(10)
            .enumerate()
            .map(|(i, c)| {
                let digit = CROCKFORD
                    .iter()
                    .position(|a| *a == c as u8)
                    .expect("crockford");
                u128::from(digit as u32) << (45 - 5 * i)
            })
            .sum()
    }

    fn now_ms() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    }

    #[test]
    fn ulids_are_26_crockford_characters() {
        let id = ulid();
        assert_eq!(id.len(), 26, "{id}");
        assert!(id.chars().all(|c| CROCKFORD.contains(&(c as u8))), "{id}");
    }

    /// The whole point of the format: ids order by time, so a dataset or a
    /// directory listing reads chronologically.
    #[test]
    fn ulids_sort_by_time() {
        let now = now_ms();
        let id = ulid();
        let decoded = decode_ms(&id);
        assert!(
            decoded.abs_diff(now) < 5_000,
            "decoded {decoded} vs wall clock {now}"
        );
    }

    /// Consecutive ids in one process never sort backwards and never collide,
    /// including within a single millisecond — the monotonic guard.
    #[test]
    fn consecutive_ulids_are_strictly_increasing() {
        let a = ulid();
        let b = ulid();
        assert!(a < b, "{a} then {b}");
        assert_ne!(a, b);

        // And the increase is in the value, not just the string: the pair
        // remains time-ordered when decoded.
        assert!(decode_ms(&a) <= decode_ms(&b));
    }

    /// `result_path` ties the filename to the run id via its first eight
    /// characters — with a ULID those are timestamp-derived, which is the
    /// design's intent: the suffix is meaningful, and two real runs cannot
    /// share them, since each takes far longer than the millisecond they
    /// encode. (Uniqueness within a millisecond lives in the id itself — see
    /// `consecutive_ulids_are_strictly_increasing` — not in the filename.)
    #[test]
    fn a_generated_run_id_names_its_file() {
        let mut r = run();
        r.run_id = ulid();
        let path = result_path(std::path::Path::new("d"), &r).unwrap();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let first8: String = r.run_id.chars().take(8).collect();
        assert!(
            name.ends_with(&format!("-{first8}.json")),
            "{name} should end with {first8}"
        );
    }
}
