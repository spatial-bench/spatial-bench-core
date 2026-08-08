//! Loading and validating vendored subject manifests.

use crate::case::{Case, Param, ParamDomain};
use crate::catalog::Catalog;
use crate::manifest::{Manifest, ManifestError, ParamDecl};
use crate::tag::{TagKey, TagMap, TagValue};
use crate::vocab::{ExtKey, Vocabulary};
use std::collections::BTreeMap;
use std::path::Path;

/// Parse one `subjects/<name>/subject.toml`.
pub fn parse(path: &Path) -> Result<Manifest, ManifestError> {
    let text = std::fs::read_to_string(path).map_err(|e| ManifestError::Io {
        path: path.to_owned(),
        err: e.to_string(),
    })?;
    let manifest: Manifest = toml::from_str(&text).map_err(|e| ManifestError::Toml {
        path: path.to_owned(),
        err: e.to_string(),
    })?;
    if manifest.schema != crate::manifest::MANIFEST_SCHEMA {
        return Err(ManifestError::UnsupportedSchema {
            path: path.to_owned(),
            found: manifest.schema,
        });
    }
    Ok(manifest)
}

/// Load every subject under `subjects/`, merge their vocabularies, expand each
/// case's `matrix`, and validate the result.
pub fn load_dir(dir: &Path) -> Result<Catalog, ManifestError> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| ManifestError::Io {
            path: dir.to_owned(),
            err: e.to_string(),
        })?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    // Deterministic order: the catalog, and therefore every enumeration and
    // estimate built from it, must not depend on readdir order.
    entries.sort();

    let mut vocab = Vocabulary::default();
    let mut cases = Vec::new();
    let mut subjects: BTreeMap<String, crate::catalog::SubjectFacts> = BTreeMap::new();
    for subject_dir in entries {
        let path = subject_dir.join("subject.toml");
        if !path.exists() {
            continue;
        }
        let manifest = parse(&path)?;
        let (subject_cases, ext) = lower(&manifest)?;
        vocab
            .extend(&manifest.name, ext)
            .map_err(|e| ManifestError::UnknownKey {
                subject: manifest.name.clone(),
                key: format!("{e:?}"),
            })?;
        subjects.insert(
            manifest.name.clone(),
            crate::catalog::SubjectFacts {
                pinned_ref: manifest.source.pinned_ref.clone(),
                features: manifest.driver.features.clone(),
                min_rustc: manifest
                    .toolchain
                    .as_ref()
                    .and_then(|t| t.min_rustc.clone()),
                rustflags: manifest.driver.rustflags.clone(),
            },
        );
        cases.extend(subject_cases);
    }

    // Validation happens after every manifest is in, so a case may legitimately
    // reference an extension key declared by the subject it belongs to.
    for case in &cases {
        let subject = case
            .tags
            .get("impl")
            .map(ToString::to_string)
            .unwrap_or_default();
        for (key, value) in &case.tags {
            vocab
                .validate(key, value)
                .map_err(|_| ManifestError::UnknownKey {
                    subject: subject.clone(),
                    key: key.to_string(),
                })?;
        }
    }

    Ok(Catalog::new(cases, vocab, subjects))
}

