//! The tag vocabulary, in three tiers.
//!
//! **Universal** keys are true of anything measurable and exist in every domain.
//!
//! **Domain core** keys are engine-owned and shared by every subject declaring
//! that domain. This is what makes a cross-library chart possible: two subjects
//! can only be compared on an axis they both name identically, and the domain
//! core is what guarantees they do. A future domain declares its own core rather
//! than being forced through `spatial_index`'s keys.
//!
//! **Extension** keys are declared by a subject manifest and namespaced under
//! its name — `kiddo.stem`, `nanoflann.leaf_max_size`. Two libraries will both
//! want a key called `layout` and mean different things by it; namespacing keeps
//! them apart, so grouping by a core key never silently mixes them.
//!
//! Both tiers are closed: unknown keys are rejected when a manifest loads. An
//! open vocabulary is how the current suite ended up with four spellings of
//! tree size.

use crate::tag::TagError;

/// What kind of values a key accepts, and whether it is fixed or swept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Part of a case's fixed identity.
    Identity,
    /// An axis a case is swept over; carries a domain and a default.
    Param,
}

#[derive(Clone, Copy, Debug)]
pub struct KeyDef {
    pub key: &'static str,
    pub kind: Kind,
    /// `None` = any value of the right shape (integers, floats).
    pub allowed: Option<&'static [&'static str]>,
    /// Render powers of two as `2^N` in selector expressions. Only affects how
    /// a repro line reads; both forms parse to the same value.
    pub pow2: bool,
    pub doc: &'static str,
}

/// A problem shape. Each owns a core vocabulary; adding one is an engine
/// change, reviewed once, that cannot disturb an existing domain's data.
#[derive(Clone, Copy, Debug)]
pub struct Domain {
    pub name: &'static str,
    pub identity: &'static [KeyDef],
    pub params: &'static [KeyDef],
}

/// Every domain the engine knows.
pub const DOMAINS: &[Domain] = &[Domain {
    name: "spatial_index",
    identity: IDENTITY,
    params: PARAMS,
}];

pub fn domain(name: &str) -> Option<&'static Domain> {
    DOMAINS.iter().find(|d| d.name == name)
}

/// Universal keys — true of anything measurable, so they belong to every
/// domain rather than to any one of them.
pub const UNIVERSAL: &[KeyDef] = &[
    KeyDef {
        key: "impl",
        kind: Kind::Identity,
        allowed: Some(&[
            "kiddo_v6",
            "kiddo_v5",
            "nanoflann",
            "pykdtree",
            "pkdtree",
            "alglib",
            "skdtree",
            "scipy",
            "nearestneighbors_jl",
        ]),
        pow2: false,
        doc: "implementation under test",
    },
    KeyDef {
        key: "query",
        kind: Kind::Identity,
        allowed: Some(&[
            "exact_nn",
            "approx_nn",
            "within_radius",
            "within_unsorted",
            "nearest_n_within",
            "best_n_within",
            "best_n",
            "build",
            "add_points",
        ]),
        pow2: false,
        doc: "operation being measured",
    },
];

/// Core identity keys of the `spatial_index` domain.
pub const IDENTITY: &[KeyDef] = &[
    KeyDef {
        key: "k",
        kind: Kind::Identity,
        allowed: None,
        pow2: false,
        doc: "result count; 1 for nearest-one",
    },
    KeyDef {
        key: "dims",
        kind: Kind::Identity,
        allowed: None,
        pow2: false,
        doc: "dimensionality K",
    },
    KeyDef {
        key: "axis",
        kind: Kind::Identity,
        allowed: Some(&["f32", "f64"]),
        pow2: false,
        doc: "scalar type of the axes",
    },
    KeyDef {
        key: "idx",
        kind: Kind::Identity,
        allowed: Some(&["u16", "u32", "u64"]),
        pow2: false,
        doc: "index type",
    },
    KeyDef {
        key: "metric",
        kind: Kind::Identity,
        allowed: Some(&["squared_euclidean", "manhattan", "euclidean"]),
        pow2: false,
        doc: "distance metric",
    },
    KeyDef {
        key: "dataset",
        kind: Kind::Identity,
        allowed: Some(&["uniform", "gaussian"]),
        pow2: false,
        doc: "the distribution the construction and query points are drawn from",
    },
    KeyDef {
        key: "isa",
        kind: Kind::Identity,
        allowed: Some(&["scalar", "avx2", "avx512", "neon"]),
        pow2: false,
        doc: "instruction set actually exercised",
    },
    KeyDef {
        key: "parallelism",
        kind: Kind::Identity,
        allowed: Some(&["single_threaded", "multi_threaded"]),
        pow2: false,
        doc: "how many processor cores the implementation may use",
    },
    KeyDef {
        key: "query_batching",
        kind: Kind::Identity,
        allowed: Some(&["single_query", "batch_query"]),
        pow2: false,
        doc: "the shape of the measured operation: one query point through \n             the single-point API, or many through a bulk batch API",
    },
    KeyDef {
        key: "defaults_or_tuned",
        kind: Kind::Identity,
        allowed: Some(&["default", "tuned"]),
        pow2: false,
        doc: "runner-computed label: default when every configuration \n             parameter equals the manifest's declared defaults for this \n             library version, tuned when any differs. Manifests cannot \n             declare it — the runner derives it at load",
    },
];

