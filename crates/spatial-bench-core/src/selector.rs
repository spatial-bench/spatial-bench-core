//! The selector language.
//!
//! ```text
//! impl=kiddo_v6,query=exact_nn,axis=f64,k=1|5|20,tree_size=2^20..2^26
//! ```
//!
//! `,` = AND · `|` = OR within a key · `..` = inclusive range · `*` = key must
//! exist · `!key=value` = negate. Repeating `--select` unions the matched sets.
//!
//! The same expression both filters which cases run and pins which param values
//! are swept, so there is no separate `--param` flag to keep in sync.
//!
//! Note the selector never reaches a subject. Because every subject is vendored,
//! the engine resolves the selection itself and drives each case through its
//! adapter -- stock criterion filter args for Rust, a templated command line for
//! everything else -- so no subject needs to link or parse anything of ours.

use crate::tag::{TagKey, TagMap, TagValue};

#[derive(Clone, Debug, PartialEq)]
pub enum Match {
    /// `k=1|5|20`
    OneOf(Vec<TagValue>),
    /// `tree_size=2^20..2^26`, inclusive
    Range { lo: TagValue, hi: TagValue },
    /// `stem=*` — any value, but the key must be present.
    Any,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Clause {
    pub key: TagKey,
    pub matcher: Match,
    /// `!key=value`. Negation still requires the key to be present, so
    /// `!kiddo.stem=eytzinger` does not admit a subject that has no stem at all.
    pub negated: bool,
}

impl Clause {
    /// Does a tag map satisfy this clause?
    ///
    /// A missing key never satisfies, negated or not — see [`Clause::negated`].
    pub fn admits(&self, tags: &TagMap) -> bool {
        let Some(value) = tags.get(&self.key) else {
            return false;
        };
        let hit = match &self.matcher {
            Match::Any => true,
            Match::OneOf(values) => values.contains(value),
            Match::Range { lo, hi } => value >= lo && value <= hi,
        };
        hit != self.negated
    }
}

/// A conjunction of clauses. Multiple selectors from repeated `--select` flags
/// are unioned by [`SelectorSet`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Selector {
    pub clauses: Vec<Clause>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SelectorSet {
    pub selectors: Vec<Selector>,
}

fn split_top(expr: &str, sep: char) -> Vec<&str> {
    expr.split(sep)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect()
}

impl Selector {
    pub fn parse(expr: &str) -> Result<Self, SelectorError> {
        let mut clauses = Vec::new();
        for raw in split_top(expr, ',') {
            clauses.push(Self::parse_clause(raw)?);
        }
        if clauses.is_empty() {
            return Err(SelectorError::Syntax {
                at: 0,
                msg: "empty selector".into(),
            });
        }
        Ok(Self { clauses })
    }

    fn parse_clause(raw: &str) -> Result<Clause, SelectorError> {
        let (negated, body) = match raw.strip_prefix('!') {
            Some(rest) => (true, rest.trim()),
            None => (false, raw),
        };
        let Some((key, value)) = body.split_once('=') else {
            return Err(SelectorError::Syntax {
                at: 0,
                msg: format!("`{raw}` is not key=value"),
            });
        };
        let key = key.trim();
        let value = value.trim();
        if key.is_empty() {
            return Err(SelectorError::Syntax {
                at: 0,
                msg: "missing key".into(),
            });
        }

        let matcher = if value == "*" {
            Match::Any
        } else if let Some((lo, hi)) = value.split_once("..") {
            let lo = TagValue::parse(lo).map_err(|_| SelectorError::BadValue {
                key: key.to_owned(),
                raw: lo.to_owned(),
            })?;
            let hi = TagValue::parse(hi).map_err(|_| SelectorError::BadValue {
                key: key.to_owned(),
                raw: hi.to_owned(),
            })?;
            if lo > hi {
                return Err(SelectorError::InvertedRange {
                    key: key.to_owned(),
                });
            }
            Match::Range { lo, hi }
        } else {
            let mut values = Vec::new();
            for part in split_top(value, '|') {
                values.push(TagValue::parse(part).map_err(|_| SelectorError::BadValue {
                    key: key.to_owned(),
                    raw: part.to_owned(),
                })?);
            }
            if values.is_empty() {
                return Err(SelectorError::BadValue {
                    key: key.to_owned(),
                    raw: value.to_owned(),
                });
            }
            Match::OneOf(values)
        };

        Ok(Clause {
            key: TagKey::Owned(key.to_owned()),
            matcher,
            negated,
        })
    }

