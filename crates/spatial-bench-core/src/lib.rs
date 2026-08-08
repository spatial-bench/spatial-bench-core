//! Tag-addressed, library-agnostic spatial-index benchmarking: core types.
//!
//! Design: `docs/design.md`. This crate is a **skeleton** — the
//! types define the shape; bodies are `unimplemented!()` pending sign-off.
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
pub mod harness;
pub mod machine;
pub mod manifest;
pub mod picker;
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
