//! Cases: what can be measured, and how.

use crate::tag::{TagMap, TagValue};
use serde::Serialize;
use std::time::Duration;

/// Per-point time budget, mirroring criterion's own knobs so an estimate tracks
/// whatever the run will actually be told to do.
#[derive(Clone, Copy, Debug)]
pub struct Budget {
    pub warm_up: Duration,
    pub measurement: Duration,
    /// Process start plus tree build, paid once per point under `perf`.
    pub process_overhead: Duration,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            warm_up: Duration::from_secs(3),
            measurement: Duration::from_secs(5),
            process_overhead: Duration::from_secs(2),
        }
    }
}

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
    /// How to execute it: `exec` or `rust-codegen`.
    pub adapter: String,
    /// Engine crate providing this subject's driver.
    pub driver_crate: String,
    /// Two-phase only: macro a generated `main.rs` invokes.
    pub driver_macro: Option<String>,
    /// Single-phase only: templated against the resolved tags.
    pub command: Vec<String>,
    pub output: Option<String>,
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
        let per_point = match runner {
            Runner::Criterion => budget.warm_up + budget.measurement,
            Runner::Perf => budget.warm_up + budget.measurement + budget.process_overhead,
            Runner::Asm | Runner::Mca => Duration::ZERO,
        };
        per_point * points as u32
    }
}
