//! The catalog: every case, validated, queryable.

use crate::case::Case;
use crate::selector::SelectorSet;
use crate::tag::TagMap;
use std::path::PathBuf;

pub struct Catalog {
    cases: Vec<Case>,
    vocab: crate::vocab::Vocabulary,
    subjects: std::collections::BTreeMap<String, SubjectFacts>,
}

/// Per-subject facts a build needs, kept from the manifest after lowering.
#[derive(Clone, Debug, Default)]
pub struct SubjectFacts {
    pub pinned_ref: String,
    /// `cargo-git` (a Rust crate cargo can fetch itself) or `git` (a repo the
    /// engine's build recipe handles, e.g. a C++ shim).
    pub source_kind: String,
    pub repo: Option<String>,
    /// Published version of the subject's driver crate, for builds with no
    /// engine source tree to depend on by path.
    pub driver_version: Option<String>,
    /// The exact revision the manifest pins its ref to , when declared.
    /// The build refuses a ref that resolves to anything else.
    pub expected_sha: Option<String>,
    pub source_package: Option<String>,
    /// Driver assets relative to the manifest's own directory,
    /// when the manifest declares them.
    pub driver_path: Option<String>,
    /// The exec build recipe — lang, entry, flags. rust-codegen
    /// subjects carry none.
    pub build: Option<crate::manifest::Build>,
    /// The directory this subject's manifest was loaded from — the anchor for
    /// manifest-relative driver assets ( the catalog lives in the
    /// bencher checkout, and every subject dir is self-contained).
    pub manifest_dir: PathBuf,
    pub features: Vec<String>,
    pub min_rustc: Option<String>,
    pub rustflags: Option<String>,
    /// One per [[driver]] block, each scoped to a semver range of the
    /// library under test. The pin's version selects which one runs.
    pub drivers: Vec<DriverFacts>,
}

/// The build-relevant slice of one [[driver]] block.
#[derive(Clone, Debug)]
pub struct DriverFacts {
    pub name: String,
    pub min_supported_semver: Option<String>,
    pub max_supported_semver: Option<String>,
    pub version: Option<String>,
    pub path: Option<String>,
    pub features: Vec<String>,
    pub rustflags: Option<String>,
    pub macro_name: Option<String>,
    pub crate_name: Option<String>,
}

impl Catalog {
    pub fn new(
        cases: Vec<Case>,
        vocab: crate::vocab::Vocabulary,
        subjects: std::collections::BTreeMap<String, SubjectFacts>,
    ) -> Self {
        Self {
            cases,
            vocab,
            subjects,
        }
    }

    /// The driver this subject's pin selects: the one whose supported
    /// semver range contains the version the pin names. Ranges are
    /// inclusive at the bottom and exclusive at the top; a missing bound is
    /// open. A pin outside every range, or matching several, is refused.
    pub fn selected_driver(&self, subject: &str) -> Result<&DriverFacts, String> {
        let facts = self
            .subjects
            .get(subject)
            .ok_or_else(|| format!("{subject}: no such subject"))?;
        let version = semver::Version::parse(
            facts
                .pinned_ref
                // A few upstreams publish semver tags as `v.0.4.1`; accept
                // that cosmetic leading dot while retaining the exact tag as
                // the source pin Cargo resolves.
                .trim_start_matches(['v', 'V', '.'])
                .split(['-', '+'])
                .next()
                .unwrap_or(""),
        )
        .map_err(|e| {
            format!(
                "{subject}: the pinned ref `{}` is not a semver version, so no \
                 driver range can be selected: {e}",
                facts.pinned_ref
            )
        })?;
        let mut matches = facts.drivers.iter().filter(|d| {
            let above_min = d
                .min_supported_semver
                .as_deref()
                .and_then(|s| semver::Version::parse(s.trim_start_matches('v')).ok())
                .map(|min| version >= min)
                .unwrap_or(true);
            let below_max = d
                .max_supported_semver
                .as_deref()
                .and_then(|s| semver::Version::parse(s.trim_start_matches('v')).ok())
                .map(|max| version < max)
                .unwrap_or(true);
            above_min && below_max
        });
        let Some(selected) = matches.next() else {
            return Err(format!(
                "{subject}: the pinned version {version} is outside every \
                 driver's supported range"
            ));
        };
        if matches.next().is_some() {
            return Err(format!(
                "{subject}: the pinned version {version} is inside more than \
                 one driver's supported range — tighten the ranges"
            ));
        }
        Ok(selected)
    }