    /// Keys this selector constrains.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.clauses.iter().map(|c| c.key.as_ref())
    }

    /// The clause constraining `key`, if any.
    pub fn clause_for(&self, key: &str) -> Option<&Clause> {
        self.clauses.iter().find(|c| c.key.as_ref() == key)
    }

    /// Does a case's fixed tags survive?
    ///
    /// Clauses naming one of the case's param axes are deferred to
    /// [`crate::case::Case::expand`]: a case whose `k` is fixed at 1 must not be
    /// filtered out by a `tree_size` constraint it has not resolved yet.
    pub fn admits_case(&self, tags: &TagMap, params: &[&str]) -> bool {
        self.clauses
            .iter()
            .filter(|c| !params.contains(&c.key.as_ref()))
            .all(|c| c.admits(tags))
    }

    /// Does a fully-resolved data point survive?
    pub fn admits_point(&self, tags: &TagMap) -> bool {
        self.clauses.iter().all(|c| c.admits(tags))
    }

    /// Canonical form. This is the repro line the interactive picker prints, so
    /// it must round-trip through [`Selector::parse`].
    pub fn to_expr(&self) -> String {
        self.clauses
            .iter()
            .map(|c| {
                let pow2 = crate::vocab::lookup(c.key.as_ref()).is_some_and(|d| d.pow2);
                let value = match &c.matcher {
                    Match::Any => "*".to_owned(),
                    Match::OneOf(vs) => vs
                        .iter()
                        .map(|v| v.to_expr(pow2))
                        .collect::<Vec<_>>()
                        .join("|"),
                    Match::Range { lo, hi } => {
                        format!("{}..{}", lo.to_expr(pow2), hi.to_expr(pow2))
                    }
                };
                format!("{}{}={}", if c.negated { "!" } else { "" }, c.key, value)
            })
            .collect::<Vec<_>>()
            .join(",")
    }
}

impl SelectorSet {
    /// An empty set matches everything — no `--select` means "the lot".
    pub fn parse_all<I, S>(exprs: I) -> Result<Self, SelectorError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let selectors = exprs
            .into_iter()
            .map(|e| Selector::parse(e.as_ref()))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { selectors })
    }

    pub fn admits_case(&self, tags: &TagMap, params: &[&str]) -> bool {
        self.selectors.is_empty() || self.selectors.iter().any(|s| s.admits_case(tags, params))
    }

    pub fn admits_point(&self, tags: &TagMap) -> bool {
        self.selectors.is_empty() || self.selectors.iter().any(|s| s.admits_point(tags))
    }

    pub fn to_exprs(&self) -> Vec<String> {
        self.selectors.iter().map(Selector::to_expr).collect()
    }
}

