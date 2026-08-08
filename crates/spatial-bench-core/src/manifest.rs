//! Subject manifests — the on-disk `subjects/<name>/subject.toml` format.
//!
//! Every subject is vendored here, kiddo included, so this is the only way a
//! case enters the catalog. The format is data rather than a Rust trait
//! precisely because most subjects are unwilling or unable to participate: the
//! engine has to be able to describe nanoflann without nanoflann's cooperation.

use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::PathBuf;

pub const MANIFEST_SCHEMA: u32 = 1;

#[derive(Debug, Deserialize)]
pub struct Manifest {
    pub schema: u32,
    /// Subject name; must equal the `impl` tag of every case it declares.
    pub name: String,
    /// Which domain's core vocabulary this subject's cases are written against.
    pub domain: String,
    pub source: Source,
    /// Lowest rustc this subject builds with. The engine resolves a run's single
    /// toolchain as the highest floor among selected subjects, so a subject
    /// states a floor rather than pinning one.
    #[serde(default)]
    pub toolchain: Option<Toolchain>,
    #[serde(default)]
    pub build: Option<Build>,
    /// How this subject's driver is built and run. The engine owns the harness
    /// for every subject, so this describes the engine's own driver crate, not
    /// anything belonging to the library under test.
    pub driver: Driver,
    /// Extension keys, namespaced under the subject on use (`kiddo.stem`).
    #[serde(default)]
    pub vocab: BTreeMap<String, VocabKey>,
    #[serde(default, rename = "case")]
    pub cases: Vec<CaseDecl>,
}

/// Where the subject comes from. The engine builds it, so it pins it.
#[derive(Debug, Deserialize)]
pub struct Source {
    pub kind: String,
    pub repo: Option<String>,
    /// Mandatory: results are only comparable over time if the exact built
    /// revision is recorded. See the design's pinning-and-provenance section.
    pub pinned_ref: String,
}

#[derive(Debug, Deserialize)]
pub struct Driver {
    /// `exec` for a single-phase driver, `rust-codegen` for a two-phase one.
    pub adapter: String,
    /// Crate in the engine that provides the driver.
    #[serde(rename = "crate")]
    pub crate_name: String,
    /// Cargo features the subject is built with.
    #[serde(default)]
    pub features: Vec<String>,
    /// Two-phase only: the macro a generated `main.rs` invokes, once per
    /// selected compile-time combination.
    #[serde(rename = "macro")]
    pub macro_name: Option<String>,
    /// Core keys that are generic parameters *for this subject*.
    ///
    /// Extension keys carry their own `compile_time` marking, but a core key
    /// like `axis` or `dims` cannot: it is owned by the domain, and whether it
    /// is a generic parameter depends on the subject. kiddo monomorphises on
    /// scalar type and dimensionality; a subject that dispatches on them at run
    /// time would list neither.
    #[serde(default)]
    pub compile_time: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct Toolchain {
    pub min_rustc: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Build {
    pub kind: String,
    pub shim: Option<PathBuf>,
    pub std: Option<String>,
    #[serde(default)]
    pub flags: Vec<String>,
    /// Some libraries template their index on dimensionality, so the shim needs
    /// one specialisation per value and a runtime dispatch.
    #[serde(default)]
    pub compile_time_dims: Vec<u32>,
}

#[derive(Debug, Deserialize)]
pub struct VocabKey {
    #[serde(default)]
    pub values: Option<Vec<String>>,
    #[serde(default)]
    pub kind: Option<String>,
    /// A generic parameter: each value is a separate monomorphisation, so it
    /// drives two-phase code generation. Unmarked keys are runtime arguments
    /// and cost nothing to add to a sweep.
    #[serde(default)]
    pub compile_time: bool,
}

#[derive(Debug, Deserialize)]
pub struct CaseDecl {
    #[serde(default)]
    pub runners: Vec<String>,
    /// Templated against resolved tags. Single-phase drivers only; a two-phase
    /// driver receives its parameters through the generated source instead.
    #[serde(default)]
    pub command: Vec<String>,
    pub output: Option<String>,

    /// Expands this declaration into one case per combination — the sweep is
    /// data, replacing today's nested for-loops and duplicated bench files.
    #[serde(default)]
    pub matrix: BTreeMap<String, Vec<toml::Value>>,
    #[serde(default)]
    pub tags: BTreeMap<String, toml::Value>,
    #[serde(default)]
    pub params: BTreeMap<String, ParamDecl>,
}

#[derive(Debug, Deserialize)]
pub struct ParamDecl {
    /// `"2^16..2^25"`
    pub range: Option<String>,
    pub kind: Option<String>,
    pub default: Option<toml::Value>,
    #[serde(default)]
    pub values: Vec<toml::Value>,
}

#[derive(Debug)]
pub enum ManifestError {
    Io {
        path: PathBuf,
        err: String,
    },
    Toml {
        path: PathBuf,
        err: String,
    },
    /// Refuses to guess at a format it does not know.
    UnsupportedSchema {
        path: PathBuf,
        found: u32,
    },
    /// A case whose `impl` tag disagrees with the manifest name — that would let
    /// one subject declare cases attributed to another.
    ImplMismatch {
        subject: String,
        found: String,
    },
    UnknownKey {
        subject: String,
        key: String,
    },
    /// An extension key not namespaced under its own subject.
    BadNamespace {
        subject: String,
        key: String,
    },
    MissingPin {
        subject: String,
    },
    /// A domain no build of the engine knows.
    UnknownDomain {
        subject: String,
        domain: String,
    },
    /// An explicit --rustc below some selected subject's floor. Refused rather
    /// than dropping the subject: a comparison quietly missing a contender is
    /// worse than one that will not start.
    ToolchainBelowFloor {
        subject: String,
        floor: String,
        pinned: String,
    },
}
