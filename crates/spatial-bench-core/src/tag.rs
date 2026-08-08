//! Tag keys and values — the atoms of benchmark identity.
//!
//! A data point is identified by its tag map and nothing else. There is no name
//! to parse: everything a chart needs to group, facet or filter is a key here.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// A tag value. Deliberately narrow — these end up in filenames, selector
/// expressions and chart legends, so arbitrary nesting is not wanted.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TagValue {
    /// `stem=eytzinger`, `axis=f64`
    Word(String),
    /// `k=5`, `tree_size=1048576`
    Int(i64),
    /// `radius=0.05`, `epsilon=0.01`. Ordered via total-order bits so `TagValue`
    /// can be a `BTreeMap` key; NaN is rejected at construction.
    Float(F64Ord),
    /// `hugepages=on` is a `Word`, not this — reserve `Bool` for genuinely
    /// binary axes that read naturally as true/false.
    Bool(bool),
}

/// `f64` with a total order, so tag maps stay sortable and canonically
/// serialisable (which the machine hash and point de-duplication both rely on).
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct F64Ord(pub f64);

impl PartialEq for F64Ord {
    fn eq(&self, other: &Self) -> bool {
        self.0.to_bits() == other.0.to_bits()
    }
}
impl Eq for F64Ord {}
impl PartialOrd for F64Ord {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for F64Ord {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.total_cmp(&other.0)
    }
}

impl fmt::Display for TagValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TagValue::Word(w) => f.write_str(w),
            TagValue::Int(i) => write!(f, "{i}"),
            TagValue::Float(x) => write!(f, "{}", x.0),
            TagValue::Bool(b) => write!(f, "{b}"),
        }
    }
}

impl TagValue {
    /// Parse a value as written in a selector expression.
    ///
    /// `2^20` expands to `1048576` — the sizes are always powers of two and the
    /// exponent is what anyone actually says out loud.
    pub fn parse(raw: &str) -> Result<Self, TagError> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err(TagError::BadValue {
                key: String::new(),
                raw: raw.to_owned(),
            });
        }
        // `2^20` is how tree sizes are said out loud, so it is what the selector
        // accepts; it expands here so nothing downstream sees the shorthand.
        if let Some(exponent) = raw.strip_prefix("2^") {
            let n: u32 = exponent.parse().map_err(|_| TagError::BadValue {
                key: String::new(),
                raw: raw.to_owned(),
            })?;
            if n >= 63 {
                return Err(TagError::BadValue {
                    key: String::new(),
                    raw: raw.to_owned(),
                });
            }
            return Ok(TagValue::Int(1i64 << n));
        }
        if let Ok(i) = raw.parse::<i64>() {
            return Ok(TagValue::Int(i));
        }
        if let Ok(f) = raw.parse::<f64>() {
            // `nan` and `inf` parse as f64; they are not usable identities, and a
            // NaN would break the total order the tag map relies on.
            if f.is_finite() {
                return Ok(TagValue::Float(F64Ord(f)));
            }
            return Err(TagError::NaN);
        }
        Ok(match raw {
            "true" => TagValue::Bool(true),
            "false" => TagValue::Bool(false),
            other => TagValue::Word(other.to_owned()),
        })
    }

    /// Render for a selector expression, round-tripping through [`Self::parse`].
    ///
    /// `pow2` asks for `2^20` rather than `1048576`, which is what makes the
    /// printed repro line readable for tree sizes.
    pub fn to_expr(&self, pow2: bool) -> String {
        if pow2 {
            if let TagValue::Int(i) = self {
                if *i > 0 && i.count_ones() == 1 {
                    return format!("2^{}", i.trailing_zeros());
                }
            }
        }
        self.to_string()
    }
}

/// A tag key.
///
/// `Cow` rather than `&'static str` so one type serves both directions: catalog
/// declarations borrow their keys for free, while a result document read back
/// off disk owns them. Without this, reading results would need a parallel
/// `String`-keyed schema type and a lossy conversion between the two.
pub type TagKey = std::borrow::Cow<'static, str>;