/// Turn one manifest into concrete cases, expanding `matrix` into the cartesian
/// product. This is where a sweep stops being a nested for-loop in a bench file
/// and becomes data.
fn lower(manifest: &Manifest) -> Result<(Vec<Case>, Vec<ExtKey>), ManifestError> {
    if crate::vocab::domain(&manifest.domain).is_none() {
        return Err(ManifestError::UnknownDomain {
            subject: manifest.name.clone(),
            domain: manifest.domain.clone(),
        });
    }
    if manifest.source.pinned_ref.trim().is_empty() {
        return Err(ManifestError::MissingPin {
            subject: manifest.name.clone(),
        });
    }

    let ns = crate::vocab::namespace_of(&manifest.name);
    let ext: Vec<ExtKey> = manifest
        .vocab
        .iter()
        .map(|(key, decl)| ExtKey {
            subject: manifest.name.clone(),
            key: format!("{ns}.{key}"),
            allowed: decl.values.clone(),
            compile_time: decl.compile_time,
        })
        .collect();
    let mut compile_time_keys: Vec<String> = ext
        .iter()
        .filter(|e| e.compile_time)
        .map(|e| e.key.clone())
        .collect();
    // Extension keys marked compile_time, plus the core keys this subject
    // declares as generic. A core key like `axis` cannot carry the marking
    // itself: it is owned by the domain, and whether it is a generic
    // parameter depends on the subject.
    for key in &manifest.driver.compile_time {
        if crate::vocab::lookup(key).is_none() {
            return Err(ManifestError::UnknownKey {
                subject: manifest.name.clone(),
                key: format!("driver.compile_time names `{key}`, not a core key"),
            });
        }
        compile_time_keys.push(key.clone());
    }
    // Sorted so generated source is byte-identical run to run.
    compile_time_keys.sort();

    let mut cases = Vec::new();
    for decl in &manifest.cases {
        let base = toml_tags(&decl.tags, &manifest.name)?;
        if let Some(found) = base.get("impl") {
            // A subject must not declare cases attributed to another, or
            // vendoring everything in one place would buy nothing.
            if found.to_string() != manifest.name {
                return Err(ManifestError::ImplMismatch {
                    subject: manifest.name.clone(),
                    found: found.to_string(),
                });
            }
        }
        for combo in expand_matrix(&decl.matrix, &manifest.name)? {
            let mut tags = base.clone();
            tags.extend(combo);
            cases.push(Case {
                id: format!("{}:{}", manifest.name, cases.len()),
                tags,
                params: lower_params(&decl.params, &manifest.name)?,
                runners: decl.runners.clone(),
                adapter: manifest.driver.adapter.clone(),
                subject: manifest.name.clone(),
                driver_crate: manifest.driver.crate_name.clone(),
                driver_macro: manifest.driver.macro_name.clone(),
                command: decl.command.clone(),
                output: decl.output.clone(),
                compile_time_keys: compile_time_keys.clone(),
            });
        }
    }
    Ok((cases, ext))
}

/// Cartesian product of the `matrix` axes, in a deterministic order.
fn expand_matrix(
    matrix: &BTreeMap<String, Vec<toml::Value>>,
    subject: &str,
) -> Result<Vec<TagMap>, ManifestError> {
    let mut out = vec![TagMap::new()];
    for (key, values) in matrix {
        let mut next = Vec::with_capacity(out.len() * values.len());
        for base in &out {
            for raw in values {
                let value = tag_value(raw, key, subject)?;
                let mut tags = base.clone();
                tags.insert(TagKey::Owned(key.clone()), value);
                next.push(tags);
            }
        }
        out = next;
    }
    Ok(out)
}

fn lower_params(
    params: &BTreeMap<String, ParamDecl>,
    subject: &str,
) -> Result<Vec<Param>, ManifestError> {
    params
        .iter()
        .map(|(key, decl)| {
            let domain = if let Some(range) = &decl.range {
                parse_range(range, key, subject)?
            } else if !decl.values.is_empty() {
                ParamDomain::Values(
                    decl.values
                        .iter()
                        .map(|v| tag_value(v, key, subject))
                        .collect::<Result<_, _>>()?,
                )
            } else {
                match decl.kind.as_deref() {
                    Some("float") => ParamDomain::AnyFloat,
                    _ => ParamDomain::AnyInt,
                }
            };
            let default = decl
                .default
                .as_ref()
                .map(|v| tag_value(v, key, subject))
                .transpose()?
                .ok_or_else(|| ManifestError::UnknownKey {
                    subject: subject.to_owned(),
                    key: format!("{key} has no default"),
                })?;
            Ok(Param {
                key: key.clone(),
                domain,
                default,
            })
        })
        .collect()
}