/// Core param axes of the `spatial_index` domain. One name per concept — these
/// replace the ~40 `KIDDO_*` env vars.
pub const PARAMS: &[KeyDef] = &[
    KeyDef {
        key: "tree_size",
        kind: Kind::Param,
        allowed: None,
        pow2: true,
        doc: "point count; write as 2^N in selectors",
    },
    KeyDef {
        key: "query_count",
        kind: Kind::Param,
        allowed: None,
        pow2: false,
        doc: "queries per measurement",
    },
    KeyDef {
        key: "query_batch_size",
        kind: Kind::Param,
        allowed: None,
        pow2: false,
        doc: "query points per batch API call, for batch_query cases",
    },
    KeyDef {
        key: "radius",
        kind: Kind::Param,
        allowed: None,
        pow2: false,
        doc: "radius for within_* queries",
    },
    KeyDef {
        key: "epsilon",
        kind: Kind::Param,
        allowed: None,
        pow2: false,
        doc: "approx_nn error bound",
    },
    KeyDef {
        key: "threads",
        kind: Kind::Param,
        allowed: None,
        pow2: false,
        doc: "worker threads",
    },
];

/// Look a key up in the universal tier plus the `spatial_index` core.
///
/// Takes no domain argument yet because only one exists; it gains one when a
/// second domain lands, at which point this becomes `lookup_in(domain, key)`.
pub fn lookup(key: &str) -> Option<&'static KeyDef> {
    UNIVERSAL
        .iter()
        .chain(IDENTITY)
        .chain(PARAMS)
        .find(|d| d.key == key)
}

/// Resolve an identifier written in [`crate::tags!`] to its vocabulary key.
///
/// `impl` and `type` are Rust keywords, so the macro accepts a trailing
/// underscore (`impl_`) and it is stripped here.
pub fn key_str(ident: &str) -> &'static str {
    let trimmed = ident.strip_suffix('_').unwrap_or(ident);
    match lookup(trimmed) {
        Some(def) => def.key,
        None => panic!("unknown core tag key `{ident}` — add it here, or declare it as a\n             namespaced subject extension in subjects/<name>/subject.toml"),
    }
}

/// Validate a core key/value pair. Extension keys are checked by
/// [`Vocabulary::validate`], which knows what each subject declared.
pub fn validate(key: &str, value: &crate::tag::TagValue) -> Result<(), TagError> {
    let Some(def) = lookup(key) else {
        return Err(TagError::UnknownKey(key.to_owned()));
    };
    if let Some(allowed) = def.allowed {
        let rendered = value.to_string();
        if !allowed.contains(&rendered.as_str()) {
            return Err(TagError::NotAllowed {
                key: key.to_owned(),
                value: rendered,
                allowed: allowed.iter().map(|s| s.to_string()).collect(),
            });
        }
    }
    Ok(())
}

/// An extension key contributed by a subject manifest, e.g. `kiddo.stem`.
#[derive(Clone, Debug)]
pub struct ExtKey {
    /// Owning subject, e.g. `kiddo_v6`.
    pub subject: String,
    /// Namespaced key as written in tags, e.g. `kiddo.stem`.
    pub key: String,
    pub allowed: Option<Vec<String>>,
    /// A generic parameter of its subject; drives two-phase code generation.
    pub compile_time: bool,
}

/// Core vocabulary plus every extension contributed by loaded manifests.
#[derive(Clone, Debug, Default)]
pub struct Vocabulary {
    pub extensions: Vec<ExtKey>,
}

impl Vocabulary {
    /// Is this key known — core, or an extension of some loaded subject?
    pub fn knows(&self, key: &str) -> bool {
        lookup(key).is_some() || self.extensions.iter().any(|e| e.key == key)
    }

    /// Register a subject's extension keys.
    ///
    /// Rejects an extension that shadows a core key, or that is not namespaced
    /// under its own subject — both would defeat the point of the split.
    pub fn extend(&mut self, subject: &str, keys: Vec<ExtKey>) -> Result<(), TagError> {
        let prefix = format!("{}.", namespace_of(subject));
        for key in &keys {
            if lookup(&key.key).is_some() {
                return Err(TagError::BadExtension {
                    key: key.key.clone(),
                    reason: format!("shadows the core key `{}`", key.key),
                });
            }
            if !key.key.starts_with(&prefix) {
                return Err(TagError::BadExtension {
                    key: key.key.clone(),
                    reason: format!("must be namespaced under `{prefix}`"),
                });
            }
        }
        self.extensions.extend(keys);
        Ok(())
    }

    /// Validate a key/value pair against core plus loaded extensions.
    pub fn validate(&self, key: &str, value: &crate::tag::TagValue) -> Result<(), TagError> {
        if lookup(key).is_some() {
            return validate(key, value);
        }
        let Some(ext) = self.extensions.iter().find(|e| e.key == key) else {
            return Err(TagError::UnknownKey(key.to_owned()));
        };
        if let Some(allowed) = &ext.allowed {
            let rendered = value.to_string();
            if !allowed.iter().any(|a| a == &rendered) {
                return Err(TagError::NotAllowed {
                    key: key.to_owned(),
                    value: rendered,
                    allowed: allowed.clone(),
                });
            }
        }
        Ok(())
    }
}

/// The namespace a subject's extension keys must sit under.
///
/// A subject named `kiddo_v6` namespaces as `kiddo.`: the version belongs in
/// the `impl` tag and the pinned ref, not baked into every extension key, or
/// `kiddo.stem` and `kiddo_v7.stem` would read as unrelated axes and split a
/// chart that should be continuous.
pub fn namespace_of(subject: &str) -> &str {
    subject.split_once('_').map_or(subject, |(head, tail)| {
        if tail.starts_with('v') && tail[1..].chars().all(|c| c.is_ascii_digit()) {
            head
        } else {
            subject
        }
    })
}
