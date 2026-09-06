//! Materialising and building a generated driver.
//!
//! The generated `main.rs` needs a package around it: one that depends on the
//! engine's driver crate and on the library under test. The driver crate itself
//! carries no dependency on any subject — every reference to one lives inside a
//! macro body and expands here — which is what lets a build be pointed at a
//! pinned ref or a working tree without anything hardcoding a path.

use crate::codegen::Generated;
use std::path::{Path, PathBuf};

/// Where a subject's source comes from for one build.
#[derive(Clone, Debug)]
pub enum SubjectSource {
    /// A working tree. Results are marked as such: they record a checkout, not
    /// a revision, so the dataset can reject them.
    Path(PathBuf),
    Git {
        repo: String,
        reference: String,
    },
}

/// Where the engine's driver crate comes from for one build.
///
/// Running from a source checkout (or with one named via `--engine-src`):
/// depend by path, so driver-crate edits are picked up on the next run — cargo
/// tracks the path dep's mtime inside the build dir. An installed binary with
/// no reachable tree: depend on the published crate, which is what makes
/// `cargo install` self-contained once the driver crates are published.
#[derive(Clone, Debug)]
pub enum DriverSource {
    Path(PathBuf),
    Registry { version: String },
}

#[derive(Clone, Debug)]
pub struct BuildRequest {
    pub subject: String,
    /// Engine crate providing the macro and harness runtime.
    pub driver_crate: String,
    pub driver_source: DriverSource,
    /// The engine checkout to patch the driver crate's core/measure deps to
    /// ( bencher-resident driver crates carry version-only deps).
    /// `None` — no patch; registry versions stand.
    pub engine_root: Option<PathBuf>,
    pub subject_crate: String,
    pub subject_source: SubjectSource,
    pub features: Vec<String>,
    pub rustflags: Option<String>,
    pub generated: Generated,
}

/// Write the package for a generated driver and return its directory.
///
/// Keyed by the cache key, so an identical selection reuses a directory and
/// cargo's own incremental state. Nothing here is a source file: the whole tree
/// is engine output.
///
/// **Trusted-input boundary :** paths and manifest strings are
/// interpolated into the generated Cargo.toml as TOML values. The CLI
/// validates subject paths through [`toml_safe_path`]; manifest fields are
/// trusted engine content (§4's review model) and are not escaped further —
/// relax either assumption deliberately, not accidentally.
pub fn materialise(root: &Path, request: &BuildRequest) -> Result<PathBuf, BuildError> {
    let dir = root
        .join(&request.subject)
        .join(&request.generated.cache_key);
    std::fs::create_dir_all(dir.join("src")).map_err(|e| BuildError::Io {
        path: dir.clone(),
        err: e.to_string(),
    })?;

    let subject_dep = match &request.subject_source {
        SubjectSource::Path(p) => {
            format!(
                "{{ path = {:?}, features = {:?} }}",
                p.display().to_string(),
                request.features
            )
        }
        SubjectSource::Git { repo, reference } => {
            // A tag unless the pin is a full commit id: cargo's `rev` accepts
            // any committish, but naming a tag as a tag keeps Cargo.lock — and
            // therefore the provenance trail — saying what was meant.
            let pin = if is_commit_id(reference) {
                format!("rev = {reference:?}")
            } else {
                format!("tag = {reference:?}")
            };
            format!(
                "{{ git = {repo:?}, {pin}, features = {:?} }}",
                request.features
            )
        }
    };

    let driver_dep = match &request.driver_source {
        DriverSource::Path(p) => format!("{{ path = {:?} }}", p.display().to_string()),
        // A registry requirement, not a pin: the manifest names the version
        // (and it feeds the build cache key), so an update is a deliberate,
        // reviewable engine change.
        DriverSource::Registry { version } => format!("\"{version}\""),
    };

    // bencher-resident driver crates carry version-only deps on
    // core + measure; when an engine checkout is reachable, a patch table
    // redirects those to the local sources so a run tests the engine it was
    // built from. Without an engine checkout the registry versions stand.
    let patch_table = request
        .engine_root
        .as_ref()
        .filter(|root| {
            root.join("crates/spatial-bench-core").is_dir()
                && root.join("crates/spatial-bench-measure").is_dir()
        })
        .map(|root| {
            format!(
                "\n# The engine checkout this run was built from: the driver's version \
                 # deps on core + measure resolve here instead of the registry.\n\
                 [patch.crates-io]\n\
                 spatial-bench-core = {{ path = {:?} }}\n\
                 spatial-bench-measure = {{ path = {:?} }}\n",
                root.join("crates/spatial-bench-core").display().to_string(),
                root.join("crates/spatial-bench-measure")
                    .display()
                    .to_string()
            )
        })
        .unwrap_or_default();

    let manifest = format!(
        "# Generated by spatial-bench. Do not edit; this whole directory is output.\n\
         [package]\n\
         name = \"spatial-bench-generated\"\n\
         version = \"0.0.0\"\n\
         edition = \"2021\"\n\
         publish = false\n\n\
         [[bin]]\n\
         name = \"driver\"\n\
         path = \"src/main.rs\"\n\n\
         [dependencies]\n\
         {driver} = {driver_dep}\n\
         {subject} = {subject_dep}\n\n\
         [profile.release]\n\
         # Benchmarks are meaningless below the optimisation level they ship at.\n\
         opt-level = 3\n\
         debug = true\n\
\n\
         # Standalone, even if this directory lands inside a cargo workspace: a\n\
         # non-member package under a workspace root is a build error otherwise.\n\
         # Standalone, even if this directory lands inside a cargo workspace: a
         # non-member package under a workspace root is a build error otherwise.
         [workspace]
{patch_table}",
        driver = request.driver_crate,
        subject = request.subject_crate,
    );

    write_if_changed(&dir.join("Cargo.toml"), &manifest)?;
    write_if_changed(&dir.join("src/main.rs"), &request.generated.source)?;
    Ok(dir)
}

