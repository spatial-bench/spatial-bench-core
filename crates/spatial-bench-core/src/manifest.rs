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
    /// How this subject's driver is built and run. A subject may declare
    /// several, each scoped to a semver range of the library under test: an
    /// API change across ranges gets a new driver rather than a broken one.
    /// The engine selects the driver whose range contains the pinned
    /// version. The engine owns the harness, so this describes the engine's
    /// own driver crates, not anything belonging to the library under test.
    #[serde(default, rename = "driver")]
    pub drivers: Vec<Driver>,
    /// Extension keys, namespaced under the subject on use (`kiddo.stem`).
    #[serde(default)]
    pub vocab: BTreeMap<String, VocabKey>,
    /// The out-of-the-box configuration of the pinned library version: the
    /// tag values its own quick-start entry point produces. The runner
    /// labels each case `defaults_or_tuned = default` when every key here
    /// matches the case's tags, `tuned` when any differs. The label is
    /// computed, never declared: a manifest that sets it fails to load.
    #[serde(default)]
    pub defaults: BTreeMap<String, toml::Value>,
    #[serde(default, rename = "case")]
    pub cases: Vec<CaseDecl>,
}

/// Where the subject comes from. The engine builds it, so it pins it.
#[derive(Clone, Debug, Deserialize)]
pub struct Source {
    pub kind: String,
    pub repo: Option<String>,
    /// The distribution name on PyPI, for `kind = "pypi"` subjects.
    pub package: Option<String>,
    /// Mandatory: results are only comparable over time if the exact built
    /// revision is recorded. See the design's pinning-and-provenance section.
    pub pinned_ref: String,
    /// The exact commit the ref must resolve to . Git tags are mutable: a
    /// moved tag would otherwise compile different code under the same ref
    /// name, detectable only after the fact in the run header. Declared here,
    /// the build refuses anything else — "detectable" becomes "refused".
    /// Optional: without it the build proceeds and the sha is recorded.
    pub sha: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Driver {
    /// Names the driver within the subject; cases reference it.
    pub name: String,
    /// Lowest library version this driver understands, inclusive. Absent:
    /// no lower bound.
    #[serde(default)]
    pub min_supported_semver: Option<String>,
    /// Highest library version this driver understands, exclusive. Absent:
    /// no upper bound.
    #[serde(default)]
    pub max_supported_semver: Option<String>,
    /// `exec` for a single-phase driver, `rust-codegen` for a two-phase one.
    pub adapter: String,
    /// Crate providing the driver — rust-codegen only: a rust driver is a
    /// crate; an exec driver is a program beside the manifest. Absent for
    /// exec subjects.
    #[serde(rename = "crate", default)]
    pub crate_name: Option<String>,
    /// Exec only: the driver's language — `cxx` or `python`. It
    /// decides how the entry is built and invoked; the harness contract it
    /// speaks is the same for every language.
    pub lang: Option<String>,
    /// Exec only: the driver's entry file, relative to this
    /// manifest's directory — `shim.cpp` for cxx, `driver.py` for python.
    pub entry: Option<String>,
    /// Where the driver's assets live, **relative to this manifest's
    /// directory** ( the catalog moved to the bencher repo, and every
    /// subject dir is self-contained — `driver = "driver"` for a rust
    /// codegen crate beside the manifest, `entry` files for exec subjects).
    /// When set and present, it wins over the engine's own crates and the
    /// registry fallback.
    pub path: Option<String>,
    /// Published version of that crate, used when the binary has no engine
    /// source tree to depend on by path (an installed `cargo install` build).
    /// Reviewed here — like the pin — so a driver-crate update is a deliberate
    /// engine change, and it feeds the build cache key.
    pub version: Option<String>,
    /// Cargo features the subject is built with.
    #[serde(default)]
    pub features: Vec<String>,
    /// RUSTFLAGS the driver needs. Part of the build cache key, since the same
    /// source compiled with different flags is a different binary.
    pub rustflags: Option<String>,
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

#[derive(Clone, Debug, Deserialize)]
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
    /// Include directories, relative to the library source root — where the
    /// headers live after the pin is fetched (nanoflann: `include`).
    #[serde(default)]
    pub include: Vec<String>,
    /// Optional source build performed after the pinned git checkout is
    /// fetched. This keeps the subject itself out of the host package manager:
    /// a manual workflow run can change `pinned_ref` and rebuild that exact
    /// source revision.
    #[serde(default)]
    pub cmake: Option<CmakeBuild>,
}

/// A deliberately small CMake recipe for a C++ subject source tree.
#[derive(Clone, Debug, Deserialize)]
pub struct CmakeBuild {
    /// Source directory relative to the fetched repository; defaults to its root.
    pub source_dir: Option<PathBuf>,
    /// Extra arguments passed to CMake's configure step.
    #[serde(default)]
    pub configure: Vec<String>,
    /// Optional CMake target to build. With no target, builds the default set.
    pub target: Option<String>,
    /// Header directories relative to the CMake build directory for the shim.
    #[serde(default)]
    pub include: Vec<String>,
    /// Libraries relative to the CMake build directory to pass directly to g++.
    #[serde(default)]
    pub libraries: Vec<String>,
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

    /// Expands this declaration into one case per combination — the sweep is
    /// data, replacing today's nested for-loops and duplicated bench files.
    #[serde(default)]
    pub matrix: BTreeMap<String, Vec<toml::Value>>,
    #[serde(default)]
    pub tags: BTreeMap<String, toml::Value>,
    #[serde(default)]
    pub params: BTreeMap<String, ParamDecl>,
    /// Which [[driver]] block this case runs under. Required once the
    /// manifest declares more than one driver; with a single driver it may
    /// be omitted.
    #[serde(default)]
    pub driver: Option<String>,
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
    /// A `[driver] adapter` this engine does not know. A load error, not a
    /// run-time surprise: the closed-vocabulary rule applies to the manifest
    /// itself (design §6), and an unknown adapter string would otherwise be a
    /// typo measured as a missing driver.
    UnknownAdapter {
        subject: String,
        found: String,
    },
    /// A `[source] kind` this engine does not build. Same closed-vocabulary
    /// rule as the adapter: a typo is a load error, not a run-time surprise.
    UnknownSourceKind {
        subject: String,
        found: String,
    },
    /// A case's tag violates the vocabulary — unknown key, or a value the key
    /// does not allow. The message names the remedy, because "not allowed"
    /// alone reads like a manifest bug when it is really an engine change.
    Vocab {
        subject: String,
        message: String,
    },
    /// A tag value the selector language cannot express, or that would not
    /// mean itself when parsed back — caught at load, not as a silent hole in
    /// a chart (design §6's closed-vocabulary rule applied to values, not
    /// just keys).
    BadSelectorValue {
        subject: String,
        key: String,
        value: String,
        why: String,
    },
    /// A declared `sha` that is not a full commit id. Anything shorter would
    /// silently weaken the pin to a prefix match .
    BadSha {
        subject: String,
        found: String,
    },
}
