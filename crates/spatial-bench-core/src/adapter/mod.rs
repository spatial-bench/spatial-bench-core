//! Adapters turn (case, resolved params) into data points.
//!
//! The split exists so that a subject unwilling to cooperate is a first-class
//! citizen: `rust-codegen` generates, builds and runs a driver the engine
//! owns, `exec` drives any command at all. Adding a language means adding a
//! manifest, not engine code.
//!
//! [`Adapter::parse`], [`prepare`] and [`run`] are §5's dispatch point, shared
//! by `spatial-bench run` and `conform` (§13) so there is exactly one place
//! that knows what an adapter means. A known-but-unbuilt adapter is refused
//! here, naming the subject; an unknown kind is refused earlier, at manifest
//! load.
//!
//! The measurement itself lives in the `spatial-bench-measure` crate — the one
//! criterion routine every Rust driver is measured by — keeping this crate and
//! core free of criterion's dependency tree.

use crate::codegen::Generated;
use crate::schema::Point;
use std::path::PathBuf;

/// How a subject's driver runs: the typed form of a manifest's
/// `[driver] adapter` string.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Adapter {
    /// Two-phase (§5): the engine generates a `main.rs` of macro invocations,
    /// builds it against the subject, runs it once per sweep.
    RustCodegen,
    /// Single-phase: a command with templated args, one per point — what makes
    /// an unwilling, non-Rust subject measurable at all.
    Exec,
}

impl Adapter {
    /// The adapter kinds this engine knows. Unknown kinds never get this far
    /// — `catalog_load` rejects them at manifest load — but `parse` is the
    /// single place the strings become types, so it is the thing to test.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "rust-codegen" => Some(Adapter::RustCodegen),
            "exec" => Some(Adapter::Exec),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Adapter::RustCodegen => "rust-codegen",
            Adapter::Exec => "exec",
        }
    }
}

/// One subject's share of a run. `spatial-bench run` builds one per subject and
/// hands it to [`prepare`] and [`run`]; `conform` will do the same, which is
/// what keeps the two from drifting.
///
/// Everything adapter-specific lives in [`CodegenInputs`], resolved only when
/// the adapter needs it — so an `exec` subject is refused by [`prepare`] with
/// the honest message, rather than dying earlier on a driver crate it never
/// asked for.
pub struct SubjectRequest {
    pub subject: String,
    pub adapter: Adapter,
    /// Where generated packages (and, one day, shims) are built. Engine
    /// output, never sources.
    pub build_root: PathBuf,
    pub toolchain: crate::toolchain::Version,
    /// The engine checkout to patch bencher-resident driver crates' core/
    /// measure deps to (day 1.5). `None` — registry versions stand.
    pub engine_root: Option<PathBuf>,
    /// rust-codegen only: everything the two-phase generation and build need.
    pub codegen: Option<CodegenInputs>,
    /// exec only: the manifest's build recipe and source pin (day 1.5).
    pub exec: Option<crate::exec::ExecInputs>,
}

/// The rust-codegen adapter's inputs. The CLI resolves these — engine tree or
/// published crate, pin or working tree — because they are run policy; what
/// they *mean* is decided here, in the dispatch.
pub struct CodegenInputs {
    /// Engine crate providing the macro and harness runtime.
    pub driver_crate: String,
    pub driver_macro: String,
    pub driver_source: crate::build::DriverSource,
    /// Cache-lane identity for the driver crate: "path:<root>" or
    /// "registry:<version>". Part of the build cache key.
    pub driver_rev: String,
    pub subject_crate: String,
    pub subject_source: crate::build::SubjectSource,
    /// The subject's revision for the cache key: its pin, or "worktree:<path>"
    /// for a `--subject-path` build.
    pub subject_rev: String,
    pub features: Vec<String>,
    pub rustflags: Option<String>,
}

/// What [`prepare`] produced: generated and materialised, not yet compiled.
/// [`run`] takes it from here; a dry run stops at this point and prints it.
#[derive(Debug)]
pub struct Prepared {
    /// The materialised package directory, keyed by cache key.
    pub dir: PathBuf,
    /// How many monomorphisations the selection needs, and the cache key that
    /// identifies the build.
    pub generated: Generated,
    pub adapter: Adapter,
    pub toolchain: crate::toolchain::Version,
    pub rustflags: Option<String>,
    /// The argv that runs the driver: `[binary]` for compiled drivers,
    /// `[venv python, driver.py]` for python exec subjects (day 1.5). The
    /// harness contract's transport is identical either way.
    pub program: Vec<String>,
    /// The library revision actually built against — the resolved git sha for
    /// cxx exec subjects; `None` for rust (the Cargo.lock provenance covers
    /// it) and for python (the pinned PyPI version is immutable).
    pub resolved_sha: Option<String>,
}

