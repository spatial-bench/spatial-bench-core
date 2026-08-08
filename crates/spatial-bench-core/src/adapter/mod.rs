//! Adapters turn (case, resolved params) into data points.
//!
//! The split exists so that a subject unwilling to cooperate is a first-class
//! citizen: `criterion_rust` drives a Rust bench binary, `exec` drives any
//! command at all. Adding a language means adding a manifest, not engine code.

pub mod criterion_rust;

/// Substitute `{key}` placeholders from a resolved tag map.
///
/// Used for both criterion filters and `exec` command lines, so a subject's
/// manifest can name axes rather than repeating a case per combination.
pub fn template(pattern: &str, tags: &crate::tag::TagMap) -> Result<String, TemplateError> {
    let mut out = String::with_capacity(pattern.len());
    let mut rest = pattern;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else {
            return Err(TemplateError::Unterminated {
                pattern: pattern.to_owned(),
            });
        };
        let key = &after[..close];
        let Some(value) = tags.get(key) else {
            return Err(TemplateError::UnknownKey {
                pattern: pattern.to_owned(),
                key: key.to_owned(),
            });
        };
        out.push_str(&value.to_string());
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

#[derive(Debug, PartialEq)]
pub enum TemplateError {
    /// A `{` with no `}`.
    Unterminated { pattern: String },
    /// A placeholder naming a tag the point does not carry. Fatal rather than
    /// left literal: a filter with an unsubstituted `{axis}` would match no
    /// benchmark and look like an empty run.
    UnknownKey { pattern: String, key: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tags;

    fn point() -> crate::tag::TagMap {
        tags! { axis: "f64", k: 20, dims: 3, tree_size: 1_048_576usize }
    }

    #[test]
    fn substitutes_every_placeholder() {
        assert_eq!(
            template("profile_v6/{axis}/kiddo_nearest_n_k{k}", &point()).unwrap(),
            "profile_v6/f64/kiddo_nearest_n_k20"
        );
        assert_eq!(
            template("--points={tree_size}", &point()).unwrap(),
            "--points=1048576"
        );
        assert_eq!(
            template("no placeholders", &point()).unwrap(),
            "no placeholders"
        );
    }

    /// An unsubstituted placeholder would silently match no benchmark and read
    /// as an empty run, so it is an error rather than passed through literally.
    #[test]
    fn unknown_placeholders_are_fatal() {
        assert_eq!(
            template("{nope}", &point()),
            Err(TemplateError::UnknownKey {
                pattern: "{nope}".into(),
                key: "nope".into()
            })
        );
        assert!(matches!(
            template("{axis", &point()),
            Err(TemplateError::Unterminated { .. })
        ));
    }
}