/// `"2^16..2^25"` enumerates powers of two; `"1..16"` a plain integer range.
/// Enumerating rather than storing endpoints keeps a param domain something the
/// interactive picker can offer as concrete choices.
fn parse_range(raw: &str, key: &str, subject: &str) -> Result<ParamDomain, ManifestError> {
    let bad = || ManifestError::UnknownKey {
        subject: subject.to_owned(),
        key: format!("{key}: cannot parse range `{raw}`"),
    };
    let (lo, hi) = raw.split_once("..").ok_or_else(bad)?;
    if let (Some(l), Some(h)) = (lo.trim().strip_prefix("2^"), hi.trim().strip_prefix("2^")) {
        let (l, h): (u32, u32) = (l.parse().map_err(|_| bad())?, h.parse().map_err(|_| bad())?);
        if l > h || h >= 63 {
            return Err(bad());
        }
        return Ok(ParamDomain::Values(
            (l..=h).map(|n| TagValue::Int(1i64 << n)).collect(),
        ));
    }
    let (l, h): (i64, i64) = (
        lo.trim().parse().map_err(|_| bad())?,
        hi.trim().parse().map_err(|_| bad())?,
    );
    if l > h {
        return Err(bad());
    }
    Ok(ParamDomain::IntRange {
        lo: l,
        hi: h,
        step: 1,
    })
}

fn toml_tags(raw: &BTreeMap<String, toml::Value>, subject: &str) -> Result<TagMap, ManifestError> {
    raw.iter()
        .map(|(k, v)| Ok((TagKey::Owned(k.clone()), tag_value(v, k, subject)?)))
        .collect()
}

