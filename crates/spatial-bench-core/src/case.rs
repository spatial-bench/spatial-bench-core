//! Cases: what can be measured, and how.

use crate::tag::{TagMap, TagValue};
use serde::Serialize;
use std::time::Duration;

/// Process start plus tree build, paid per point under `perf`.
const PROCESS_OVERHEAD_MS: u64 = 2_000;

/// The budget is the harness contract's, not a second one: an estimate that
/// could disagree with what a driver is actually told to do is worse than no
/// estimate at all.
pub use crate::harness::Budget;

/// Which measurement backend can drive a case.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Runner {
    /// criterion sweep; many points per process.
    Criterion,
    /// `perf stat`; counters attribute to a process, so ONE point per process.
    Perf,
    /// Not in this pass — emits artefacts rather than points.
    Asm,
    Mca,
}

impl Runner {
    /// The runner a manifest's `runners` list names.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "criterion" => Some(Runner::Criterion),
            "perf" => Some(Runner::Perf),
            "asm" => Some(Runner::Asm),
            "mca" => Some(Runner::Mca),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Runner::Criterion => "criterion",
            Runner::Perf => "perf",
            Runner::Asm => "asm",
            Runner::Mca => "mca",
        }
    }

    /// Runners the engine can actually execute. `Asm`/`Mca` emit artefacts
    /// rather than points (§11) and have no run path; the picker offers only
    /// implemented runners, because a menu item that ends in a refusal is not
    /// a choice .
    pub fn implemented(self) -> bool {
        matches!(self, Runner::Criterion | Runner::Perf)
    }
}

impl std::fmt::Display for Runner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl ParamDomain {
    /// Concrete values, when the domain is finite. `None` for open domains,
    /// which the interactive picker cannot offer as a list.
    pub fn enumerate(&self) -> Option<Vec<TagValue>> {
        match self {
            ParamDomain::Values(v) => Some(v.clone()),
            ParamDomain::IntRange { lo, hi, step } => Some(
                (*lo..=*hi)
                    .step_by(*step as usize)
                    .map(TagValue::Int)
                    .collect(),
            ),
            ParamDomain::AnyInt | ParamDomain::AnyFloat => None,
        }
    }
}

/// The domain of a param axis.
#[derive(Clone, Debug, Serialize)]
pub enum ParamDomain {
    /// An explicit set, e.g. tree sizes `2^16..=2^25`.
    Values(Vec<TagValue>),
    /// Inclusive integer range with a step, for large sweeps.
    IntRange { lo: i64, hi: i64, step: i64 },
    /// Any positive integer; the case supplies a default.
    AnyInt,
    /// Any float; the case supplies a default.
    AnyFloat,
}

