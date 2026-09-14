//! Source resolution shared by the run pipeline and `conform` (§13): where a
//! subject's *source* and its *driver crate* come from for a given build.
//!
//! One implementation, two callers — when these helpers were mirrored in
//! `run` and `conform`, the mirrors drifted the first time resolution gained
//! a new branch ('s manifest-relative drivers), and `conform` started
//! refusing a subject the run path served happily.
//!
//! Resolution order for a driver crate: a manifest-relative path (—
//! the bencher layout, where driver assets live beside the manifest), then an
//! engine-owned crate under the engine checkout (transitional), then the
//! published version the manifest pins. For a subject's source: a
//! `--subject-path` working tree, else the manifest's pin.

use crate::build::{DriverSource, SubjectSource};
use crate::catalog::Catalog;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// What resolution needs. Both callers hold these; both pass the same fields.
pub struct Inputs<'a> {
    pub catalog: &'a Catalog,
    pub engine_root: Option<&'a PathBuf>,
    pub subject_paths: &'a BTreeMap<String, PathBuf>,
}

/// Where the driver crate for `subject` comes from, and the cache-lane
/// identity for it. The cache key covers which branch resolved, so a
/// bencher-path build and an engine-path build of the same source never share
/// a directory.
pub fn driver_source(
    inputs: &Inputs<'_>,
    subject: &str,
    driver_crate: &str,
) -> Result<(DriverSource, String), String> {
    if let Some(driver_path) = inputs.catalog.manifest_driver_path(subject) {
        let rev = match inputs.engine_root {
            // The driver crate's version deps on core + measure are patched to
            // the engine checkout when one is reachable — part of what is
            // built, so part of the cache key.
            Some(root) => format!("path:{};engine:{}", driver_path.display(), root.display()),
            None => format!("path:{}", driver_path.display()),
        };
        return Ok((DriverSource::Path(driver_path), rev));
    }
    if let Some(root) = inputs.engine_root {
        // Transitional: engine-owned driver crates (the pre-bencher layout).
        let path = root.join("crates").join(driver_crate);
        if path.is_dir() {
            return Ok((DriverSource::Path(path), format!("path:{}", root.display())));
        }
    }
    let Some(version) = inputs.catalog.driver_version(subject) else {
        return Err(format!(
            "no driver at the path {subject}'s manifest declares, no engine \
             source tree to build {driver_crate} from, and no published driver \
             version.\n\
             Point at a checkout:  --engine-src /path/to/spatial-bench",
        ));
    };
    let driver_rev = format!("registry:{version}");
    Ok((DriverSource::Registry { version }, driver_rev))
}

/// Where a subject's source comes from for this run: a `--subject-path`
/// working tree if given, else the manifest's pin — which is the default and
/// the only path a submittable run can take.
pub fn subject_source(inputs: &Inputs<'_>, subject: &str) -> Result<SubjectSource, String> {
    if let Some(path) = inputs.subject_paths.get(subject) {
        return Ok(SubjectSource::Path(path.clone()));
    }
    let (kind, repo, pin) = inputs.catalog.source(subject).ok_or_else(|| {
        format!("the catalog has no source for {subject}; the manifest is missing")
    })?;
    match kind.as_str() {
        "cargo-git" => {
            let repo = repo
                .ok_or_else(|| format!("{subject} declares source kind cargo-git but no repo"))?;
            Ok(SubjectSource::Git {
                repo,
                reference: pin,
            })
        }
        "cargo" => Ok(SubjectSource::Registry {
            version: pin,
            package: inputs
                .catalog
                .source_full(subject)
                .ok()
                .and_then(|source| source.package),
        }),
        other => Err(format!(
            "building {subject} from source kind `{other}` is not implemented \
             yet (it needs the engine's build recipe, not cargo)",
        )),
    }
}

/// The subject's revision for the cache key: its pin, or "worktree:<path>"
/// for a `--subject-path` build.
pub fn subject_rev(inputs: &Inputs<'_>, subject: &str) -> String {
    match inputs.subject_paths.get(subject) {
        // A working tree has no revision. Naming it as such keeps two builds
        // from sharing a cache key across an edit.
        Some(path) => format!("worktree:{}", path.display()),
        None => inputs.catalog.pinned_ref(subject).unwrap_or_default(),
    }
}

/// Assemble a subject's build request for runs and conformance checks.
/// Callers resolve Rust flags according to their selection policy.
pub(crate) fn subject_request(
    inputs: &Inputs<'_>,
    subject: &str,
    first_case: &crate::case::Case,
    build_root: &std::path::Path,
    toolchain: crate::toolchain::Version,
    rustflags: Option<String>,
) -> Result<crate::adapter::SubjectRequest, String> {
    let exec_inputs = match first_case.adapter {
        // an exec subject's driver is built by the language builder,
        // from the manifest's own recipe and pin. The builder verifies the
        // library sha itself, so the run-level sha enforcement is skipped.
        crate::adapter::Adapter::Exec => {
            let build = inputs.catalog.build(subject).ok_or_else(|| {
                format!("{subject} declares the exec adapter but no build recipe")
            })?;
            let entry = inputs
                .catalog
                .manifest_dir(subject)
                .ok_or_else(|| format!("{subject} has no manifest directory"))?
                .join(
                    first_case
                        .driver_entry
                        .as_deref()
                        .ok_or_else(|| format!("{subject} declares no driver entry"))?,
                );
            Some(crate::exec::ExecInputs {
                manifest_dir: inputs
                    .catalog
                    .manifest_dir(subject)
                    .ok_or_else(|| format!("{subject} has no manifest directory"))?,
                lang: first_case
                    .driver_lang
                    .clone()
                    .ok_or_else(|| format!("{subject} declares no driver language"))?,
                entry,
                build,
                source: inputs.catalog.source_full(subject)?,
                expected_sha: inputs.catalog.expected_sha(subject),
            })
        }
        _ => None,
    };
    let codegen = match first_case.adapter {
        crate::adapter::Adapter::RustCodegen => {
            let driver_crate = first_case
                .driver_crate
                .clone()
                .ok_or_else(|| format!("{subject} declares no driver crate"))?;
            let (driver_source, driver_rev) = driver_source(inputs, subject, &driver_crate)?;
            let subject_source = subject_source(inputs, subject)?;
            let subject_rev = subject_rev(inputs, subject);
            Some(crate::adapter::CodegenInputs {
                driver_crate,
                driver_macro: first_case
                    .driver_macro
                    .clone()
                    .ok_or_else(|| format!("{subject} declares no driver macro"))?,
                driver_source,
                driver_rev,
                subject_crate: crate::vocab::namespace_of(subject).to_owned(),
                subject_source,
                subject_rev,
                features: inputs.catalog.features(subject),
                rustflags,
            })
        }
        _ => None,
    };
    Ok(crate::adapter::SubjectRequest {
        subject: subject.to_owned(),
        adapter: first_case.adapter,
        build_root: build_root.to_path_buf(),
        toolchain,
        engine_root: inputs.engine_root.cloned(),
        exec: exec_inputs,
        codegen,
    })
}
