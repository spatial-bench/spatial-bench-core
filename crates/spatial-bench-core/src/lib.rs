//! Tag-addressed, library-agnostic spatial-index benchmarking: core types.
//!
//! Design: `docs/design.md`.
//!
//! Two ideas carry the whole design:
//!
//! 1. A data point is identified by its **tag map** and nothing else, so no
//!    downstream tool ever parses a name to recover structure.
//! 2. Every subject is **vendored** — kiddo included — so no library can declare
//!    or alter how it is measured, and no library needs to cooperate to be
//!    measured at all.

pub mod adapter;
pub mod build;
pub mod case;
pub mod catalog;
pub mod catalog_load;
pub mod codegen;
pub mod conform;
pub mod context;
pub mod exec;
pub mod fingerprint;
pub mod harness;
pub mod machine;
pub mod manifest;
pub mod perf;
pub mod picker;
pub mod resolve;
pub mod run;
pub mod schema;
pub mod selector;
pub mod tag;
pub mod toolchain;
pub mod vocab;

pub use catalog::Catalog;
pub use selector::{Selector, SelectorSet};

/// Load every vendored subject manifest under `subjects/`.
///
/// Subjects are always vendored -- kiddo included -- so this is the only way
/// cases enter the catalog. See the design's "Subjects are always vendored".
pub fn load_subjects(dir: &std::path::Path) -> Result<Catalog, manifest::ManifestError> {
    catalog_load::load_dir(dir)
}

#[cfg(test)]
pub(crate) mod test_support {
    //! The catalog moved out of the engine on — it lives in the
    //! bencher checkout. Tests assert against the real catalog by design, so
    //! they discover it rather than carrying a copy: `SPATIAL_BENCH_BENCHERS`
    //! first, then the conventional sibling checkout beside the engine
    //! repository. Panics with setup instructions when neither exists.
    pub fn subjects_dir() -> std::path::PathBuf {
        if let Some(dir) = std::env::var_os("SPATIAL_BENCH_BENCHERS") {
            let dir = std::path::PathBuf::from(dir).join("subjects");
            assert!(
                dir.is_dir(),
                "SPATIAL_BENCH_BENCHERS points at {}: no subjects/ inside",
                dir.display()
            );
            return dir;
        }
        let engine_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("the engine root resolves");
        let sibling = engine_root
            .parent()
            .expect("the engine root has a parent")
            .join("spatial-bench-benchers")
            .join("subjects");
        assert!(
            sibling.is_dir(),
            "no bencher catalog found (looked at {}).  moved the \
             catalog out of the engine: clone spatial-bench-benchers beside \
             this checkout, or set SPATIAL_BENCH_BENCHERS.",
            sibling.display()
        );
        sibling
    }
}