impl Param {
    /// Every value in the domain this selector admits; the default when the
    /// selector says nothing about this axis.
    pub fn selected_values(&self, sel: &crate::selector::SelectorSet) -> Vec<TagValue> {
        let constrained = sel
            .selectors
            .iter()
            .filter_map(|s| s.clause_for(&self.key))
            .next()
            .is_some();
        if !constrained {
            return vec![self.default.clone()];
        }
        let enumerated = self.domain.enumerate();
        match enumerated {
            Some(values) => values
                .into_iter()
                .filter(|v| {
                    sel.selectors.iter().any(|s| {
                        s.clause_for(&self.key).is_some_and(|c| {
                            let mut probe = crate::tag::TagMap::new();
                            probe.insert(crate::tag::TagKey::Owned(self.key.clone()), v.clone());
                            c.admits(&probe)
                        })
                    })
                })
                .collect(),
            // An open domain cannot be enumerated, so the selector's own literal
            // values are the sweep — `query_count=1000|10000` means exactly those.
            None => sel
                .selectors
                .iter()
                .filter_map(|s| s.clause_for(&self.key))
                .flat_map(|c| match &c.matcher {
                    crate::selector::Match::OneOf(vs) => vs.clone(),
                    _ => vec![self.default.clone()],
                })
                .collect(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Param {
    pub key: String,
    pub domain: ParamDomain,
    pub default: TagValue,
}

/// One measurable thing.
#[derive(Clone, Debug, Serialize)]
pub struct Case {
    /// Stable, human-readable handle. Used in logs and error messages only —
    /// never parsed, never the identity.
    pub id: String,
    /// The subject that declared it, i.e. which manifest it came from.
    pub subject: String,
    /// How to execute it: [`crate::adapter::Adapter`], typed at manifest load.
    pub adapter: crate::adapter::Adapter,
    /// Engine crate providing this subject's driver. rust-codegen only —
    /// an exec driver is a program beside the manifest, not a crate.
    pub driver_crate: Option<String>,
    /// Two-phase only: macro a generated `main.rs` invokes.
    pub driver_macro: Option<String>,
    /// exec only: the driver's language and entry file.
    pub driver_lang: Option<String>,
    pub driver_entry: Option<String>,
    /// Which [[driver]] block this case belongs to: the driver whose
    /// supported semver range contains the pinned library version.
    pub driver: String,
    /// Fixed identity tags.
    pub tags: TagMap,
    /// Axes this case can be swept over.
    pub params: Vec<Param>,
    pub runners: Vec<String>,
    /// Tag keys that are generic parameters of this subject. Two cases sharing
    /// these values share a generated binary, however their runtime axes differ.
    pub compile_time_keys: Vec<String>,
}

impl Case {
    /// The compile-time coordinate of this case: the values that decide which
    /// monomorphisation it needs. Cases with equal keys share one build.
    pub fn compile_time_key(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = self
            .compile_time_keys
            .iter()
            .filter_map(|k| {
                self.tags
                    .get(k.as_str())
                    .map(|v| (k.clone(), v.to_string()))
            })
            .collect();
        out.sort();
        out
    }
}

impl Case {
    /// The param axes this case sweeps, for the selector's deferral rule.
    pub fn param_keys(&self) -> Vec<&str> {
        self.params.iter().map(|p| p.key.as_str()).collect()
    }

    /// Expand into one fully-resolved tag map per data point.
    ///
    /// A param constrained by the selector is swept over the constrained values;
    /// an unconstrained one contributes only its default. That is what lets one
    /// expression both choose cases and pin sweeps without a second flag.
    pub fn expand(&self, sel: &crate::selector::SelectorSet) -> Vec<TagMap> {
        let mut points = vec![self.tags.clone()];
        for param in &self.params {
            let values = param.selected_values(sel);
            let mut next = Vec::with_capacity(points.len() * values.len().max(1));
            for base in &points {
                for value in &values {
                    let mut tags = base.clone();
                    tags.insert(crate::tag::TagKey::Owned(param.key.clone()), value.clone());
                    next.push(tags);
                }
            }
            points = next;
        }
        points.retain(|tags| sel.admits_point(tags));
        points
    }

    /// Lower bound on the resident bytes a point needs: the raw coordinate and
    /// index storage, ignoring stem/leaf overhead and any transient build
    /// scratch.
    ///
    /// This exists because the real ceiling on `tree_size` is host memory, which
    /// is per-machine and therefore cannot be a constant in a subject manifest.
    /// A manifest declares the range worth sweeping; the engine decides what
    /// fits here, on this box, at run time.
    pub fn footprint_bytes(tags: &TagMap) -> Option<u64> {
        let int = |k: &str| match tags.get(k) {
            Some(TagValue::Int(i)) if *i > 0 => Some(*i as u64),
            _ => None,
        };
        let width = |k: &str, default: u64| match tags.get(k).map(ToString::to_string).as_deref() {
            Some("f32") | Some("u32") => 4,
            Some("f64") | Some("u64") => 8,
            Some("u16") => 2,
            _ => default,
        };
        let points = int("tree_size")?;
        let dims = int("dims")?;
        Some(points * (dims * width("axis", 8) + width("idx", 4)))
    }

    /// Predicted wall-clock time for `points` data points of this case.
    ///
    /// Deliberately crude, and stated as such in the menu: it is warm-up plus
    /// measurement per point, and for `perf` a per-process overhead, because
    /// perf attributes counters to a process and so runs one point per process
    /// rather than sweeping inside one.
    ///
    /// Compile time is not included -- it is amortised across every case
    /// sharing a subject and cannot be attributed to one case.
    pub fn estimate(&self, runner: Runner, points: usize, budget: &Budget) -> Duration {
        let per_point = Duration::from_millis(budget.warm_up_ms + budget.measurement_ms);
        let per_point = match runner {
            Runner::Criterion => per_point,
            // perf attributes counters to a process, so it pays process start
            // and tree build once per point rather than once per sweep.
            Runner::Perf => per_point + Duration::from_millis(PROCESS_OVERHEAD_MS),
            Runner::Asm | Runner::Mca => Duration::ZERO,
        };
        per_point * points as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// the picker offers runners by name, so the strings and the
    /// implemented set are contract, not presentation.
    #[test]
    fn runner_strings_parse_and_implemented_is_criterion_only() {
        assert_eq!(Runner::parse("criterion"), Some(Runner::Criterion));
        assert_eq!(Runner::parse("perf"), Some(Runner::Perf));
        assert!(Runner::parse("nope").is_none());
        assert!(Runner::Criterion.implemented());
        assert!(Runner::Perf.implemented(), "§11's perf runner");
        assert_eq!(Runner::Perf.to_string(), "perf");
        assert!(!Runner::Asm.implemented());
    }
}