#[derive(Debug, PartialEq)]
pub enum SelectorError {
    UnknownKey(String),
    Syntax {
        at: usize,
        msg: String,
    },
    BadValue {
        key: String,
        raw: String,
    },
    /// e.g. `tree_size=2^26..2^20`
    InvertedRange {
        key: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tags;

    fn point() -> TagMap {
        tags! {
            impl_: "kiddo_v6", query: "exact_nn", k: 1, dims: 3, axis: "f64",
            tree_size: 1_048_576usize, query_count: 1000usize,
        }
    }

    #[test]
    fn parses_the_documented_grammar() {
        let s = Selector::parse("impl=kiddo_v6,k=1|5|20,tree_size=2^20..2^26,axis=*,!query=build")
            .unwrap();
        assert_eq!(s.clauses.len(), 5);
        assert_eq!(s.clause_for("axis").unwrap().matcher, Match::Any);
        assert!(s.clause_for("query").unwrap().negated);
        assert_eq!(
            s.clause_for("k").unwrap().matcher,
            Match::OneOf(vec![TagValue::Int(1), TagValue::Int(5), TagValue::Int(20)])
        );
    }

    #[test]
    fn and_within_a_selector() {
        assert!(Selector::parse("impl=kiddo_v6,axis=f64")
            .unwrap()
            .admits_point(&point()));
        assert!(!Selector::parse("impl=kiddo_v6,axis=f32")
            .unwrap()
            .admits_point(&point()));
    }

    #[test]
    fn or_within_a_key() {
        assert!(Selector::parse("k=1|5|20").unwrap().admits_point(&point()));
        assert!(!Selector::parse("k=5|20").unwrap().admits_point(&point()));
    }

    #[test]
    fn inclusive_ranges() {
        for expr in ["tree_size=2^20..2^26", "tree_size=2^16..2^20"] {
            assert!(
                Selector::parse(expr).unwrap().admits_point(&point()),
                "{expr}"
            );
        }
        assert!(!Selector::parse("tree_size=2^21..2^26")
            .unwrap()
            .admits_point(&point()));
        assert_eq!(
            Selector::parse("tree_size=2^26..2^20"),
            Err(SelectorError::InvertedRange {
                key: "tree_size".into()
            })
        );
    }

    /// A missing key never satisfies a clause, negated or not. So a selector
    /// asking about `kiddo.stem` excludes a subject that has no stem, rather
    /// than admitting it vacuously.
    #[test]
    fn negation_still_requires_the_key() {
        assert!(Selector::parse("!k=50").unwrap().admits_point(&point()));
        assert!(!Selector::parse("!k=1").unwrap().admits_point(&point()));
        assert!(!Selector::parse("!kiddo.stem=eytzinger")
            .unwrap()
            .admits_point(&point()));
        assert!(!Selector::parse("kiddo.stem=*")
            .unwrap()
            .admits_point(&point()));
    }

    /// Param clauses are deferred: a case has not resolved its sweep yet, so a
    /// constraint on a param must not filter the case out.
    #[test]
    fn param_clauses_are_deferred_at_case_level() {
        let case_tags = tags! { impl_: "kiddo_v6", query: "exact_nn", k: 1, axis: "f64" };
        let sel = Selector::parse("impl=kiddo_v6,tree_size=2^26").unwrap();
        assert!(sel.admits_case(&case_tags, &["tree_size", "query_count"]));
        // …but it does apply once the point is resolved.
        assert!(!sel.admits_point(&point()));
    }

    #[test]
    fn repeated_selectors_union() {
        let set = SelectorSet::parse_all(["axis=f32", "k=1"]).unwrap();
        assert!(set.admits_point(&point()));
        let set = SelectorSet::parse_all(["axis=f32", "k=99"]).unwrap();
        assert!(!set.admits_point(&point()));
    }

    #[test]
    fn empty_set_matches_everything() {
        assert!(SelectorSet::default().admits_point(&point()));
    }

    /// The printed repro line has to parse back to the same selection, or CI
    /// would not reproduce what the interactive picker ran.
    #[test]
    fn canonical_form_round_trips() {
        for expr in [
            "impl=kiddo_v6,k=1|5|20,tree_size=2^20..2^26",
            "axis=*,!query=build",
            "tree_size=2^20|2^23|2^26,query_count=1000",
            "kiddo.stem=eytzinger|donnelly_cyclic_simd_full",
        ] {
            let parsed = Selector::parse(expr).unwrap();
            let rendered = parsed.to_expr();
            assert_eq!(rendered, expr, "canonical form changed");
            assert_eq!(Selector::parse(&rendered).unwrap(), parsed);
        }
    }

    #[test]
    fn rejects_malformed_input() {
        assert!(Selector::parse("nonsense").is_err());
        assert!(Selector::parse("=value").is_err());
        assert!(Selector::parse("").is_err());
        assert!(Selector::parse("k=2^99").is_err());
    }
}
