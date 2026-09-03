//! The interactive picker's logic, kept separate from its terminal rendering so
//! it can be tested without a tty.
//!
//! The picker walks the tag keys present in the catalog and offers, for each,
//! only the values that keep the selection non-empty — so an empty run cannot be
//! constructed by accident.

use crate::case::{Budget, Runner};
use crate::catalog::Catalog;
use crate::selector::{Clause, Match, Selector, SelectorSet};
use crate::tag::{TagKey, TagValue};
use std::time::Duration;

/// One offerable key and the values still reachable for it.
#[derive(Clone, Debug, PartialEq)]
pub struct Facet {
    pub key: String,
    pub values: Vec<TagValue>,
    /// Values the current selection has pinned. Empty means unconstrained.
    pub chosen: Vec<TagValue>,
}

/// What the picker shows for the current selection.
#[derive(Clone, Debug)]
pub struct Preview {
    pub cases: usize,
    pub points: usize,
    pub runners: Vec<String>,
    pub estimate: Duration,
    /// Requested values that reach nothing. Surfaced before the run rather than
    /// silently yielding a smaller matrix than asked for.
    pub unsatisfied: Vec<String>,
}

/// A selection being built up.
#[derive(Clone, Debug, Default)]
pub struct Picker {
    chosen: Vec<Clause>,
}