/// A 40+ hex-character reference is a commit id; anything else (v6.0.0-alpha.4,
/// main, a short sha) is a ref name.
pub fn is_commit_id(reference: &str) -> bool {
    reference.len() >= 40 && reference.chars().all(|c| c.is_ascii_hexdigit())
}

/// True when a path can be interpolated into a generated Cargo.toml without
/// corrupting it . The generated manifest is TOML built by string
/// formatting, so a quote, backslash or newline in a path would inject or
/// break keys. The CLI validates `--subject-path` values through this before
/// building; manifest strings are trusted engine content (§4) and are not
/// re-checked here.
pub fn toml_safe_path(path: &Path) -> bool {
    let s = path.to_string_lossy();
    !s.contains('"') && !s.contains('\\') && !s.contains('\n') && !s.contains('\r')
}

/// The version and git revision cargo actually built for `crate_name`, read
/// from the generated package's Cargo.lock.
///
/// This is §4's provenance made real: a git dependency's lockfile entry is
/// `git+<repo>?tag=<pin>#<sha>`, so the run header can record the exact
/// revision without the engine cloning anything itself.
pub fn built_subject(lock: &Path, crate_name: &str) -> Option<(String, Option<String>)> {
    let raw = std::fs::read_to_string(lock).ok()?;
    let value: toml::Value = toml::from_str(&raw).ok()?;
    for entry in value.get("package")?.as_array()? {
        if entry.get("name").and_then(|v| v.as_str()) != Some(crate_name) {
            continue;
        }
        let version = entry.get("version")?.as_str()?.to_owned();
        let sha = entry
            .get("source")
            .and_then(|v| v.as_str())
            .and_then(|s| s.rsplit_once('#'))
            .map(|(_, sha)| sha.to_owned());
        return Some((version, sha));
    }
    None
}

/// The ISA a selection pins is a compiler input: these are the rustc flags
/// each vocabulary value maps to. A value the host cannot honour fails the
/// build — an honest outcome, per the ISA vocabulary's contract.
pub fn isa_rustflags(isa: &str) -> Option<&'static str> {
    match isa {
        "native" => Some("-C target-cpu=native"),
        "avx2" => Some("-C target-cpu=x86-64-v3"),
        "avx512" => Some("-C target-cpu=x86-64-v4"),
        "sve" => Some("-C target-feature=+sve"),
        "neon" => Some("-C target-feature=+neon"),
        "scalar_only" => Some(""),
        _ => None,
    }
}