fn tag_value(raw: &toml::Value, key: &str, subject: &str) -> Result<TagValue, ManifestError> {
    match raw {
        toml::Value::String(s) => TagValue::parse(s).map_err(|_| ManifestError::UnknownKey {
            subject: subject.to_owned(),
            key: format!("{key}: bad value `{s}`"),
        }),
        toml::Value::Integer(i) => Ok(TagValue::Int(*i)),
        toml::Value::Float(f) => Ok(TagValue::Float(crate::tag::F64Ord(*f))),
        toml::Value::Boolean(b) => Ok(TagValue::Bool(*b)),
        other => Err(ManifestError::UnknownKey {
            subject: subject.to_owned(),
            key: format!("{key}: unsupported value {other:?}"),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::selector::SelectorSet;

    fn subjects_dir() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../subjects")
    }

    /// Both vendored manifests parse under the current schema.
    #[test]
    fn vendored_manifests_parse() {
        for name in ["kiddo", "nanoflann"] {
            let path = subjects_dir().join(name).join("subject.toml");
            let manifest = parse(&path).unwrap_or_else(|e| panic!("{name}: {e:?}"));
            assert_eq!(manifest.schema, crate::manifest::MANIFEST_SCHEMA);
            assert!(!manifest.cases.is_empty(), "{name} declares no cases");
        }
    }

    /// Every subject names a domain the engine knows, so its cases are written
    /// against a core vocabulary that actually exists.
    #[test]
    fn every_subject_declares_a_known_domain() {
        for name in ["kiddo", "nanoflann"] {
            let manifest = parse(&subjects_dir().join(name).join("subject.toml")).unwrap();
            assert!(
                crate::vocab::domain(&manifest.domain).is_some(),
                "{name}: unknown domain `{}`",
                manifest.domain
            );
        }
    }

    /// Every subject is pinned. Without this a dataset spanning months is not
    /// comparable and nothing records why.
    #[test]
    fn every_subject_is_pinned() {
        for name in ["kiddo", "nanoflann"] {
            let manifest = parse(&subjects_dir().join(name).join("subject.toml")).unwrap();
            assert!(!manifest.source.pinned_ref.is_empty(), "{name} is unpinned");
        }
    }

    /// A case's `impl` must match the manifest that declares it, so no subject
    /// can contribute cases attributed to another.
    #[test]
    fn cases_are_attributed_to_their_own_subject() {
        for name in ["kiddo", "nanoflann"] {
            let manifest = parse(&subjects_dir().join(name).join("subject.toml")).unwrap();
            for case in &manifest.cases {
                let decl = case.tags.get("impl").and_then(|v| v.as_str());
                assert_eq!(
                    decl,
                    Some(manifest.name.as_str()),
                    "{name}: impl tag mismatch"
                );
            }
        }
    }

    /// kiddo is vendored on exactly the same terms as an unwilling third party.
    /// If this ever fails, the trust argument has been quietly abandoned.
    #[test]
    fn kiddo_has_no_privileged_path() {
        let kiddo = parse(&subjects_dir().join("kiddo/subject.toml")).unwrap();
        let nanoflann = parse(&subjects_dir().join("nanoflann/subject.toml")).unwrap();
        assert_eq!(kiddo.schema, nanoflann.schema);
        assert!(!kiddo.source.pinned_ref.is_empty());
        assert!(
            !kiddo.cases.is_empty() && !nanoflann.cases.is_empty(),
            "both subjects declare cases through the same manifest format"
        );
    }

    // ---- end-to-end loading -------------------------------------------------

    fn catalog() -> Catalog {
        load_dir(&subjects_dir()).expect("vendored manifests should load")
    }

    /// The matrix expands into real cases. For kiddo: seven unconstrained
    /// strategies over two scalars and four values of k, plus the two cyclic
    /// SIMD strategies declared once per scalar because each pins the block
    /// height its assertions demand.
    #[test]
    fn matrix_expands_into_cases() {
        let catalog = catalog();
        assert_eq!(
            catalog.for_subject("kiddo_v6").count(),
            7 * 2 * 4 + 2 * 4 + 2 * 4
        );
        assert_eq!(catalog.for_subject("nanoflann").count(), 8);
    }

    /// The cyclic SIMD strategies assert BH==3 for f64 and BH==4 for f32. The
    /// catalog must never offer the other pairing, or a run would panic inside
    /// the timed region.
    #[test]
    fn cyclic_strategies_only_appear_at_their_required_block_height() {
        let catalog = catalog();
        for case in catalog.cases() {
            let Some(stem) = case.tags.get("kiddo.stem").map(ToString::to_string) else {
                continue;
            };
            if !stem.starts_with("donnelly_cyclic") {
                continue;
            }
            let axis = case.tags.get("axis").map(ToString::to_string).unwrap();
            let bh = case
                .tags
                .get("kiddo.block_height")
                .map(ToString::to_string)
                .unwrap();
            let required = if axis == "f64" { "3" } else { "4" };
            assert_eq!(
                bh, required,
                "{stem} on {axis} must use block height {required}"
            );
        }
    }

    /// Extension keys arrive namespaced by subject, not by subject *version*:
    /// `kiddo.stem`, so a v7 manifest later does not split a chart that should
    /// stay continuous.
    #[test]
    fn extensions_are_namespaced_without_the_version() {
        let catalog = catalog();
        let vocab = catalog.vocabulary();
        assert!(vocab.knows("kiddo.stem"));
        assert!(vocab.knows("nanoflann.leaf_max_size"));
        assert!(!vocab.knows("kiddo_v6.stem"));
        assert!(
            !vocab.knows("stem"),
            "extension must not leak into the core"
        );
    }

    /// An unconstrained param contributes only its default, so enumerating the
    /// catalog does not silently plan a 10x sweep nobody asked for.
    #[test]
    fn unconstrained_params_use_their_default() {
        let catalog = catalog();
        let all = catalog.points(&SelectorSet::default());
        assert_eq!(all.len(), 72 + 8, "one point per case at default params");
        for (_, tags) in &all {
            assert_eq!(tags.get("tree_size"), Some(&TagValue::Int(1 << 20)));
            assert_eq!(tags.get("query_count"), Some(&TagValue::Int(1000)));
        }
    }

    /// …and a constrained one sweeps exactly the constrained values.
    #[test]
    fn constrained_params_sweep() {
        let catalog = catalog();
        let sel = SelectorSet::parse_all([
            "impl=kiddo_v6,k=1,kiddo.stem=eytzinger,tree_size=2^20|2^23|2^26",
        ])
        .unwrap();
        let points = catalog.points(&sel);
        assert_eq!(points.len(), 2 * 3, "2 scalars x 3 sizes at k=1");
        // 2^26 must be reachable: the declared range is what is worth sweeping,
        // not a capability limit. What actually fits is a memory question the
        // engine answers per machine.
        let sizes: std::collections::BTreeSet<_> = points
            .iter()
            .filter_map(|(_, t)| t.get("tree_size"))
            .collect();
        assert_eq!(sizes.len(), 3);
    }

    /// A range over an enumerable domain expands to the domain members inside
    /// it, not to its endpoints.
    #[test]
    fn ranges_expand_within_the_declared_domain() {
        let catalog = catalog();
        let sel = SelectorSet::parse_all([
            "impl=kiddo_v6,k=1,axis=f64,kiddo.stem=eytzinger,tree_size=2^16..2^19",
        ])
        .unwrap();
        let points = catalog.points(&sel);
        assert_eq!(points.len(), 4, "2^16, 2^17, 2^18, 2^19");
    }

    /// A cross-subject selection is the point of the shared domain core: both
    /// subjects name `k` and `axis` identically, so one selector reaches both.
    #[test]
    fn one_selector_reaches_both_subjects() {
        let catalog = catalog();
        let sel = SelectorSet::parse_all(["query=exact_nn,k=20,axis=f64"]).unwrap();
        let subjects: std::collections::BTreeSet<_> = catalog
            .points(&sel)
            .iter()
            .map(|(c, _)| c.subject.clone())
            .collect();
        assert_eq!(subjects.len(), 2, "core vocabulary should span subjects");
    }

    /// Extension keys stay private to their subject: asking about kiddo's stem
    /// excludes nanoflann rather than matching it vacuously.
    #[test]
    fn extension_selection_excludes_other_subjects() {
        let catalog = catalog();
        let sel = SelectorSet::parse_all(["kiddo.stem=eytzinger"]).unwrap();
        let subjects: std::collections::BTreeSet<_> = catalog
            .points(&sel)
            .iter()
            .map(|(c, _)| c.subject.clone())
            .collect();
        assert_eq!(subjects, ["kiddo_v6".to_string()].into_iter().collect());
    }

    /// Load order must not depend on readdir order, or two machines would build
    /// different-looking catalogs from identical inputs.
    #[test]
    fn load_is_deterministic() {
        let a = catalog();
        let b = catalog();
        let ids: Vec<_> = a.cases().iter().map(|c| c.id.clone()).collect();
        let ids2: Vec<_> = b.cases().iter().map(|c| c.id.clone()).collect();
        assert_eq!(ids, ids2);
    }

    /// Non-core keys must be namespaced under their subject, so `kiddo.stem` and
    /// a future `pkdtree.stem` cannot collide.
    #[test]
    fn extension_keys_are_namespaced() {
        for (name, prefix) in [("kiddo", "kiddo."), ("nanoflann", "nanoflann.")] {
            let manifest = parse(&subjects_dir().join(name).join("subject.toml")).unwrap();
            for case in &manifest.cases {
                for key in case.tags.keys() {
                    if crate::vocab::lookup(key).is_none() {
                        assert!(
                            key.starts_with(prefix),
                            "{name}: `{key}` is neither core nor namespaced under `{prefix}`"
                        );
                    }
                }
            }
        }
    }
}