impl Picker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn selection(&self) -> SelectorSet {
        if self.chosen.is_empty() {
            return SelectorSet::default();
        }
        SelectorSet {
            selectors: vec![Selector {
                clauses: self.chosen.clone(),
            }],
        }
    }

    /// Pin a key to a set of values. An empty set clears the key.
    pub fn choose(&mut self, key: &str, values: Vec<TagValue>) {
        self.chosen.retain(|c| c.key.as_ref() != key);
        if !values.is_empty() {
            self.chosen.push(Clause {
                key: TagKey::Owned(key.to_owned()),
                matcher: Match::OneOf(values),
                negated: false,
            });
        }
        // Keep clause order stable so the printed command does not churn as
        // choices are revised.
        self.chosen.sort_by(|a, b| a.key.cmp(&b.key));
    }

    /// Keys worth offering, with the values still reachable for each.
    ///
    /// A key with only one reachable value is still listed: it tells the reader
    /// what the selection has already narrowed to.
    pub fn facets(&self, catalog: &Catalog) -> Vec<Facet> {
        let selection = self.selection();
        let points = catalog.points(&selection);

        let mut keys: Vec<String> = Vec::new();
        for (_, tags) in &points {
            for key in tags.keys() {
                if !keys.iter().any(|k| k == key.as_ref()) {
                    keys.push(key.to_string());
                }
            }
        }
        keys.sort();

        keys.into_iter()
            .map(|key| {
                // Values reachable if this key alone were unconstrained, so a
                // choice can always be widened again.
                let mut without = self.clone();
                without.choose(&key, Vec::new());
                let values = catalog.values_for(&key, &without.selection());
                let chosen = self
                    .chosen
                    .iter()
                    .find(|c| c.key.as_ref() == key)
                    .and_then(|c| match &c.matcher {
                        Match::OneOf(v) => Some(v.clone()),
                        _ => None,
                    })
                    .unwrap_or_default();
                Facet {
                    key,
                    values,
                    chosen,
                }
            })
            .collect()
    }

    pub fn preview(&self, catalog: &Catalog, runner: Runner, budget: &Budget) -> Preview {
        let selection = self.selection();
        let cases = catalog.matching(&selection);
        let points = catalog.points(&selection);
        let estimate = catalog.estimate(&selection, runner, budget);
        Preview {
            cases: cases.len(),
            points: points.len(),
            runners: catalog.runners_for(&selection),
            estimate,
            unsatisfied: catalog.unsatisfied(&selection),
        }
    }

    /// The selection with clauses that narrow nothing removed.
    ///
    /// The picker pins a key whenever you touch it, including keys with only one
    /// reachable value, so a fully-explored selection accumulates a clause per
    /// facet. Echoing all of them produces a repro line too long to read or
    /// paste. A clause whose removal leaves the point set unchanged is dropped:
    /// the shortened expression selects exactly the same points.
    pub fn minimal(&self, catalog: &Catalog) -> SelectorSet {
        let full = self.selection();
        let target = catalog.points(&full).len();
        let mut kept = self.chosen.clone();

        // Reverse order so indices stay valid while removing.
        for index in (0..kept.len()).rev() {
            let mut without = kept.clone();
            without.remove(index);
            let candidate = if without.is_empty() {
                SelectorSet::default()
            } else {
                SelectorSet {
                    selectors: vec![Selector {
                        clauses: without.clone(),
                    }],
                }
            };
            if catalog.points(&candidate).len() == target {
                kept = without;
            }
        }

        if kept.is_empty() {
            SelectorSet::default()
        } else {
            SelectorSet {
                selectors: vec![Selector { clauses: kept }],
            }
        }
    }

    /// The non-interactive equivalent of the current selection, minimised.
    ///
    /// This is the line printed for reuse in CI, so it must parse back to a
    /// selection producing the same points.
    pub fn command_for(&self, runner: Runner, catalog: &Catalog) -> String {
        let minimal = self.minimal(catalog);
        Self::render(runner, &minimal)
    }

    fn render(runner: Runner, selection: &SelectorSet) -> String {
        let runner = match runner {
            Runner::Criterion => "criterion",
            Runner::Perf => "perf",
            Runner::Asm => "asm",
            Runner::Mca => "mca",
        };
        if selection.selectors.is_empty() {
            return format!("spatial-bench run --runner {runner}");
        }
        format!(
            "spatial-bench run --runner {runner} --select '{}'",
            selection.to_exprs().join("' --select '")
        )
    }

    /// Unminimised form, for tests and callers with no catalog to hand.
    pub fn command(&self, runner: Runner) -> String {
        let runner = match runner {
            Runner::Criterion => "criterion",
            Runner::Perf => "perf",
            Runner::Asm => "asm",
            Runner::Mca => "mca",
        };
        if self.chosen.is_empty() {
            return format!("spatial-bench run --runner {runner}");
        }
        format!(
            "spatial-bench run --runner {runner} --select '{}'",
            self.selection().to_exprs().join("' --select '")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> Catalog {
        let dir = crate::test_support::subjects_dir();
        crate::catalog_load::load_dir(&dir).unwrap()
    }

    fn word(s: &str) -> TagValue {
        TagValue::Word(s.to_owned())
    }

    #[test]
    fn offers_every_key_present_in_the_catalog() {
        let facets = Picker::new().facets(&catalog());
        let keys: Vec<&str> = facets.iter().map(|f| f.key.as_str()).collect();
        for expected in ["impl", "query", "k", "axis", "tree_size", "kiddo.stem"] {
            assert!(
                keys.contains(&expected),
                "missing facet {expected}; got {keys:?}"
            );
        }
    }

    #[test]
    fn narrows_the_offered_values_as_choices_are_made() {
        let catalog = catalog();
        let mut picker = Picker::new();
        let before = picker
            .facets(&catalog)
            .into_iter()
            .find(|f| f.key == "kiddo.stem")
            .unwrap();
        assert_eq!(before.values.len(), 9, "every declared stem strategy");

        picker.choose("impl", vec![word("nanoflann")]);
        let after = picker.facets(&catalog);
        assert!(
            !after.iter().any(|f| f.key == "kiddo.stem"),
            "kiddo's private axes should vanish once only nanoflann is selected"
        );
    }

    /// The picker must never offer a value that would empty the selection.
    #[test]
    fn never_offers_a_value_that_yields_no_points() {
        let catalog = catalog();
        let mut picker = Picker::new();
        picker.choose("impl", vec![word("kiddo_v6")]);
        for facet in picker.facets(&catalog) {
            for value in &facet.values {
                let mut probe = picker.clone();
                probe.choose(&facet.key, vec![value.clone()]);
                assert!(
                    !catalog.points(&probe.selection()).is_empty(),
                    "{}={} empties the selection",
                    facet.key,
                    value
                );
            }
        }
    }

    /// A choice can always be widened again: the values offered for a key ignore
    /// that key's own current pin.
    #[test]
    fn a_pinned_key_still_offers_its_alternatives() {
        let catalog = catalog();
        let mut picker = Picker::new();
        picker.choose("axis", vec![word("f64")]);
        let facet = picker
            .facets(&catalog)
            .into_iter()
            .find(|f| f.key == "axis")
            .unwrap();
        assert_eq!(facet.values.len(), 2, "f32 must still be offerable");
        assert_eq!(facet.chosen, vec![word("f64")]);
    }

    /// A param axis must offer its whole declared domain. An unconstrained
    /// param resolves to its default, so offering resolved values would show one
    /// tree size and make every other unreachable from the menu.
    #[test]
    fn param_facets_offer_the_declared_domain_not_the_default() {
        let catalog = catalog();
        let facet = Picker::new()
            .facets(&catalog)
            .into_iter()
            .find(|f| f.key == "tree_size")
            .unwrap();
        assert_eq!(facet.values.len(), 14, "2^16..2^29 inclusive");
        assert!(facet.values.contains(&TagValue::Int(1 << 16)));
        assert!(facet.values.contains(&TagValue::Int(1 << 29)));
    }

    #[test]
    fn preview_counts_cases_points_and_runners() {
        let catalog = catalog();
        let mut picker = Picker::new();
        picker.choose("impl", vec![word("kiddo_v6")]);
        let preview = picker.preview(&catalog, Runner::Criterion, &Budget::default());
        assert_eq!(preview.cases, 72);
        assert_eq!(preview.points, 72);
        assert_eq!(
            preview.runners,
            vec!["criterion".to_string(), "perf".to_string()]
        );
        assert!(preview.unsatisfied.is_empty());
    }

    /// perf runs one point per process, so its estimate must exceed criterion's
    /// for the same selection.
    #[test]
    fn perf_is_estimated_higher_than_criterion() {
        let catalog = catalog();
        let picker = Picker::new();
        let budget = Budget::default();
        let c = picker
            .preview(&catalog, Runner::Criterion, &budget)
            .estimate;
        let p = picker.preview(&catalog, Runner::Perf, &budget).estimate;
        assert!(p > c, "perf {p:?} should exceed criterion {c:?}");
        assert_eq!(
            c,
            Duration::from_secs(8) * 88,
            "88 points at warm-up+measurement (72 kiddo + 8 nanoflann + 8 pykdtree)"
        );
    }

    #[test]
    fn preview_surfaces_unreachable_requests() {
        let catalog = catalog();
        let mut picker = Picker::new();
        picker.choose("k", vec![TagValue::Int(1), TagValue::Int(999)]);
        let preview = picker.preview(&catalog, Runner::Criterion, &Budget::default());
        assert_eq!(preview.unsatisfied, vec!["k=999".to_string()]);
    }

    /// The printed command is the CI repro line, so it must parse back to the
    /// same selection it was generated from.
    #[test]
    fn printed_command_round_trips() {
        let mut picker = Picker::new();
        picker.choose("impl", vec![word("kiddo_v6")]);
        picker.choose("k", vec![TagValue::Int(1), TagValue::Int(5)]);
        picker.choose("tree_size", vec![TagValue::Int(1 << 20)]);

        let command = picker.command(Runner::Criterion);
        assert_eq!(
            command,
            "spatial-bench run --runner criterion \
             --select 'impl=kiddo_v6,k=1|5,tree_size=2^20'"
        );

        let expr = command.split('\'').nth(1).unwrap();
        assert_eq!(
            Selector::parse(expr).unwrap(),
            picker.selection().selectors[0]
        );
    }

    /// A selection that pins every facet must still print a readable line. Only
    /// the clauses that actually narrow the point set survive.
    #[test]
    fn the_repro_line_drops_clauses_that_narrow_nothing() {
        let catalog = catalog();
        let mut picker = Picker::new();
        // Pin every facet, as walking the whole menu does.
        for facet in Picker::new().facets(&catalog) {
            if let Some(first) = facet.values.first() {
                picker.choose(&facet.key, vec![first.clone()]);
            }
        }
        let full = picker.selection();
        let minimal = picker.minimal(&catalog);

        assert!(
            full.selectors[0].clauses.len() >= 14,
            "the picker pinned everything"
        );
        assert!(
            minimal.selectors[0].clauses.len() < full.selectors[0].clauses.len(),
            "redundant clauses should be dropped"
        );
        assert_eq!(
            catalog.points(&minimal).len(),
            catalog.points(&full).len(),
            "minimising must not change what is selected"
        );

        // `dims` has one reachable value, so constraining it narrows nothing.
        let keys: Vec<String> = minimal.selectors[0]
            .clauses
            .iter()
            .map(|c| c.key.to_string())
            .collect();
        assert!(!keys.contains(&"dims".to_string()), "got {keys:?}");
    }

    #[test]
    fn an_empty_selection_prints_a_runnable_command() {
        assert_eq!(
            Picker::new().command(Runner::Criterion),
            "spatial-bench run --runner criterion"
        );
    }
}
