//! `conform` (§13, check 3): does the built driver contain exactly what the
//! manifest declares?
//!
//! The catalog is declared rather than registered, which is safe only while
//! the binaries agree with the declarations. The drift-guard test (§13 check
//! 2) proves the generated source *compiles*; this module proves the compiled
//! driver *contains exactly* the manifest's cases — a subject that renamed a
//! strategy, or a manifest that grew a case the driver never grew, is caught
//! here rather than as a hole in a comparison.
//!
//! Conform builds but measures nothing, so unlike a run it does not require a
//! machine fingerprint: it makes no claim about numbers, only about coverage.

use crate::adapter;
use crate::case::Case;
use crate::catalog::Catalog;
use crate::toolchain;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Inputs for checking drivers through the shared source resolution and build path.
pub struct ConformConfig<'a> {
    pub catalog: &'a Catalog,
    /// Check one subject; every subject that declares cases when `None`.
    pub subject: Option<&'a str>,
    /// Where generated packages are built. Engine output, never sources.
    pub build_root: PathBuf,
    /// An engine source checkout for driver path deps, if one is reachable.
    pub engine_root: Option<PathBuf>,
    /// Working-tree overrides: subject name → directory.
    pub subject_paths: &'a BTreeMap<String, PathBuf>,
}

/// One subject's conformance outcome.
/// One compile-time coordinate: the sorted (key, value) pairs that identify a
/// monomorphisation, as `Case::compile_time_key` builds them.
pub type CaseKey = Vec<(String, String)>;

#[derive(Debug)]
pub struct SubjectConformance {
    pub subject: String,
    /// Manifest cases whose compile-time key the driver contains.
    pub manifest_cases: usize,
    /// Registrations the driver listed.
    pub driver_registrations: usize,
    /// Declared cases the driver does not contain — missing monomorphisations.
    pub missing_in_driver: Vec<CaseKey>,
    /// Registrations no manifest case declares — a stale or over-built driver.
    pub extra_in_driver: Vec<CaseKey>,
}

impl SubjectConformance {
    pub fn is_match(&self) -> bool {
        self.missing_in_driver.is_empty() && self.extra_in_driver.is_empty()
    }
}

#[derive(Debug)]
pub struct ConformReport {
    pub subjects: Vec<SubjectConformance>,
}

impl ConformReport {
    /// Every checked subject matches its manifest.
    pub fn all_match(&self) -> bool {
        self.subjects.iter().all(SubjectConformance::is_match)
    }
}

/// Check each selected subject's driver against its manifest (§13, check 3).
pub fn conform(config: &ConformConfig<'_>) -> Result<ConformReport, String> {
    let catalog = config.catalog;
    let mut names: Vec<String> = catalog.cases().iter().map(|c| c.subject.clone()).collect();
    names.sort();
    names.dedup();
    if let Some(filter) = config.subject {
        if !names.iter().any(|n| n == filter) {
            return Err(format!(
                "no subject named `{filter}` in the catalog (subjects: {})",
                names.join(", ")
            ));
        }
        names.retain(|n| n == filter);
    }

    // One toolchain across everything conform builds (§4), resolved exactly as
    // a run would resolve it for the same subjects.
    let floors: Vec<(String, Option<String>)> = names
        .iter()
        .map(|s| (s.clone(), catalog.min_rustc(s)))
        .collect();
    let toolchain = toolchain::resolve(&floors, None).map_err(|e| format!("{e:?}"))?;

    let mut subjects = Vec::new();
    for subject in &names {
        let cases: Vec<&Case> = catalog.for_subject(subject).collect();
        let Some(first) = cases.first() else {
            continue;
        };
        let request = crate::resolve::subject_request(
            &crate::resolve::Inputs {
                catalog,
                engine_root: config.engine_root.as_ref(),
                subject_paths: config.subject_paths,
            },
            subject,
            first,
            &config.build_root,
            toolchain,
            catalog.rustflags(subject),
        )?;
        let prepared = adapter::prepare(&request, &cases).map_err(|e| e.to_string())?;
        // adapter::list compiles the rust driver (conform claims coverage, so
        // the binary must build) or reuses the exec environment built above;
        // the --list protocol is the same either way.
        let actual = adapter::list(&prepared).map_err(|e| e.to_string())?;

        // The manifest side: one compile-time key per declared case.
        let mut expected: Vec<Vec<(String, String)>> =
            cases.iter().map(|c| c.compile_time_key()).collect();
        expected.sort();
        expected.dedup();

        let (missing_in_driver, extra_in_driver) = compare_sets(&expected, &actual);
        subjects.push(SubjectConformance {
            subject: subject.clone(),
            manifest_cases: expected.len(),
            driver_registrations: actual.len(),
            missing_in_driver,
            extra_in_driver,
        });
    }
    Ok(ConformReport { subjects })
}

/// Set-equality, both directions, and order-insensitive: the driver lists its
/// pairs in whatever order its registration macro provides, the manifest sorts
/// by namespaced key — conform caught exactly this mismatch on its first run —
/// so each side's pairs are canonicalised before comparing. A declared case
/// with no registration is a missing monomorphisation; a registration with no
/// declared case is a stale or over-built driver. Both are drift; neither is
/// silently absorbed.
fn compare_sets(expected: &[CaseKey], actual: &[CaseKey]) -> (Vec<CaseKey>, Vec<CaseKey>) {
    let canonical = |keys: &[CaseKey]| -> Vec<CaseKey> {
        let mut sorted: Vec<CaseKey> = keys.to_vec();
        for key in &mut sorted {
            key.sort();
        }
        sorted.sort();
        sorted.dedup();
        sorted
    };
    let expected = canonical(expected);
    let actual = canonical(actual);
    let missing = expected
        .iter()
        .filter(|e| !actual.contains(e))
        .cloned()
        .collect();
    let extra = actual
        .iter()
        .filter(|a| !expected.contains(a))
        .cloned()
        .collect();
    (missing, extra)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(flat: &[(&str, &str)]) -> Vec<(String, String)> {
        flat.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    /// The three conformance outcomes: equal sets, a declared case the driver
    /// lacks, and a registration the manifest does not declare. Both drift
    /// directions must surface — either one alone would hide a hole or a lie.
    #[test]
    fn set_equality_reports_both_drift_directions() {
        let e1 = pairs(&[("axis", "f64"), ("kiddo.stem", "eytzinger")]);
        let e2 = pairs(&[("axis", "f32"), ("kiddo.stem", "eytzinger")]);
        let expected = vec![e1.clone(), e2.clone()];

        // Match.
        let (missing, extra) = compare_sets(&expected, &[e2.clone(), e1.clone()]);
        assert!(missing.is_empty() && extra.is_empty());

        // A declared case the driver does not contain.
        let (missing, extra) = compare_sets(&expected, std::slice::from_ref(&e1));
        assert_eq!(missing, vec![e2.clone()]);
        assert!(extra.is_empty());

        // A registration the manifest does not declare, on top of a complete
        // driver: the extra is the only drift.
        let stale = pairs(&[("axis", "f64"), ("kiddo.stem", "donnelly")]);
        let (missing, extra) = compare_sets(&expected, &[e1, e2, stale.clone()]);
        assert!(missing.is_empty());
        assert_eq!(extra, vec![stale]);
    }

    #[test]
    fn a_subject_with_no_declared_cases_matches_trivially() {
        let (missing, extra) = compare_sets(&[], &[]);
        assert!(missing.is_empty() && extra.is_empty());
    }
}
