//! The catalog: every case, validated, queryable.

use crate::case::Case;
use crate::selector::SelectorSet;
use crate::tag::TagMap;

pub struct Catalog {
    cases: Vec<Case>,
    vocab: crate::vocab::Vocabulary,
}

impl Catalog {
    pub fn new(cases: Vec<Case>, vocab: crate::vocab::Vocabulary) -> Self {
        Self { cases, vocab }
    }

    /// An empty catalog. Cases only ever arrive by loading vendored subject
    /// manifests — see [`crate::catalog_load::load_dir`].
    pub fn empty() -> Self {
        Self {
            cases: Vec::new(),
            vocab: crate::vocab::Vocabulary::default(),
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
                if clause.negated {
                    continue;
                }
                let crate::selector::Match::OneOf(wanted) = &clause.matcher else {
                    continue;
                };
                for value in wanted {
                    let reached = points
                        .iter()
                        .any(|(_, tags)| tags.get(&clause.key) == Some(value));
                    if !reached {
                        let pow2 =
                            crate::vocab::lookup(clause.key.as_ref()).is_some_and(|d| d.pow2);
                        missing.push(format!("{}={}", clause.key, value.to_expr(pow2)));
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
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../subjects");
        crate::catalog_load::load_dir(&dir).unwrap()
    }

    /// Compile-time keys come from the manifest's `compile_time` markings, and
    /// only those drive code generation.
    #[test]
    fn compile_time_keys_are_the_generic_parameters() {
        let catalog = catalog();
        let case = catalog.for_subject("kiddo_v6").next().unwrap();
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
        let one = SelectorSet::parse_all(["impl=kiddo_v6,k=1,axis=f64,tree_size=2^20"]).unwrap();
        let many =
            SelectorSet::parse_all(["impl=kiddo_v6,k=1,axis=f64,tree_size=2^16..2^25"]).unwrap();

        assert_eq!(catalog.points(&one).len(), 1);
        assert_eq!(catalog.points(&many).len(), 10);
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
        let all_kiddo = SelectorSet::parse_all(["impl=kiddo_v6"]).unwrap();
        // Only one stem/leaf/bucket combination is declared so far, so every
        // kiddo case shares a single build unit.
        assert_eq!(catalog.build_units(&all_kiddo).len(), 1);
        assert_eq!(catalog.points(&all_kiddo).len(), 8);

        let both = catalog.build_units(&SelectorSet::default());
        assert_eq!(both.len(), 2, "kiddo and nanoflann build separately");
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
            &SelectorSet::parse_all(["impl=kiddo_v6,k=1"]).unwrap(),
            Runner::Criterion,
            &budget,
        );
        assert!(
            narrow < all,
            "narrow {narrow:?} should be under all {all:?}"
        );
        assert_eq!(narrow * 8, all, "16 points vs 2");
    }

    /// A listing should only column what actually differs.
    #[test]
    fn varying_keys_excludes_what_every_case_shares() {
        let catalog = catalog();
        let keys = catalog.varying_keys(&SelectorSet::parse_all(["impl=kiddo_v6"]).unwrap());
        assert!(keys.contains(&"k".to_string()));
        assert!(keys.contains(&"axis".to_string()));
        assert!(!keys.contains(&"impl".to_string()), "shared by every row");
        assert!(!keys.contains(&"query".to_string()), "shared by every row");
    }

    /// A requested value nothing can serve is reported, not silently dropped.
    #[test]
    fn unreachable_selector_values_are_reported() {
        let catalog = catalog();
        let sel = SelectorSet::parse_all(["impl=kiddo_v6,k=1|999"]).unwrap();
        assert_eq!(catalog.unsatisfied(&sel), vec!["k=999".to_string()]);
    }

    /// …but a value every case serves is not reported.
    #[test]
    fn satisfied_values_are_not_reported() {
        let catalog = catalog();
        let sel = SelectorSet::parse_all(["impl=kiddo_v6,k=1|5|20|50"]).unwrap();
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
        let sel =
            SelectorSet::parse_all(["impl=kiddo_v6,k=1,axis=f64,tree_size=2^16|2^29"]).unwrap();
        let over = catalog.over_budget(&sel, 1 << 30); // 1 GiB
        assert_eq!(over.len(), 1, "only 2^29 should exceed a 1 GiB budget");
        assert_eq!(
            over[0].get("tree_size"),
            Some(&crate::tag::TagValue::Int(1 << 29))
        );
    }
}