/// Do the adapter's build-time work: generate, materialise, or (for `exec`)
/// refuse until the shim machinery exists. Called by runs and by conform's
/// generation check alike (§13 check 2).
pub fn prepare(
    request: &SubjectRequest,
    cases: &[&crate::case::Case],
) -> Result<Prepared, RunError> {
    match request.adapter {
        Adapter::RustCodegen => {
            let codegen = request
                .codegen
                .as_ref()
                .ok_or_else(|| RunError::BadManifest {
                    subject: request.subject.clone(),
                    what: "a rust-codegen request carries no codegen inputs".to_owned(),
                })?;
            let generated = crate::codegen::generate(
                &codegen.driver_crate,
                &codegen.driver_macro,
                &request.subject,
                cases,
                &crate::codegen::BuildInputs {
                    toolchain: request.toolchain.to_string(),
                    driver_rev: codegen.driver_rev.clone(),
                    rustflags: codegen.rustflags.clone(),
                    subject_rev: codegen.subject_rev.clone(),
                    features: codegen.features.clone(),
                },
            );
            let dir = crate::build::materialise(
                &request.build_root,
                &crate::build::BuildRequest {
                    subject: request.subject.clone(),
                    driver_crate: codegen.driver_crate.clone(),
                    driver_source: codegen.driver_source.clone(),
                    engine_root: request.engine_root.clone(),
                    subject_crate: codegen.subject_crate.clone(),
                    subject_source: codegen.subject_source.clone(),
                    features: codegen.features.clone(),
                    rustflags: codegen.rustflags.clone(),
                    generated: generated.clone(),
                },
            )?;
            let binary = dir.join("target/release/driver");
            Ok(Prepared {
                dir,
                generated,
                adapter: request.adapter,
                toolchain: request.toolchain,
                rustflags: codegen.rustflags.clone(),
                program: vec![binary.display().to_string()],
                resolved_sha: None,
            })
        }
        Adapter::Exec => {
            let inputs = request.exec.as_ref().ok_or_else(|| RunError::BadManifest {
                subject: request.subject.clone(),
                what: "an exec request carries no exec inputs".to_owned(),
            })?;
            let (built, dir) = crate::exec::prepare(&request.subject, inputs, &request.build_root)
                .map_err(|e| RunError::Exec {
                    subject: request.subject.clone(),
                    message: e,
                })?;
            Ok(Prepared {
                dir,
                generated: crate::codegen::Generated {
                    source: String::new(),
                    cache_key: built.cache_key.clone(),
                    combinations: built.combinations,
                },
                adapter: request.adapter,
                toolchain: request.toolchain,
                rustflags: None,
                program: built.program,
                resolved_sha: built.resolved_sha,
            })
        }
    }
}

/// Do the adapter's run-time work: compile (if the adapter builds a driver)
/// and drive the resolved points, collecting them. The spec arrives fully
/// resolved; the adapter never sees the selector.
pub fn run(prepared: &Prepared, spec: &crate::harness::RunSpec) -> Result<Vec<Point>, RunError> {
    match prepared.adapter {
        Adapter::RustCodegen => {
            crate::build::compile(
                &prepared.dir,
                &prepared.toolchain,
                prepared.rustflags.as_deref(),
            )?;
        }
        // The exec build happened in `prepare` (a venv or a compiled shim).
        Adapter::Exec => {}
    }
    let mut command = std::process::Command::new(&prepared.program[0]);
    command.args(&prepared.program[1..]);
    Ok(crate::harness::drive(&mut command, spec)?)
}

/// The driver's registration list (§13): compile (rust) or reuse the built
/// environment (exec), then ask the driver what it contains. Conform's
/// set-equality runs on this.
pub fn list(prepared: &Prepared) -> Result<Vec<Vec<(String, String)>>, RunError> {
    let mut command = std::process::Command::new(&prepared.program[0]);
    command.args(&prepared.program[1..]).arg("--list");
    match prepared.adapter {
        Adapter::RustCodegen => {
            crate::build::compile(
                &prepared.dir,
                &prepared.toolchain,
                prepared.rustflags.as_deref(),
            )?;
        }
        Adapter::Exec => {}
    }
    let out = command.output().map_err(|e| {
        RunError::Harness(crate::harness::HarnessError::Spawn {
            binary: prepared.program.join(" "),
            err: e.to_string(),
        })
    })?;
    if !out.status.success() {
        return Err(RunError::Harness(
            crate::harness::HarnessError::DriverExit {
                binary: prepared.program.join(" "),
                code: out.status.code(),
            },
        ));
    }
    crate::harness::read_registrations(out.stdout.as_slice()).map_err(RunError::Harness)
}