/// Compile a materialised driver package with the run's one toolchain,
/// honouring the subject's RUSTFLAGS (§4). Uses cargo's exit status and stderr
/// verbatim, so the operator sees the real compile error, not a paraphrase.
pub fn compile(
    dir: &Path,
    toolchain: &crate::toolchain::Version,
    rustflags: Option<&str>,
) -> Result<(), BuildError> {
    let mut cargo = crate::toolchain::cargo(toolchain);
    cargo
        .args(["build", "--release", "--bin", "driver"])
        .current_dir(dir);
    match rustflags {
        Some(flags) => {
            cargo.env("RUSTFLAGS", flags);
        }
        None => {
            cargo.env_remove("RUSTFLAGS");
        }
    }
    let out = cargo
        .output()
        .map_err(|e| BuildError::Spawn(e.to_string()))?;
    if !out.status.success() {
        return Err(BuildError::Cargo {
            status: out.status.code(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        });
    }
    Ok(())
}

/// Only touch a file whose content differs, so cargo's mtime-based staleness
/// check does not force a rebuild of an unchanged package.
fn write_if_changed(path: &Path, content: &str) -> Result<(), BuildError> {
    if let Ok(existing) = std::fs::read_to_string(path) {
        if existing == content {
            return Ok(());
        }
    }
    std::fs::write(path, content).map_err(|e| BuildError::Io {
        path: path.to_owned(),
        err: e.to_string(),
    })
}

#[derive(Debug)]
pub enum BuildError {
    Io { path: PathBuf, err: String },
    Cargo { status: Option<i32>, stderr: String },
    Spawn(String),
}

impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuildError::Io { path, err } => write!(f, "{}: {err}", path.display()),
            BuildError::Cargo { status, stderr } => {
                write!(f, "cargo exited with {status:?}\n{stderr}")
            }
            BuildError::Spawn(e) => write!(f, "could not run cargo: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::Generated;

    fn request(source: SubjectSource) -> BuildRequest {
        BuildRequest {
            subject: "kiddo_v6".into(),
            driver_crate: "spatial-bench-kiddo-v6".into(),
            driver_source: DriverSource::Path(PathBuf::from(
                "/engine/crates/spatial-bench-kiddo-v6",
            )),
            engine_root: Some(PathBuf::from("/engine")),
            subject_crate: "kiddo".into(),
            subject_source: source,
            features: vec!["test_utils".into()],
            rustflags: None,
            generated: Generated {
                source: "fn main() {}\n".into(),
                cache_key: "abc123".into(),
                combinations: 1,
            },
        }
    }

    #[test]
    fn writes_a_package_keyed_by_the_cache_key() {
        let tmp = std::env::temp_dir().join(format!("sb-build-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let dir = materialise(&tmp, &request(SubjectSource::Path("/w/kiddo".into()))).unwrap();

        assert!(dir.ends_with("kiddo_v6/abc123"));
        let manifest = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap();
        assert!(manifest.contains(r#"kiddo = { path = "/w/kiddo""#));
        assert!(
            manifest.contains("opt-level = 3"),
            "an unoptimised benchmark is meaningless"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("src/main.rs")).unwrap(),
            "fn main() {}\n"
        );
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn a_tag_pin_is_pinned_by_tag() {
        let tmp = std::env::temp_dir().join(format!("sb-build-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let dir = materialise(
            &tmp,
            &request(SubjectSource::Git {
                repo: "https://github.com/sdd/kiddo".into(),
                reference: "v6.0.0-alpha.4".into(),
            }),
        )
        .unwrap();
        let manifest = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap();
        assert!(manifest.contains(r#"tag = "v6.0.0-alpha.4""#));
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// A full commit sha pins by `rev`, because that is what it is.
    #[test]
    fn a_sha_pin_is_pinned_by_rev() {
        let tmp = std::env::temp_dir().join(format!("sb-build-sha-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let sha = "c299dde1".repeat(5); // 40 hex chars
        let dir = materialise(
            &tmp,
            &request(SubjectSource::Git {
                repo: "https://github.com/sdd/kiddo".into(),
                reference: sha.clone(),
            }),
        )
        .unwrap();
        let manifest = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap();
        assert!(manifest.contains(&format!("rev = \"{sha}\"")));
        assert!(!manifest.contains("tag ="));
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// §4 provenance: the lockfile records what cargo actually resolved and
    /// fetched, so the run header can quote it.
    #[test]
    fn built_subject_reads_version_and_sha_from_the_lock() {
        let tmp = std::env::temp_dir().join(format!("sb-build-lock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let lock = tmp.join("Cargo.lock");
        std::fs::write(
            &lock,
            "[[package]]\nname = \"other\"\nversion = \"1.0.0\"\n\n\
             [[package]]\nname = \"kiddo\"\nversion = \"6.0.0-alpha.4\"\n\
             source = \"git+https://github.com/sdd/kiddo?tag=v6.0.0-alpha.4#c299dde1234567890abcdef\"\n",
        )
        .unwrap();
        assert_eq!(
            built_subject(&lock, "kiddo"),
            Some((
                "6.0.0-alpha.4".to_owned(),
                Some("c299dde1234567890abcdef".to_owned())
            ))
        );
        // A path dependency has no source line; the version still records.
        assert_eq!(
            built_subject(&lock, "other"),
            Some(("1.0.0".to_owned(), None))
        );
        assert_eq!(built_subject(&lock, "absent"), None);
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// An installed binary with no reachable source tree depends on the
    /// published crate instead — the branch that makes `cargo install`
    /// self-contained once driver crates are published.
    #[test]
    fn a_registry_driver_depends_by_version() {
        let tmp = std::env::temp_dir().join(format!("sb-build-reg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let mut req = request(SubjectSource::Path("/w/kiddo".into()));
        req.driver_source = DriverSource::Registry {
            version: "0.1.0".into(),
        };
        let dir = materialise(&tmp, &req).unwrap();
        let manifest = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap();
        assert!(manifest.contains("spatial-bench-kiddo-v6 = \"0.1.0\""));
        assert!(!manifest.contains("path = \"/engine"));
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// Rewriting an identical file would bump its mtime and force cargo to
    /// rebuild a package that has not changed.
    #[test]
    fn an_unchanged_package_is_not_rewritten() {
        let tmp = std::env::temp_dir().join(format!("sb-build-idem-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let req = request(SubjectSource::Path("/w/kiddo".into()));
        let dir = materialise(&tmp, &req).unwrap();
        let before = std::fs::metadata(dir.join("src/main.rs"))
            .unwrap()
            .modified()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        materialise(&tmp, &req).unwrap();
        let after = std::fs::metadata(dir.join("src/main.rs"))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(before, after);
        std::fs::remove_dir_all(&tmp).ok();
    }
}
