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
            .map_err(|e| ManifestError::Vocab {
                subject: manifest.name.clone(),
                message: e.to_string(),
            })?;
        subjects.insert(
            manifest.name.clone(),
            crate::catalog::SubjectFacts {
                pinned_ref: manifest.source.pinned_ref.clone(),
                source_kind: manifest.source.kind.clone(),
                repo: manifest.source.repo.clone(),
                driver_version: manifest.drivers[0].version.clone(),
                expected_sha: manifest.source.sha.clone(),
                source_package: manifest.source.package.clone(),
                driver_path: manifest.drivers[0].path.clone(),
                manifest_dir: subject_dir.clone(),
                features: manifest.drivers[0].features.clone(),
                min_rustc: manifest
                    .toolchain
                    .as_ref()
                    .and_then(|t| t.min_rustc.clone()),
                rustflags: manifest.drivers[0].rustflags.clone(),
                build: manifest.build.clone(),
                drivers: manifest
                    .drivers
                    .iter()
                    .map(|d| crate::catalog::DriverFacts {
                        name: d.name.clone(),
                        min_supported_semver: d.min_supported_semver.clone(),
                        max_supported_semver: d.max_supported_semver.clone(),
                        version: d.version.clone(),
                        path: d.path.clone(),
                        features: d.features.clone(),
                        rustflags: d.rustflags.clone(),
                        macro_name: d.macro_name.clone(),
                        crate_name: d.crate_name.clone(),
                    })
                    .collect(),
            },
        );
        cases.extend(subject_cases);
    }

    // Validation happens after every manifest is in, so a case may legitimately
    // reference an extension key declared by the subject it belongs to. The
    // vocabulary's own message is carried through: "value `x` is not allowed
    // for `impl` … a new subject belongs in `UNIVERSAL`" says what to do;
    // the key name alone said nothing .
    for case in &cases {
        let subject = case
            .tags
            .get("impl")
            .map(ToString::to_string)
            .unwrap_or_default();
        for (key, value) in &case.tags {
            vocab
                .validate(key, value)
                .map_err(|e| ManifestError::Vocab {
                    subject: subject.clone(),
                    message: e.to_string(),
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

    // the source kind is typed here or nowhere — same closed-vocabulary
    // rule as the adapter, so a typo is a load error, not a run-time surprise
    // phrased as "not implemented". `pypi` is the python exec story:
    // the pin is the immutable PyPI version.
    match manifest.source.kind.as_str() {
        "cargo-git" | "git" | "pypi" => {}
        other => {
            return Err(ManifestError::UnknownSourceKind {
                subject: manifest.name.clone(),
                found: other.to_owned(),
            });
        }
    }
    // a declared sha must look like a full commit id — anything shorter
    // would silently weaken the pin to a prefix match.
    if let Some(sha) = &manifest.source.sha {
        if !crate::build::is_commit_id(sha) {
            return Err(ManifestError::BadSha {
                subject: manifest.name.clone(),
                found: sha.clone(),
            });
        }
    }

    // The adapter is typed here or nowhere: an unknown kind is a load error
    // naming the subject (§5's dispatch needs to know every kind it will see),
    // and a two-phase driver without a macro is a broken manifest.
    for driver in &manifest.drivers {
        let adapter = crate::adapter::Adapter::parse(&driver.adapter).ok_or(
            ManifestError::UnknownAdapter {
                subject: manifest.name.clone(),
                found: driver.adapter.clone(),
            },
        )?;
        if adapter == crate::adapter::Adapter::RustCodegen {
            if driver.macro_name.is_none() {
                return Err(ManifestError::UnknownKey {
                    subject: manifest.name.clone(),
                    key: format!(
                        "driver `{}`: driver.macro is required for the rust-codegen adapter",
                        driver.name
                    ),
                });
            }
            if driver.crate_name.is_none() {
                return Err(ManifestError::UnknownKey {
                    subject: manifest.name.clone(),
                    key: format!(
                        "driver `{}`: driver.crate is required for the rust-codegen adapter",
                        driver.name
                    ),
                });
            }
        }
        if adapter == crate::adapter::Adapter::Exec {
            // an exec subject names its language and entry file, and a
            // build recipe — the harness contract is the interface, but the
            // build is the language's.
            match driver.lang.as_deref() {
                Some("cxx") | Some("python") => {}
                other => {
                    return Err(ManifestError::UnknownKey {
                        subject: manifest.name.clone(),
                        key: format!(
                            "driver `{}`: driver.lang {:?} — exec subjects declare \
                             `cxx` or `python`",
                            driver.name,
                            other.unwrap_or("(missing)")
                        ),
                    });
                }
            }
            if driver.entry.is_none() {
                return Err(ManifestError::UnknownKey {
                    subject: manifest.name.clone(),
                    key: format!(
                        "driver `{}`: driver.entry is required for exec subjects",
                        driver.name
                    ),
                });
            }
            if manifest.build.is_none() {
                return Err(ManifestError::UnknownKey {
                    subject: manifest.name.clone(),
                    key: "a [build] recipe is required for exec subjects".to_owned(),
                });
            }
        }
    }

    let ns = crate::vocab::namespace_of(&manifest.name);
    // Declared values are checked here as well as used ones: a value no case
    // happens to use is still vocabulary the selector must be able to say.
    for (key, decl) in &manifest.vocab {
        for raw in &decl.values.clone().unwrap_or_default() {
            let value = TagValue::parse(raw).map_err(|_| ManifestError::UnknownKey {
                subject: manifest.name.clone(),
                key: format!("{ns}.{key}: bad declared value `{raw}`"),
            })?;
            ensure_selectable(&format!("{ns}.{key}"), &value, &manifest.name)?;
        }
    }
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
    // Extension keys marked compile_time, plus the core keys this subject
    // declares as generic. A core key like `axis` cannot carry the marking
    // itself: it is owned by the domain, and whether it is a generic
    // parameter depends on the subject.
    for driver in &manifest.drivers {
        for key in &driver.compile_time {
            if crate::vocab::lookup(key).is_none() {
                return Err(ManifestError::UnknownKey {
                    subject: manifest.name.clone(),
                    key: format!(
                        "driver `{}`: compile_time names `{key}`, not a core key",
                        driver.name
                    ),
                });
            }
        }
    }

    // Parse the declared defaults once: a typo'd value is a load error, not a
    // silent stream of "tuned" labels.
    let defaults: Vec<(String, crate::tag::TagValue)> = manifest
        .defaults
        .iter()
        .map(|(key, default)| Ok((key.clone(), tag_value(default, key, &manifest.name)?)))
        .collect::<Result<_, _>>()?;

    // Every case runs under one driver. With a single driver block the
    // per-case reference is optional; with several it is required, so a
    // case cannot silently land under the wrong API.
    let default_driver = if manifest.drivers.len() == 1 {
        Some(manifest.drivers[0].name.clone())
    } else {
        None
    };
    if manifest.drivers.is_empty() {
        return Err(ManifestError::UnknownKey {
            subject: manifest.name.clone(),
            key: "no [[driver]] block — the engine cannot build this subject".to_owned(),
        });
    }
    let driver_for = |decl: &crate::manifest::CaseDecl| -> Result<String, ManifestError> {
        match (&decl.driver, &default_driver) {
            (Some(d), _) => Ok(d.clone()),
            (None, Some(d)) => Ok(d.clone()),
            (None, None) => Err(ManifestError::UnknownKey {
                subject: manifest.name.clone(),
                key: "case is missing its driver reference, which is required \
                      when the manifest declares several [[driver]] blocks"
                    .to_owned(),
            }),
        }
    };

    let mut cases = Vec::new();
    // The runner owns the defaults_or_tuned label: a manifest cannot declare
    // it, only the defaults the label is computed from.
    if manifest.defaults.contains_key("defaults_or_tuned") {
        return Err(ManifestError::UnknownKey {
            subject: manifest.name.clone(),
            key: "defaults.defaults_or_tuned — the label is computed, not declared".to_owned(),
        });
    }
    for decl in &manifest.cases {
        if decl.tags.contains_key("defaults_or_tuned") {
            return Err(ManifestError::UnknownKey {
                subject: manifest.name.clone(),
                key: "defaults_or_tuned: the runner labels each case from the manifest's [defaults]; declare [defaults] instead".to_owned(),
            });
        }
        if decl.matrix.contains_key("defaults_or_tuned") {
            return Err(ManifestError::UnknownKey {
                subject: manifest.name.clone(),
                key: "defaults_or_tuned: the runner labels each case from the manifest's [defaults]; declare [defaults] instead".to_owned(),
            });
        }
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
            // defaults_or_tuned is computed here, once, at load: default when
            // every declared default matches this case's tags, tuned when any
            // differs. It flows onto points like any identity tag.
            let label = if defaults
                .iter()
                .all(|(key, default)| tags.get(key.as_str()) == Some(default))
            {
                "default"
            } else {
                "tuned"
            };
            tags.insert(
                TagKey::Borrowed("defaults_or_tuned"),
                TagValue::Word(label.to_owned()),
            );
            let driver_name = driver_for(decl)?;
            let Some(driver) = manifest.drivers.iter().find(|d| d.name == driver_name) else {
                return Err(ManifestError::UnknownKey {
                    subject: manifest.name.clone(),
                    key: format!("case references unknown driver `{driver_name}`"),
                });
            };
            let adapter = crate::adapter::Adapter::parse(&driver.adapter).ok_or(
                ManifestError::UnknownAdapter {
                    subject: manifest.name.clone(),
                    found: driver.adapter.clone(),
                },
            )?;
            let mut case_keys: Vec<String> = ext
                .iter()
                .filter(|e| e.compile_time)
                .map(|e| e.key.clone())
                .collect();
            case_keys.extend(driver.compile_time.iter().cloned());
            // Sorted so generated source is byte-identical run to run.
            case_keys.sort();
            cases.push(Case {
                id: format!("{}:{}", manifest.name, cases.len()),
                tags,
                params: lower_params(&decl.params, &manifest.name)?,
                runners: decl.runners.clone(),
                adapter,
                subject: manifest.name.clone(),
                driver_crate: driver.crate_name.clone(),
                driver_macro: driver.macro_name.clone(),
                driver_lang: driver.lang.clone(),
                driver_entry: driver.entry.clone(),
                driver: driver_name,
                compile_time_keys: case_keys,
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
    let value = match raw {
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
    }?;
    ensure_selectable(key, &value, subject)?;
    Ok(value)
}

/// Every tag value is selector-addressable, so it must be expressible in the
/// selector language and mean itself when parsed back. `a|b` would split into
/// an OR, `a,b` into two clauses, `a..b` into a range, `*` into "key exists" —
/// all silently wrong in a chart, so they are load errors rather than values
/// a selector cannot say (the closed-vocabulary rule applied to values,
/// not just keys).
fn ensure_selectable(key: &str, value: &TagValue, subject: &str) -> Result<(), ManifestError> {
    let expr = value.to_string();
    let broken = |why: &str| {
        Err(ManifestError::BadSelectorValue {
            subject: subject.to_owned(),
            key: key.to_owned(),
            value: expr.clone(),
            why: why.to_owned(),
        })
    };

    if let TagValue::Word(word) = value {
        if word.contains([',', '|']) || word.contains("..") || word == "*" {
            return broken(
                "the selector language cannot express it: it contains `,`, `|`, \
                 `..`, or reads as the wildcard `*`",
            );
        }
    }

    // Whatever the shape, it must survive a round trip through the selector:
    // parse `key=<value>` and require the clause to admit exactly this value.
    // Catches anything that parses back as a different type or splits into
    // pieces the explicit checks above missed.
    let parsed = match crate::selector::Selector::parse(&format!("{key}={expr}")) {
        Ok(parsed) => parsed,
        Err(_) => return broken("cannot be written into a selector clause"),
    };
    let mut probe = TagMap::new();
    probe.insert(crate::tag::TagKey::Owned(key.to_owned()), value.clone());
    if !parsed.admits_point(&probe) {
        return broken("does not mean itself when parsed back out of a selector");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::selector::SelectorSet;

    fn subjects_dir() -> std::path::PathBuf {
        crate::test_support::subjects_dir()
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

    /// A shared fixture writer: one minimal valid manifest per name, with a
    /// source kind and any extra TOML (vocab tables, extra case tags).
    fn write_fixture(dir: &Path, name: &str, source_kind: &str, extra: &str) {
        let subject_dir = dir.join(name);
        std::fs::create_dir_all(&subject_dir).unwrap();
        std::fs::write(
            subject_dir.join("subject.toml"),
            format!(
                "schema = 1\n\
                 name = \"{name}\"\n\
                 domain = \"spatial_index\"\n\
                 [source]\n\
                 kind = \"{source_kind}\"\n\
                 repo = \"https://example.com/{name}\"\n\
                 pinned_ref = \"v1\"\n\
                 [[driver]]\n\
                 name = \"default\"\n\
                 adapter = \"rust-codegen\"\n\
                 crate = \"{name}-driver\"\n\
                 macro = \"bench_case\"\n\
                 {extra}\
                 [[case]]\n\
                 runners = [\"criterion\"]\n\
                 tags.impl = \"{name}\"\n"
            ),
        )
        .unwrap();
    }

    /// a `[source] kind` the engine does not build is a load error naming
    /// the subject — the same closed-vocabulary rule as the adapter, so a
    /// typo is not discovered as "not implemented" minutes into a run.
    #[test]
    fn an_unknown_source_kind_is_rejected_at_load() {
        let tmp = std::env::temp_dir().join(format!("sb-load-kind-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        write_fixture(&tmp, "odd", "cvs", "");
        match load_dir(&tmp) {
            Err(ManifestError::UnknownSourceKind { subject, found }) => {
                assert_eq!(subject, "odd");
                assert_eq!(found, "cvs");
            }
            Ok(_) => panic!("expected a source-kind rejection, got a loaded catalog"),
            Err(other) => panic!("expected a source-kind rejection, got {other:?}"),
        }
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// a value the selector language cannot express is a load error —
    /// `a|b` would split into an OR and silently select the wrong things.
    /// Declared vocabulary is checked too, since it is what the picker offers.
    #[test]
    fn selector_unexpressible_values_are_rejected_at_load() {
        let tmp = std::env::temp_dir().join(format!("sb-load-sel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        write_fixture(
            &tmp,
            "odd",
            "cargo-git",
            "[vocab]\nstorage.values = [\"heap\", \"a|b\"]\n",
        );
        match load_dir(&tmp) {
            Err(ManifestError::BadSelectorValue { key, value, .. }) => {
                assert_eq!(key, "odd.storage");
                assert_eq!(value, "a|b");
            }
            Ok(_) => panic!("expected a selector-value rejection, got a loaded catalog"),
            Err(other) => panic!("expected a selector-value rejection, got {other:?}"),
        }
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// a vocabulary violation names the remedy — a new `impl` value is an
    /// engine change in `UNIVERSAL`, not a manifest bug.
    #[test]
    fn a_vocabulary_violation_names_the_remedy() {
        let tmp = std::env::temp_dir().join(format!("sb-load-vocab-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        write_fixture(&tmp, "newsptree", "cargo-git", "");
        match load_dir(&tmp) {
            Err(ManifestError::Vocab { subject, message }) => {
                assert_eq!(subject, "newsptree");
                assert!(
                    message.contains("UNIVERSAL"),
                    "the remedy must be named: {message}"
                );
            }
            Ok(_) => panic!("expected a vocabulary rejection, got a loaded catalog"),
            Err(other) => panic!("expected a vocabulary rejection, got {other:?}"),
        }
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// A manifest naming an adapter this engine does not know is a load error
    /// naming the subject — not a run-time surprise. The closed-vocabulary
    /// rule (§6) applies to the manifest itself.
    #[test]
    fn an_unknown_adapter_is_rejected_at_load() {
        let tmp = std::env::temp_dir().join(format!("sb-load-adapter-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let dir = tmp.join("weird");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("subject.toml"),
            "schema = 1\n\
             name = \"weird\"\n\
             domain = \"spatial_index\"\n\
             [source]\n\
             kind = \"cargo-git\"\n\
             repo = \"https://example.com/weird\"\n\
             pinned_ref = \"v1\"\n\
             [[driver]]\n\
             name = \"default\"\n\
             adapter = \"rust-coden\"\n\
             crate = \"weird-driver\"\n\
             [[case]]\n\
             runners = [\"criterion\"]\n\
             tags.impl = \"weird\"\n",
        )
        .unwrap();
        match load_dir(&tmp) {
            Err(ManifestError::UnknownAdapter { subject, found }) => {
                assert_eq!(subject, "weird");
                assert_eq!(found, "rust-coden");
            }
            Ok(_) => panic!("expected an adapter rejection, got a loaded catalog"),
            Err(other) => panic!("expected an adapter rejection, got {other:?}"),
        }
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// A two-phase driver without a macro cannot generate anything; caught
    /// where the manifest is reviewed, not minutes into a run.
    #[test]
    fn a_codegen_driver_without_a_macro_is_rejected_at_load() {
        let tmp = std::env::temp_dir().join(format!("sb-load-macro-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let dir = tmp.join("silent");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("subject.toml"),
            "schema = 1\n\
             name = \"silent\"\n\
             domain = \"spatial_index\"\n\
             [source]\n\
             kind = \"cargo-git\"\n\
             repo = \"https://example.com/silent\"\n\
             pinned_ref = \"v1\"\n\
             [[driver]]\n\
             name = \"default\"\n\
             adapter = \"rust-codegen\"\n\
             crate = \"silent-driver\"\n\
             [[case]]\n\
             runners = [\"criterion\"]\n\
             tags.impl = \"silent\"\n",
        )
        .unwrap();
        match load_dir(&tmp) {
            Err(ManifestError::UnknownKey { subject, key }) => {
                assert_eq!(subject, "silent");
                assert!(key.contains("driver.macro"), "{key}");
            }
            Ok(_) => panic!("expected a macro rejection, got a loaded catalog"),
            Err(other) => panic!("expected a macro rejection, got {other:?}"),
        }
        std::fs::remove_dir_all(&tmp).ok();
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
        // for_subject is the unfiltered per-subject view: 109 v6-driver cases
        // plus 14 v5-driver cases, which the pin's driver selection filters
        // out of matching().
        assert_eq!(catalog.for_subject("kiddo").count(), 123);
        assert_eq!(catalog.for_subject("nanoflann").count(), 8);
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
        assert!(
            !vocab.knows("pykdtree.stem"),
            "one subject's extension must not leak into another's namespace"
        );
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
        assert_eq!(
            all.len(),
            109 + 8 + 12,
            "one point per case at default params"
        );
        // Only the original exact_nn cases have the default query_count of
        // 1000; the within/nnw/bnw cases use 100 by design.
        let exact_nn_count = all
            .iter()
            .filter(|(_, tags)| {
                tags.get("query").map(|v| v.to_string()) == Some("exact_nn".to_owned())
            })
            .count();
        assert_eq!(exact_nn_count, 108, "108 exact_nn points at default params");
        for (_, tags) in all.iter().filter(|(_, tags)| {
            tags.get("query").map(|v| v.to_string()) == Some("exact_nn".to_owned())
        }) {
            assert_eq!(tags.get("tree_size"), Some(&TagValue::Int(1 << 20)));
            assert_eq!(tags.get("query_count"), Some(&TagValue::Int(1000)));
        }
    }

    /// …and a constrained one sweeps exactly the constrained values.
    #[test]
    fn constrained_params_sweep() {
        let catalog = catalog();
        let sel = SelectorSet::parse_all([
            "impl=kiddo,k=1,kiddo.stem=eytzinger,tree_size=2^20|2^23|2^26",
        ])
        .unwrap();
        let points = catalog.points(&sel);
        assert_eq!(points.len(), 8 * 3, "8 monomorphisations x 3 sizes at k=1");
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
            "impl=kiddo,k=1,axis=f64,kiddo.stem=eytzinger,tree_size=2^16..2^19",
        ])
        .unwrap();
        let points = catalog.points(&sel);
        assert_eq!(
            points.len(),
            4 * 4,
            "four cases over 2^16, 2^17, 2^18, 2^19"
        );
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
        assert_eq!(subjects.len(), 3, "core vocabulary should span subjects");
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
        assert_eq!(subjects, ["kiddo".to_string()].into_iter().collect());
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