/// Why a subject's share of a run could not be prepared or executed.
#[derive(Debug)]
pub enum RunError {
    /// The adapter is known but its engine side does not exist yet.
    NotImplemented {
        subject: String,
        adapter: Adapter,
        what: &'static str,
    },
    /// A manifest invariant the loader should have caught; defensive only.
    BadManifest {
        subject: String,
        what: String,
    },
    /// The exec build or run failed — a venv, a shim compile, a library
    /// fetch at its pin.
    Exec {
        subject: String,
        message: String,
    },
    Build(crate::build::BuildError),
    Harness(crate::harness::HarnessError),
}

impl From<crate::build::BuildError> for RunError {
    fn from(value: crate::build::BuildError) -> Self {
        RunError::Build(value)
    }
}

impl From<crate::harness::HarnessError> for RunError {
    fn from(value: crate::harness::HarnessError) -> Self {
        RunError::Harness(value)
    }
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunError::NotImplemented {
                subject,
                adapter,
                what,
            } => write!(
                f,
                "cannot run {subject}: the `{}` adapter is not implemented yet — {what}",
                adapter.as_str()
            ),
            RunError::BadManifest { subject, what } => {
                write!(f, "{subject}'s manifest is invalid: {what}")
            }
            RunError::Exec { subject, message } => {
                write!(f, "cannot build or run {subject}: {message}")
            }
            RunError::Build(e) => write!(f, "{e}"),
            RunError::Harness(e) => write!(f, "{e:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> crate::catalog::Catalog {
        let dir = crate::test_support::subjects_dir();
        crate::catalog_load::load_dir(&dir).unwrap()
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sb-adapter-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    // ---- the dispatch (§5) -----------------------------------------------

    #[test]
    fn adapter_strings_parse_to_known_kinds_only() {
        assert_eq!(Adapter::parse("rust-codegen"), Some(Adapter::RustCodegen));
        assert_eq!(Adapter::parse("exec"), Some(Adapter::Exec));
        // Near misses that a typo would produce: refused, not guessed.
        for bad in ["rust_codegen", "rust codegen", "rust-coden", "criterion"] {
            assert_eq!(Adapter::parse(bad), None, "{bad}");
        }
    }

    /// The old failure mode: a subject declaring `exec` died with "declares no
    /// driver macro", which described nothing. The dispatch refuses by name,
    /// saying what is missing and why. (Day 1.5: exec subjects now prepare —
    /// see the exec test below.)
    /// Day 1.5: exec subjects prepare through their language builder. The
    /// full build (fetch the pinned library, compile the shim) is exercised
    /// by the live e2e run; here the defensive path matters — an exec request
    /// without its inputs is a caller bug, refused rather than guessed at.
    #[test]
    fn exec_prepare_without_inputs_is_refused() {
        let catalog = catalog();
        let case = catalog.for_subject("nanoflann").next().unwrap();
        let request = SubjectRequest {
            subject: "nanoflann".into(),
            adapter: case.adapter,
            build_root: scratch("exec"),
            toolchain: crate::toolchain::Version(0, 0, 0),
            engine_root: None,
            exec: None,
            codegen: None,
        };
        match prepare(&request, &[case]) {
            Err(RunError::BadManifest { subject, .. }) => {
                assert_eq!(subject, "nanoflann");
            }
            Err(other) => panic!("expected a BadManifest refusal, got {other:?}"),
            Ok(_) => panic!("expected a refusal, got a prepared run"),
        }
    }

    /// The rust-codegen path prepares a real package from the real manifest —
    /// the same thing the drift-guard test then compiles, and `run` then
    /// drives. Compiling happens elsewhere; this asserts the seam produces
    /// exactly what the adapter contract says it does.
    #[test]
    fn rust_codegen_prepares_a_materialised_package() {
        let catalog = catalog();
        let case = catalog.for_subject("kiddo_v6").next().unwrap();
        let request = SubjectRequest {
            subject: "kiddo_v6".into(),
            adapter: case.adapter,
            build_root: scratch("prepare"),
            toolchain: crate::toolchain::Version(1, 89, 0),
            engine_root: None,
            exec: None,
            codegen: Some(CodegenInputs {
                driver_crate: "spatial-bench-kiddo-v6".into(),
                driver_macro: "bench_case".into(),
                driver_source: crate::build::DriverSource::Path("/engine/crates/sb".into()),
                driver_rev: "path:/engine".into(),
                subject_crate: "kiddo".into(),
                subject_source: crate::build::SubjectSource::Path("/engine/kiddo".into()),
                subject_rev: "v6.1.0".into(),
                features: vec!["test_utils".into()],
                rustflags: None,
            }),
        };
        let prepared = prepare(&request, &[case]).unwrap();
        assert_eq!(prepared.adapter, Adapter::RustCodegen);
        let main = std::fs::read_to_string(prepared.dir.join("src/main.rs")).unwrap();
        assert!(
            main.contains("bench_case!("),
            "the materialised package invokes the driver macro"
        );
        assert!(
            prepared.dir.ends_with(&prepared.generated.cache_key),
            "the package directory is keyed by the cache key"
        );
    }
}