    /// The revision a subject is pinned to.
    pub fn pinned_ref(&self, subject: &str) -> Option<String> {
        self.subjects.get(subject).map(|f| f.pinned_ref.clone())
    }

    /// A subject's declared source: (kind, repo, pin). What a pinned build
    /// builds from when `--subject-path` does not override it.
    pub fn source(&self, subject: &str) -> Option<(String, Option<String>, String)> {
        self.subjects
            .get(subject)
            .map(|f| (f.source_kind.clone(), f.repo.clone(), f.pinned_ref.clone()))
    }

    /// The driver facts of the driver the pin selects, when its range holds.
    pub fn selected_driver_facts(&self, subject: &str) -> Option<&DriverFacts> {
        self.selected_driver(subject).ok()
    }

    /// The published driver-crate version a source-less build should depend
    /// on — the pin-selected driver's. Absent means the driver crate has
    /// never been published — honest refusal beats a guessed version.
    pub fn driver_version(&self, subject: &str) -> Option<String> {
        self.selected_driver_facts(subject)
            .and_then(|d| d.version.clone())
    }

    /// The manifest-relative driver path of the pin-selected driver, resolved
    /// against the manifest's own directory — the bencher-repo layout, where
    /// driver assets live beside the manifest. `None` when the manifest
    /// declares none.
    pub fn manifest_driver_path(&self, subject: &str) -> Option<std::path::PathBuf> {
        let facts = self.subjects.get(subject)?;
        let rel = self.selected_driver_facts(subject)?.path.as_ref()?;
        let dir = facts.manifest_dir.join(rel);
        dir.is_dir().then_some(dir)
    }

    /// The exact revision the manifest pins its ref to, when declared .
    pub fn expected_sha(&self, subject: &str) -> Option<String> {
        self.subjects
            .get(subject)
            .and_then(|f| f.expected_sha.clone())
    }

    /// The directory this subject's manifest was loaded from — the anchor for
    /// manifest-relative driver assets.
    pub fn manifest_dir(&self, subject: &str) -> Option<std::path::PathBuf> {
        self.subjects.get(subject).map(|f| f.manifest_dir.clone())
    }

    /// The exec build recipe, when the subject declares one.
    pub fn build(&self, subject: &str) -> Option<crate::manifest::Build> {
        self.subjects.get(subject).and_then(|f| f.build.clone())
    }

    /// The subject's full source pin, reconstructed for the exec builders.
    pub fn source_full(&self, subject: &str) -> Result<crate::manifest::Source, String> {
        let facts = self
            .subjects
            .get(subject)
            .ok_or_else(|| format!("no subject {subject} in the catalog"))?;
        Ok(crate::manifest::Source {
            kind: facts.source_kind.clone(),
            repo: facts.repo.clone(),
            package: facts.source_package.clone(),
            pinned_ref: facts.pinned_ref.clone(),
            sha: facts.expected_sha.clone(),
        })
    }

    /// Cargo features a subject is built with.
    pub fn features(&self, subject: &str) -> Vec<String> {
        self.selected_driver_facts(subject)
            .map(|d| d.features.clone())
            .unwrap_or_default()
    }

    /// RUSTFLAGS a subject's driver is built with.
    pub fn rustflags(&self, subject: &str) -> Option<String> {
        self.selected_driver_facts(subject)
            .and_then(|d| d.rustflags.clone())
    }

    /// Any RUSTFLAGS in play, for the run record. Distinct values across
    /// subjects are joined so the record cannot imply a single setting applied.
    pub fn rustflags_any(&self) -> Option<String> {
        let mut all: Vec<String> = self
            .subjects
            .values()
            .filter_map(|f| f.rustflags.clone())
            .collect();
        all.sort();
        all.dedup();
        (!all.is_empty()).then(|| all.join("; "))
    }

    /// Lowest rustc a subject builds with, if it declares one.
    pub fn min_rustc(&self, subject: &str) -> Option<String> {
        self.subjects.get(subject).and_then(|f| f.min_rustc.clone())
    }

    /// An empty catalog. Cases only ever arrive by loading vendored subject
    /// manifests — see [`crate::catalog_load::load_dir`].
    pub fn empty() -> Self {
        Self {
            cases: Vec::new(),
            vocab: crate::vocab::Vocabulary::default(),
            subjects: Default::default(),
        }
    }