/// A complete, ordered tag map. `BTreeMap` so serialisation is canonical and two
/// points with the same identity always produce byte-identical JSON.
pub type TagMap = BTreeMap<TagKey, TagValue>;

#[derive(Debug)]
pub enum TagError {
    UnknownKey(String),
    BadValue { key: String, raw: String },
    NotAllowed { key: String, value: String },
    NaN,
}

/// Build a [`TagMap`] literal:
///
/// ```ignore
/// tags! { impl_: "kiddo_v6", query: "exact_nn", k: 1, dims: 3, axis: "f64" }
/// ```
#[macro_export]
macro_rules! tags {
    ($($key:ident : $value:expr),* $(,)?) => {{
        let mut map = $crate::tag::TagMap::new();
        $( map.insert(
            $crate::tag::TagKey::Borrowed($crate::vocab::key_str(stringify!($key))),
            $crate::tag::IntoTagValue::into_tag_value($value),
        ); )*
        map
    }};
}

/// Conversion used by [`tags!`] so literals can be written unquoted where that
/// reads better.
pub trait IntoTagValue {
    fn into_tag_value(self) -> TagValue;
}
impl IntoTagValue for &str {
    fn into_tag_value(self) -> TagValue {
        TagValue::Word(self.to_owned())
    }
}
impl IntoTagValue for i64 {
    fn into_tag_value(self) -> TagValue {
        TagValue::Int(self)
    }
}
impl IntoTagValue for i32 {
    fn into_tag_value(self) -> TagValue {
        TagValue::Int(self as i64)
    }
}
impl IntoTagValue for usize {
    fn into_tag_value(self) -> TagValue {
        TagValue::Int(self as i64)
    }
}
impl IntoTagValue for bool {
    fn into_tag_value(self) -> TagValue {
        TagValue::Bool(self)
    }
}
impl IntoTagValue for f64 {
    fn into_tag_value(self) -> TagValue {
        TagValue::Float(F64Ord(self))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_each_value_shape() {
        assert_eq!(
            TagValue::parse("eytzinger").unwrap(),
            TagValue::Word("eytzinger".into())
        );
        assert_eq!(
            TagValue::parse("f64").unwrap(),
            TagValue::Word("f64".into())
        );
        assert_eq!(TagValue::parse("20").unwrap(), TagValue::Int(20));
        assert_eq!(TagValue::parse("-3").unwrap(), TagValue::Int(-3));
        assert_eq!(TagValue::parse("true").unwrap(), TagValue::Bool(true));
        assert_eq!(
            TagValue::parse("0.05").unwrap(),
            TagValue::Float(F64Ord(0.05))
        );
    }

    #[test]
    fn expands_power_of_two_shorthand() {
        assert_eq!(TagValue::parse("2^20").unwrap(), TagValue::Int(1_048_576));
        assert_eq!(TagValue::parse("2^0").unwrap(), TagValue::Int(1));
        // No shorthand survives into a tag map; downstream only ever sees ints.
        assert!(TagValue::parse("2^64").is_err());
        assert!(TagValue::parse("2^x").is_err());
    }

    /// `nan` and `inf` parse as f64 but would break the total order the tag map
    /// depends on for canonical serialisation.
    #[test]
    fn rejects_non_finite_floats() {
        assert!(matches!(TagValue::parse("nan"), Err(TagError::NaN)));
        assert!(matches!(TagValue::parse("inf"), Err(TagError::NaN)));
        assert!(matches!(TagValue::parse("-inf"), Err(TagError::NaN)));
    }

    #[test]
    fn pow2_rendering_round_trips() {
        for raw in ["2^16", "2^20", "2^25"] {
            let v = TagValue::parse(raw).unwrap();
            assert_eq!(v.to_expr(true), raw);
            assert_eq!(TagValue::parse(&v.to_expr(true)).unwrap(), v);
        }
        // Not a power of two, so it stays literal even when pow2 is asked for.
        assert_eq!(TagValue::parse("1000").unwrap().to_expr(true), "1000");
        assert_eq!(TagValue::parse("2^20").unwrap().to_expr(false), "1048576");
    }
}