    /// The vocabulary in force: engine core plus every loaded subject's
    /// namespaced extensions.
    pub fn vocabulary(&self) -> &crate::vocab::Vocabulary {
        &self.vocab
    }

    pub fn cases(&self) -> &[Case] {
        &self.cases
    }

    /// Cases declared by one subject.
    pub fn for_subject<'a>(&'a self, subject: &str) -> impl Iterator<Item = &'a Case> {
        let subject = subject.to_owned();
        self.cases.iter().filter(move |c| c.subject == subject)
    }

    /// Cases surviving a selector's identity clauses. Param clauses are deferred
    /// to expansion, so a case is not filtered out by a sweep it has not
    /// resolved yet.
    /// Returns a Vec rather than an iterator so the selector need not outlive
    /// the catalog; catalogs are small and this is called once per command.
    pub fn matching<'a>(&'a self, sel: &SelectorSet) -> Vec<&'a Case> {
        self.cases
            .iter()
            .filter(|c| sel.admits_case(&c.tags, &c.param_keys()))
            // A case is only runnable under the driver the pin selects: a
            // driver scoped to another semver range is not this pin's
            // driver, and its cases drop out of every count, plan and run.
            // A pin that selects nothing (not semver, outside every range)
            // contributes no cases; the run path surfaces the reason when it
            // reaches that subject.
            .filter(|c| {
                self.selected_driver(&c.subject)
                    .map(|d| d.name == c.driver)
                    .unwrap_or(false)
            })
            .collect()
    }

    /// Fully-resolved data points for a selection — what the menu counts and the
    /// runner executes.
    pub fn points<'a>(&'a self, sel: &SelectorSet) -> Vec<(&'a Case, TagMap)> {
        self.matching(sel)
            .into_iter()
            .flat_map(|case| case.expand(sel).into_iter().map(move |tags| (case, tags)))
            .collect()
    }

    /// Predicted wall-clock time for a selection.
    pub fn estimate(
        &self,
        sel: &SelectorSet,
        runner: crate::case::Runner,
        budget: &crate::case::Budget,
    ) -> std::time::Duration {
        let points = self.points(sel);
        self.matching(sel)
            .iter()
            .map(|case| {
                let n = points.iter().filter(|(c, _)| c.id == case.id).count();
                case.estimate(runner, n, budget)
            })
            .sum()
    }

    /// Tag keys whose value differs across the matching cases — the columns
    /// worth showing, so a listing does not repeat what every row shares.
    pub fn varying_keys(&self, sel: &SelectorSet) -> Vec<String> {
        let cases = self.matching(sel);
        let mut keys: Vec<String> = Vec::new();
        for case in &cases {
            for key in case.tags.keys() {
                if !keys.iter().any(|k| k == key.as_ref()) {
                    keys.push(key.to_string());
                }
            }
        }
        keys.retain(|key| {
            let mut seen: Option<&crate::tag::TagValue> = None;
            cases.iter().any(|c| {
                let here = c.tags.get(key.as_str());
                match (seen, here) {
                    (None, v) => {
                        seen = v;
                        false
                    }
                    (Some(a), Some(b)) => a != b,
                    _ => true,
                }
            })
        });
        keys.sort();
        keys
    }

    /// Selector values that reached no point at all.
    ///
    /// Asking for `tree_size=2^26` when nothing offers it should say so rather
    /// than quietly returning a smaller matrix — a comparison silently missing
    /// what you asked for is the failure mode this whole design exists to stop.
    /// A value that reaches *some* subject but not others is not reported: that
    /// is legitimate, since subjects genuinely differ (Pkd-tree asserts on f32
    /// above 2^21, for instance).
    pub fn unsatisfied(&self, sel: &SelectorSet) -> Vec<String> {
        let points = self.points(sel);
        let mut missing = Vec::new();
        for selector in &sel.selectors {
            for clause in &selector.clauses {
                // A negation reaching nothing is not a broken request — there
                // is nothing odd about excluding something that does not
                // exist. Every other kind of clause is a request that can go
                // unmet, so every kind is checked.
                if clause.negated {
                    continue;
                }
                let pow2 = crate::vocab::lookup(clause.key.as_ref()).is_some_and(|d| d.pow2);
                match &clause.matcher {
                    crate::selector::Match::OneOf(wanted) => {
                        for value in wanted {
                            let reached = points
                                .iter()
                                .any(|(_, tags)| tags.get(&clause.key) == Some(value));
                            if !reached {
                                missing.push(format!("{}={}", clause.key, value.to_expr(pow2)));
                            }
                        }
                    }
                    crate::selector::Match::Range { lo, hi } => {
                        let reached = points.iter().any(|(_, tags)| {
                            tags.get(&clause.key).is_some_and(|v| v >= lo && v <= hi)
                        });
                        if !reached {
                            missing.push(format!(
                                "{}={}..{}",
                                clause.key,
                                lo.to_expr(pow2),
                                hi.to_expr(pow2)
                            ));
                        }
                    }
                    crate::selector::Match::Any => {
                        // `k=*` asks that the key exist at all; if no point
                        // carries it, that is an unmet request like any other.
                        let reached = points
                            .iter()
                            .any(|(_, tags)| tags.get(&clause.key).is_some());
                        if !reached {
                            missing.push(format!("{}=*", clause.key));
                        }
                    }
                }
            }
        }
        missing.sort();
        missing.dedup();
        missing
    }

    /// Points whose estimated footprint exceeds `budget_bytes`.
    ///
    /// The engine warns rather than refuses: the estimate is a lower bound, and
    /// the operator may know better than it does.
    pub fn over_budget(&self, sel: &SelectorSet, budget_bytes: u64) -> Vec<TagMap> {
        self.points(sel)
            .into_iter()
            .filter_map(|(_, tags)| {
                Case::footprint_bytes(&tags)
                    .filter(|b| *b > budget_bytes)
                    .map(|_| tags)
            })
            .collect()
    }

    /// Values a key can be given, among cases still reachable under a partial
    /// selection. Drives the interactive picker, so an empty set cannot be
    /// constructed by accident.
    ///
    /// For a **param** axis this is the declared domain, not the value a point
    /// currently resolves to: an unconstrained param contributes only its
    /// default, so reporting resolved values would offer a single tree size and
    /// make the rest unreachable from the menu.
    pub fn values_for(&self, key: &str, partial: &SelectorSet) -> Vec<crate::tag::TagValue> {
        let matching = self.matching(partial);
        let mut seen: std::collections::BTreeSet<crate::tag::TagValue> = Default::default();

        let mut is_param = false;
        for case in &matching {
            if let Some(param) = case.params.iter().find(|p| p.key == key) {
                is_param = true;
                match param.domain.enumerate() {
                    Some(values) => seen.extend(values),
                    // Open domain: nothing to list, so offer the default as the
                    // one concrete value and let the selector take any other.
                    None => {
                        seen.insert(param.default.clone());
                    }
                }
            }
        }
        if is_param {
            return seen.into_iter().collect();
        }

        for (_, tags) in self.points(partial) {
            if let Some(value) = tags.get(key) {
                seen.insert(value.clone());
            }
        }
        seen.into_iter().collect()
    }

    /// Distinct compile-time coordinates in a selection: one generated binary
    /// per entry, however many runtime points each carries.
    ///
    /// This is the number that decides how long a run spends compiling, so the
    /// menu reports it separately from the point count.
    pub fn build_units(&self, sel: &SelectorSet) -> Vec<(String, Vec<(String, String)>)> {
        let mut units: Vec<(String, Vec<(String, String)>)> = self
            .matching(sel)
            .iter()
            .map(|c| (c.subject.clone(), c.compile_time_key()))
            .collect();
        units.sort();
        units.dedup();
        units
    }

    /// Measurement backends every matching case supports. Intersection, not
    /// union: offering a runner that only some cases can use would silently
    /// drop the rest from the run.
    pub fn runners_for(&self, sel: &SelectorSet) -> Vec<String> {
        let matching = self.matching(sel);
        let mut iter = matching.iter();
        let Some(first) = iter.next() else {
            return Vec::new();
        };
        let mut common: std::collections::BTreeSet<String> =
            first.runners.iter().cloned().collect();
        for case in iter {
            let here: std::collections::BTreeSet<String> = case.runners.iter().cloned().collect();
            common = common.intersection(&here).cloned().collect();
        }
        common.into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tags;

    fn catalog() -> Catalog {
        let dir = crate::test_support::subjects_dir();
        crate::catalog_load::load_dir(&dir).unwrap()
    }

    /// Compile-time keys come from the manifest's `compile_time` markings, and
    /// only those drive code generation.
    #[test]
    fn compile_time_keys_are_the_generic_parameters() {
        let catalog = catalog();
        let case = catalog.for_subject("kiddo").next().unwrap();
        let keys = &case.compile_time_keys;
        assert!(keys.contains(&"kiddo.stem".to_string()));
        assert!(keys.contains(&"kiddo.leaf".to_string()));
        assert!(keys.contains(&"kiddo.bucket".to_string()));
        assert!(
            !keys.contains(&"kiddo.storage".to_string()),
            "runtime, not generic"
        );

        // nanoflann resolves its templates in the shim, so it generates nothing.
        let nano = catalog.for_subject("nanoflann").next().unwrap();
        assert!(nano.compile_time_keys.is_empty());
    }

    /// Sweeping a runtime axis must not add builds. This is the property the
    /// whole two-phase design exists for: compile time scales with the
    /// selection's generic combinations, not with its point count.
    #[test]
    fn runtime_sweeps_do_not_multiply_builds() {
        let catalog = catalog();
        let one =
            SelectorSet::parse_all(["impl=kiddo,k=1,axis=f64,kiddo.stem=eytzinger,tree_size=2^20"])
                .unwrap();
        let many = SelectorSet::parse_all([
            "impl=kiddo,k=1,axis=f64,kiddo.stem=eytzinger,tree_size=2^16..2^25",
        ])
        .unwrap();

        assert_eq!(catalog.points(&one).len(), 4);
        assert_eq!(catalog.points(&many).len(), 40);
        assert_eq!(
            catalog.build_units(&one).len(),
            catalog.build_units(&many).len(),
            "ten tree sizes is still one build"
        );
    }

    /// …whereas a compile-time axis does add builds, one per combination.
    #[test]
    fn compile_time_axes_add_one_build_each() {
        let catalog = catalog();
        // Every generic combination is a build: nine strategies over two
        // scalars. k is a runtime axis, so its four values add none.
        let all_kiddo = SelectorSet::parse_all(["impl=kiddo"]).unwrap();
        assert_eq!(catalog.build_units(&all_kiddo).len(), 9 * 2 + 2);
        assert_eq!(catalog.points(&all_kiddo).len(), 109);

        let one = SelectorSet::parse_all(["impl=kiddo,axis=f64,kiddo.stem=eytzinger"]).unwrap();
        assert_eq!(catalog.build_units(&one).len(), 2);
        // tuned flatvec + default vec_of_arenas are two monomorphisations of
        // the same stem/axis; 12 exact_nn + 4 within + 4 bnw + 2 nnw points
        // match the selection.
        assert_eq!(
            catalog.points(&one).len(),
            22,
            "the tuned and default cases match the stem/axis selection"
        );

        // nanoflann and pykdtree resolve in their exec builders, so each is
        // one build.
        let both = catalog.build_units(&SelectorSet::default());
        assert!(
            both.len() >= 9 * 2 + 2 + 1 + 1,
            "the default catalog includes every kiddo combination and the exec subjects"
        );
    }

    /// Intersection, not union: offering a runner only some cases support would
    /// silently drop the rest of the selection.
    #[test]
    fn runners_are_the_intersection_across_matching_cases() {
        let catalog = catalog();
        let all = catalog.runners_for(&SelectorSet::default());
        assert_eq!(all, vec!["criterion".to_string(), "perf".to_string()]);
        let none = catalog.runners_for(&SelectorSet::parse_all(["impl=nope"]).unwrap());
        assert!(none.is_empty());
    }

    /// Regression: `run` previously estimated with an empty selection, so a
    /// narrow request reported the whole catalog's runtime.
    #[test]
    fn estimate_scales_with_its_own_selection() {
        use crate::case::{Budget, Runner};
        let catalog = catalog();
        let budget = Budget::default();
        let all = catalog.estimate(&SelectorSet::default(), Runner::Criterion, &budget);
        let narrow = catalog.estimate(
            &SelectorSet::parse_all(["impl=kiddo,k=1"]).unwrap(),
            Runner::Criterion,
            &budget,
        );
        assert!(
            narrow < all,
            "narrow {narrow:?} should be under all {all:?}"
        );
        // Every selected case takes one warm-up + measurement budget. Keep the
        // assertion data-driven: the reviewed bencher catalog deliberately
        // grows independently of this engine crate.
        let budget_per_point = std::time::Duration::from_secs(8);
        assert_eq!(
            all,
            budget_per_point * catalog.points(&SelectorSet::default()).len() as u32
        );
        assert_eq!(
            narrow,
            budget_per_point
                * catalog
                    .points(&SelectorSet::parse_all(["impl=kiddo,k=1"]).unwrap())
                    .len() as u32
        );
    }

    /// A listing should only column what actually differs.
    #[test]
    fn varying_keys_excludes_what_every_case_shares() {
        let catalog = catalog();
        let keys = catalog.varying_keys(&SelectorSet::parse_all(["impl=kiddo"]).unwrap());
        assert!(keys.contains(&"k".to_string()));
        assert!(keys.contains(&"axis".to_string()));
        assert!(!keys.contains(&"impl".to_string()), "shared by every row");
        // query is now a varying key (the standard corpus includes multiple query kinds)
        // assert!(!keys.contains(&"query".to_string()), "shared by every row");
    }

    /// A requested value nothing can serve is reported, not silently dropped.
    #[test]
    fn unreachable_selector_values_are_reported() {
        let catalog = catalog();
        let sel = SelectorSet::parse_all(["impl=kiddo,k=1|999"]).unwrap();
        assert_eq!(catalog.unsatisfied(&sel), vec!["k=999".to_string()]);
    }

    /// a range that reaches nothing is reported like an unmet value —
    /// the whole point of the function, not just its OneOf case. An empty
    /// expansion reports every unsatisfied clause, because every clause
    /// reached no point; the range is in the list with the reason visible.
    #[test]
    fn unreachable_ranges_are_reported() {
        let catalog = catalog();
        let sel = SelectorSet::parse_all(["impl=kiddo,tree_size=2^40..2^41"]).unwrap();
        assert_eq!(
            catalog.unsatisfied(&sel),
            vec!["impl=kiddo".to_string(), "tree_size=2^40..2^41".to_string()]
        );
    }

    /// `key=*` asks that the key exist at all. A key no case carries
    /// empties the selection, and the existence requirement is in the report
    /// with everything else it took down.
    #[test]
    fn an_existence_requirement_reaching_nothing_is_reported() {
        let catalog = catalog();
        // kiddo declares `hugepages` vocabulary but no case sets it, so the
        // key exists in the vocabulary and in no point.
        let sel = SelectorSet::parse_all(["impl=kiddo,kiddo.hugepages=*"]).unwrap();
        assert_eq!(
            catalog.unsatisfied(&sel),
            vec!["impl=kiddo".to_string(), "kiddo.hugepages=*".to_string()]
        );
    }

    /// …but a value every case serves is not reported.
    #[test]
    fn satisfied_values_are_not_reported() {
        let catalog = catalog();
        let sel = SelectorSet::parse_all(["impl=kiddo,k=1|5|20|50"]).unwrap();
        assert!(catalog.unsatisfied(&sel).is_empty());
    }

    /// 2^26 f64 at K=3 is roughly 1.7 GB of raw storage — comfortably real on a
    /// 64 GB box, which is exactly why the ceiling cannot be a manifest constant.
    #[test]
    fn footprint_tracks_scalar_and_dimensionality() {
        let f64_3d = tags! { tree_size: 1usize << 26, dims: 3, axis: "f64", idx: "u32" };
        let f32_3d = tags! { tree_size: 1usize << 26, dims: 3, axis: "f32", idx: "u32" };
        let big = Case::footprint_bytes(&f64_3d).unwrap();
        let small = Case::footprint_bytes(&f32_3d).unwrap();
        assert_eq!(big, (1u64 << 26) * (3 * 8 + 4));
        assert!(big > small, "f64 must cost more per point than f32");
        assert!((1.6e9..1.9e9).contains(&(big as f64)), "got {big} bytes");
    }

    #[test]
    fn over_budget_flags_only_what_exceeds_it() {
        let catalog = catalog();
        let sel = SelectorSet::parse_all([
            "impl=kiddo,k=1,axis=f64,kiddo.stem=eytzinger,tree_size=2^16|2^29",
        ])
        .unwrap();
        let over = catalog.over_budget(&sel, 1 << 30); // 1 GiB
        assert_eq!(over.len(), 4, "only 2^29 should exceed a 1 GiB budget");
        assert_eq!(
            over[0].get("tree_size"),
            Some(&crate::tag::TagValue::Int(1 << 29))
        );
    }
}
