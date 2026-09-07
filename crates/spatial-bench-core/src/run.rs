//! The run pipeline: everything between a resolved selection and a written run
//! document.
//!
//! This is the orchestration the CLI used to own: toolchain resolution, the
//! fingerprint gate (§9), feasibility warnings (§4), per-subject dispatch
//! through the adapters (§5), provenance from the lockfile, and the document
//! itself (§10). It lives in core rather than the CLI so `spatial-bench run`
//! and `conform` (§13) drive an identical path, and so the CLI stays argument
//! parsing, rendering, and exit codes.
//!
//! Printing: progress and diagnostics go to stderr (`building …`, the memory
//! warning); stdout rendering — summaries, the outcome — belongs to the
//! caller, which is why the returns are structured rather than printed here.
//! Errors are user-facing strings, not an API surface: every failure here is
//! terminal for the run.

use crate::case::{Budget, Runner};
use crate::catalog::Catalog;
use crate::codegen;
use crate::harness::{self, CaseSpec, RunSpec};
use crate::schema::{self, SubjectProvenance};
use crate::toolchain::{self, Version};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// One run's inputs, resolved by the caller. The CLI assembles this from
/// arguments and environment; `conform` will assemble it from its own policy.
pub struct RunConfig<'a> {
    pub catalog: &'a Catalog,
    pub selection: &'a crate::selector::SelectorSet,
    pub runner: Runner,
    /// Pin the toolchain. Refused if below any selected subject's floor (§4).
    pub rustc: Option<&'a str>,
    /// Proceed with no fingerprint file; the machine hash is degraded by
    /// construction and the dataset should reject the run (§9).
    pub allow_unfingerprinted: bool,
    /// The random seed for the dataset generator.
    pub random_seed: u64,
    /// Where generated packages are built. Engine output, never sources.
    pub build_root: PathBuf,
    /// An engine source checkout for driver path deps, if one is reachable.
    /// `None` means drivers come from the versions their manifests pin.
    pub engine_root: Option<PathBuf>,
    /// Working-tree overrides: subject name → directory. Such runs record a
    /// checkout rather than a revision and are marked `git_dirty` (§12).
    pub subject_paths: &'a BTreeMap<String, PathBuf>,
}

/// What a dry run reports, per subject: the compile-time cost and the cache
/// key that identifies the build.
#[derive(Debug)]
pub struct SubjectPlan {
    pub subject: String,
    pub combinations: usize,
    pub cache_key: String,
}

/// A dry run's plan: the one toolchain everything will build with, and what
/// each subject's build will be.
#[derive(Debug)]
pub struct RunPlan {
    /// `None` — no rust subjects in the selection.
    pub toolchain: Option<Version>,
    pub subjects: Vec<SubjectPlan>,
}

/// What a completed run produced.
pub struct RunOutcome {
    /// Where the run document was written (§10's layout).
    pub path: PathBuf,
    pub points: usize,
    pub toolchain: Option<Version>,
}

/// The one toolchain every Rust subject in this selection builds with (§4):
/// the highest floor among them, or the caller's pin.
pub fn resolve_toolchain(config: &RunConfig<'_>) -> Result<Option<Version>, String> {
    let floors: Vec<(String, Option<String>)> =
        codegen::by_subject(config.catalog, config.selection)
            .iter()
            .map(|(subject, _)| (subject.clone(), config.catalog.min_rustc(subject)))
            .collect();
    let version = toolchain::resolve(&floors, config.rustc).map_err(|e| format!("{e:?}"))?;
    Ok((version != Version(0, 0, 0)).then_some(version))
}

/// The dry run: prepare each subject's build (generate + materialise, both
/// idempotent) and report what would compile. Stops before the fingerprint
/// gate — a plan measures nothing, so it does not need the host to be valid.
pub fn plan(config: &RunConfig<'_>) -> Result<RunPlan, String> {
    let toolchain = resolve_toolchain(config)?;
    let mut subjects = Vec::new();
    for (subject, cases) in &codegen::by_subject(config.catalog, config.selection) {
        let request = subject_request(config, subject, cases[0], toolchain.as_ref())?;
        let prepared = crate::adapter::prepare(&request, cases).map_err(|e| e.to_string())?;
        subjects.push(SubjectPlan {
            subject: subject.clone(),
            combinations: prepared.generated.combinations,
            cache_key: prepared.generated.cache_key,
        });
    }
    Ok(RunPlan {
        toolchain,
        subjects,
    })
}

/// Execute a run: validate the host, build and drive each selected subject
/// through its adapter, and write the run document. Returns where the
/// document landed.
pub fn execute(config: &RunConfig<'_>) -> Result<RunOutcome, String> {
    let catalog = config.catalog;
    let selection = config.selection;
    let points = catalog.points(selection);
    if points.is_empty() {
        return Err("that selection matches no data points".to_owned());
    }
    let budget = Budget::default();
    let groups = codegen::by_subject(catalog, selection);
    let toolchain = resolve_toolchain(config)?;

    // The run's clock starts here — after planning, before validation and
    // building — so `finished_at`, taken when the document is written, shows
    // real wall-clock. Both timestamps used to be the same `now`, which made
    // every run look instantaneous.
    let started_at = schema::utc_timestamp();

    // §9: validate this host before measuring anything. A stale fingerprint
    // would attribute new hardware's numbers to the old machine's history,
    // which is worse than refusing to run.
    let host = crate::fingerprint::validate_for_run(config.allow_unfingerprinted)?;
    let machine = host.machine;

    // §4: a manifest's ranges say what is worth sweeping; what actually fits
    // is a per-machine memory question, answered here, with a warning rather
    // than a refusal — the estimate ignores tree overhead, and the operator
    // may know better than it does.
    if let Some(total) = machine.mem.total_bytes {
        for tags in catalog.over_budget(selection, total / 2) {
            let size = tags
                .get("tree_size")
                .map(ToString::to_string)
                .unwrap_or_default();
            eprintln!(
                "warning: tree_size {size} needs more than half this host's \
                 {} GiB (lower-bound estimate); run at your own risk",
                total / (1 << 30)
            );
        }
    }

    let dataset_generator_path = resolve_dataset_generator();
    let mut collected: Vec<crate::schema::Point> = Vec::new();
    // §4: every run records what it actually built, per subject.
    let mut subjects: BTreeMap<String, SubjectProvenance> = Default::default();

    for (subject, cases) in &groups {
        // §5's dispatch: prepare (generate + materialise) and run (compile +
        // drive) are the adapter's business, shared with conform (§13). What
        // this loop adds is policy: sources, seeds, provenance, the document.
        let request = subject_request(config, subject, cases[0], toolchain.as_ref())?;
        let prepared = crate::adapter::prepare(&request, cases).map_err(|e| e.to_string())?;

        eprintln!(
            "building {subject} in {}{}",
            prepared.dir.display(),
            toolchain
                .as_ref()
                .map(|v| format!(" (rustc {v})"))
                .unwrap_or_else(|| " (no rust subjects)".to_owned())
        );
        let built: Vec<crate::schema::Point> = match config.runner {
            Runner::Criterion => {
                // A criterion run is one process per subject: a single spec
                // carrying every resolved point, swept inside the driver.
                let spec = RunSpec {
                    harness_version: harness::HARNESS_VERSION,
                    budget,
                    cases: points
                        .iter()
                        .filter(|(c, _)| &c.subject == subject)
                        .map(|(c, tags)| CaseSpec {
                            id: c.id.clone(),
                            tags: tags.clone(),
                            dataset_generator: dataset_generator_path.clone(),
                            dataset: tags
                                .get("dataset")
                                .map(ToString::to_string)
                                .unwrap_or_default(),
                            random_seed: config.random_seed,
                        })
                        .collect(),
                };
                crate::adapter::run(&prepared, &spec).map_err(|e| e.to_string())?
            }
            Runner::Perf => {
                // §11: perf stat attributes counters to a PROCESS, so a perf
                // run is ONE point per process — the binary is built once,
                // then spawned under perf with a single-case spec per point.
                // Counters cover the whole process (build, warm-up,
                // measurement), which is the per-process overhead the
                // estimates already price in.
                // The exec build happened in `prepare` (a venv or a compiled
                // shim); the rust driver compiles here.
                if prepared.adapter == crate::adapter::Adapter::RustCodegen {
                    crate::build::compile(
                        &prepared.dir,
                        &prepared.toolchain,
                        prepared.rustflags.as_deref(),
                    )
                    .map_err(|e| e.to_string())?;
                }
                let mut out = Vec::new();
                for (c, tags) in points.iter().filter(|(c, _)| &c.subject == subject) {
                    let spec = RunSpec {
                        harness_version: harness::HARNESS_VERSION,
                        budget,
                        cases: vec![CaseSpec {
                            id: c.id.clone(),
                            tags: tags.clone(),
                            dataset_generator: dataset_generator_path.clone(),
                            dataset: tags
                                .get("dataset")
                                .map(ToString::to_string)
                                .unwrap_or_default(),
                            random_seed: config.random_seed,
                        }],
                    };
                    out.push(
                        crate::perf::measure_point(&prepared.program, &spec)
                            .map_err(|e| e.to_string())?,
                    );
                }
                out
            }
            Runner::Asm | Runner::Mca => {
                return Err(format!(
                    "{subject}: the {} runner emits artefacts rather than points \
                     (design §11) — no measurement this pipeline can run",
                    config.runner
                ));
            }
        };

        // Provenance: what cargo actually resolved, from the lockfile. A
        // working-tree build records the crate's real version but no revision,
        // and Source.git_dirty already marks the run for the dataset to reject.
        let pin = catalog.pinned_ref(subject).unwrap_or_default();
        let crate_name = crate::vocab::namespace_of(subject).to_owned();
        let built_from = crate::build::built_subject(&prepared.dir.join("Cargo.lock"), &crate_name);

        // a manifest-pinned sha is enforced, not merely recorded — for
        // rust subjects via the lockfile revision; for cxx exec subjects the
        // builder itself verified the fetched sources against the declared
        // sha and recorded what it built. Worktree builds are exempt: they
        // have no revision by definition and are already marked git_dirty.
        let expected_sha = if config.subject_paths.contains_key(subject)
            || cases[0].adapter == crate::adapter::Adapter::Exec
        {
            None
        } else {
            catalog.expected_sha(subject)
        };
        if let Some(expected) = expected_sha {
            let resolved = built_from.as_ref().and_then(|(_, sha)| sha.as_deref());
            enforce_sha(subject, &expected, resolved)?;
        }

        subjects.insert(
            subject.clone(),
            SubjectProvenance {
                version: built_from
                    .as_ref()
                    .map(|(v, _)| v.clone())
                    .unwrap_or_else(|| pin.clone()),
                pinned_ref: pin,
                sha: built_from
                    .and_then(|(_, sha)| sha)
                    .or(prepared.resolved_sha.clone()),
            },
        );
        collected.extend(built);
    }

    let path = write_document(
        RunHeader {
            selection,
            runner: config.runner,
            toolchain: toolchain.as_ref(),
            rustflags: catalog.rustflags_any(),
            overrides: config.subject_paths,
            machine: &machine,
            subjects,
            started_at,
            fingerprint: host.fingerprint.as_ref().map(|p| p.display().to_string()),
        },
        collected,
    )?;
    Ok(RunOutcome {
        path,
        points: points.len(),
        toolchain,
    })
}

/// the manifest may pin the exact revision its ref must resolve to. The
/// comparison runs against what cargo actually locked, so a moved tag is
/// refused at build time, naming both revisions, instead of being detectable
/// only by auditing run headers afterwards.
/// The dataset generator binary path: beside the CLI binary, or from the
/// workspace (dev builds). Same discovery pattern as the charting trampoline.
fn resolve_dataset_generator() -> String {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join("spatial-bench-dataset");
            if candidate.is_file() {
                return candidate.display().to_string();
            }
        }
    }
    "spatial-bench-dataset".to_owned()
}

fn enforce_sha(subject: &str, expected: &str, resolved: Option<&str>) -> Result<(), String> {
    match resolved {
        Some(sha) if sha == expected => Ok(()),
        Some(sha) => Err(format!(
            "{subject}: the pin moved — the manifest expects {expected} but \
             cargo resolved {sha}. If the new revision is intended, update the \
             manifest's pinned_ref and sha together."
        )),
        None => Err(format!(
            "{subject}: the pin cannot be verified — the lockfile records no \
             git revision for it"
        )),
    }
}

/// Refused rather than run under criterion with a header claiming perf: a
/// header must never describe a measurement that did not happen. This includes
/// dry runs, which would otherwise print a plan for a measurement no code path
/// can take.
/// One subject's share of a run, assembled for the §5 dispatch. Codegen inputs
/// are resolved only for the adapter that needs them: an `exec` subject is
/// refused by `adapter::prepare` with the honest adapter error, not by a
/// driver-source lookup suggesting `--engine-src` fixes an adapter that does
/// not exist.
/// The selected cases' `isa` tag is a compiler input: it decides the build
/// flags outright (replacing the manifest's own target flags, which exist
/// only for subjects that do not declare an ISA). A selection spanning
/// several ISA values would need differently compiled binaries in one run,
/// so it is refused rather than silently measuring one of them.
fn isa_resolved_rustflags(config: &RunConfig<'_>, subject: &str) -> Result<Option<String>, String> {
    let isas: std::collections::BTreeSet<String> = config
        .catalog
        .matching(config.selection)
        .iter()
        .filter(|c| c.subject == subject)
        .filter_map(|c| c.tags.get("isa").map(ToString::to_string))
        .collect();
    match isas.len() {
        0 => Ok(config.catalog.rustflags(subject)),
        1 => {
            let isa = isas.iter().next().unwrap();
            match crate::build::isa_rustflags(isa) {
                Some(flags) => Ok(Some(flags.to_owned())),
                None => Err(format!(
                    "{subject}: unknown isa `{isa}` — the value is not in the vocabulary"
                )),
            }
        }
        _ => Err(format!(
            "{subject}: the selection spans several isa values {isas:?}; ISA \
             drives the compiler, so narrow the selection to one"
        )),
    }
}

fn subject_request(
    config: &RunConfig<'_>,
    subject: &str,
    first_case: &crate::case::Case,
    toolchain: Option<&Version>,
) -> Result<crate::adapter::SubjectRequest, String> {
    let inputs = crate::resolve::Inputs {
        catalog: config.catalog,
        engine_root: config.engine_root.as_ref(),
        subject_paths: config.subject_paths,
    };
    let exec_inputs = match first_case.adapter {
        // an exec subject's driver is built by the language builder,
        // from the manifest's own recipe and pin. The builder verifies the
        // library sha itself, so the run-level sha enforcement is skipped.
        crate::adapter::Adapter::Exec => {
            let build = config.catalog.build(subject).ok_or_else(|| {
                format!("{subject} declares the exec adapter but no build recipe")
            })?;
            let entry = config
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
                manifest_dir: config
                    .catalog
                    .manifest_dir(subject)
                    .ok_or_else(|| format!("{subject} has no manifest directory"))?,
                lang: first_case
                    .driver_lang
                    .clone()
                    .ok_or_else(|| format!("{subject} declares no driver language"))?,
                entry,
                build,
                source: config.catalog.source_full(subject)?,
                expected_sha: config.catalog.expected_sha(subject),
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
            let (driver_source, driver_rev) =
                crate::resolve::driver_source(&inputs, subject, &driver_crate)?;
            let subject_source = crate::resolve::subject_source(&inputs, subject)?;
            let subject_rev = crate::resolve::subject_rev(&inputs, subject);
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
                features: config.catalog.features(subject),
                rustflags: isa_resolved_rustflags(config, subject)?,
            })
        }
        _ => None,
    };
    Ok(crate::adapter::SubjectRequest {
        subject: subject.to_owned(),
        adapter: first_case.adapter,
        build_root: config.build_root.clone(),
        toolchain: toolchain.copied().unwrap_or(Version(0, 0, 0)),
        engine_root: config.engine_root.clone(),
        exec: exec_inputs,
        codegen,
    })
}

/// Everything the run header records, gathered by [`execute`] and handed to
/// [`write_document`] as one thing — a run document's header is a record, not
/// a call signature.
struct RunHeader<'a> {
    selection: &'a crate::selector::SelectorSet,
    runner: Runner,
    toolchain: Option<&'a Version>,
    rustflags: Option<String>,
    overrides: &'a BTreeMap<String, PathBuf>,
    machine: &'a crate::machine::Machine,
    subjects: BTreeMap<String, SubjectProvenance>,
    /// The fingerprint file this run validated against, when one did .
    fingerprint: Option<String>,
    started_at: String,
}

/// Assemble and write the run document (§10): header, machine, context, the
/// subjects block from the lockfile provenance, and the collected points.
fn write_document(
    header: RunHeader<'_>,
    points: Vec<crate::schema::Point>,
) -> Result<PathBuf, String> {
    use crate::schema::{result_path, Document, Run, Source, Toolchain};
    let RunHeader {
        selection,
        runner,
        toolchain,
        rustflags,
        overrides,
        machine,
        subjects,
        started_at,
        fingerprint: header_fingerprint,
    } = header;

    let run = Run {
        run_id: schema::ulid(),
        started_at,
        finished_at: schema::utc_timestamp(),
        runner: match runner {
            Runner::Perf => "perf",
            _ => "criterion",
        }
        .to_owned(),
        selectors: selection.to_exprs(),
        machine_hash: machine.hash(),
        machine: machine.clone(),
        // the source of trust for the machine hash is recorded — a run
        // validated against /etc is distinguishable from one validated against
        // a user-supplied file, and from one that was never verified.
        context: crate::schema::Context {
            fingerprint: header_fingerprint,
            ..crate::context::probe()
        },
        toolchain: Toolchain {
            rustc: toolchain
                .map(|v| v.to_string())
                .unwrap_or_else(|| "none (no rust subjects)".to_owned()),
            host: std::env::consts::ARCH.to_owned(),
            target_cpu: None,
            rustflags: rustflags.clone(),
            features: Vec::new(),
            cargo_profile: "release".to_owned(),
            opt_level: Some("3".to_owned()),
        },
        // A working-tree build has no revision to record. Marked here so the
        // dataset can reject it rather than treating it as reproducible.
        source: Source {
            git_sha: None,
            git_dirty: !overrides.is_empty(),
            // The engine core's version: the schema and harness live here, so
            // this is the version a dataset reader compares against.
            crate_version: env!("CARGO_PKG_VERSION").to_owned(),
        },
        subjects,
    };

    let document = Document {
        schema_version: schema::SCHEMA_VERSION,
        run,
        points,
    };
    let path =
        result_path(&data_dir().join("runs"), &document.run).map_err(|e| format!("{e:?}"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&document).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(path)
}

/// Engine output root: build dirs and run documents live here, never in a
/// checkout (that is the actual fix for stray result files in a repo root).
pub fn data_dir() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("spatial-bench")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::selector::SelectorSet;
    use std::collections::BTreeMap;

    fn catalog() -> Catalog {
        let dir = crate::test_support::subjects_dir();
        crate::catalog_load::load_dir(&dir).unwrap()
    }

    fn config<'a>(
        catalog: &'a Catalog,
        selection: &'a SelectorSet,
        runner: Runner,
        paths: &'a BTreeMap<String, PathBuf>,
    ) -> RunConfig<'a> {
        RunConfig {
            catalog,
            selection,
            runner,
            rustc: None,
            allow_unfingerprinted: true,
            random_seed: 42,
            build_root: std::env::temp_dir().join(format!("sb-run-{}", std::process::id())),
            engine_root: None,
            subject_paths: paths,
        }
    }

    /// kiddo declares a 1.89.0 floor, so that is the toolchain the whole run
    /// builds with — the header records what this function resolved.
    #[test]
    fn toolchain_is_the_highest_declared_floor() {
        let catalog = catalog();
        let selection = SelectorSet::parse_all(["impl=kiddo"]).unwrap();
        let paths = BTreeMap::new();
        let config = config(&catalog, &selection, Runner::Criterion, &paths);
        assert_eq!(resolve_toolchain(&config).unwrap(), Some(Version(1, 89, 0)));
    }

    /// A dry run plans real builds: the manifest's compile-time axes become
    /// combinations, and the toolchain matches the floors.
    #[test]
    fn plan_reports_combinations_per_subject() {
        let catalog = catalog();
        let selection =
            SelectorSet::parse_all(["impl=kiddo,kiddo.stem=eytzinger,isa=avx512"]).unwrap();
        let paths = BTreeMap::new();
        let plan = plan(&config(&catalog, &selection, Runner::Criterion, &paths)).unwrap();
        assert_eq!(plan.toolchain, Some(Version(1, 89, 0)));
        assert_eq!(plan.subjects.len(), 1);
        let kiddo = &plan.subjects[0];
        assert_eq!(kiddo.subject, "kiddo");
        // Two scalars over one stem and one leaf: two monomorphisations.
        assert_eq!(kiddo.combinations, 2);
    }

    /// exec subjects plan through their language builder — the plan
    /// fetches/compiles the shim environment (cached) and reports its
    /// specialisation count, exactly like a rust subject's plan.
    #[test]
    fn planning_an_exec_subject_plans_its_build() {
        let catalog = catalog();
        let selection = SelectorSet::parse_all(["impl=nanoflann"]).unwrap();
        let paths = BTreeMap::new();
        let plan = plan(&config(&catalog, &selection, Runner::Criterion, &paths)).unwrap();
        assert_eq!(plan.subjects.len(), 1);
        let nanoflann = &plan.subjects[0];
        assert_eq!(nanoflann.subject, "nanoflann");
        // The manifest declares compile_time_dims [3, 4]: two specialisations.
        assert_eq!(nanoflann.combinations, 2);
    }

    /// the invariant, now positive: planning a perf run works — the runner
    /// exists, so the plan no longer refuses it.
    #[test]
    fn a_perf_run_is_planned_like_any_other() {
        let catalog = catalog();
        let selection = SelectorSet::parse_all(["impl=kiddo,k=1,isa=avx512"]).unwrap();
        let paths = BTreeMap::new();
        let plan = plan(&config(&catalog, &selection, Runner::Perf, &paths)).unwrap();
        assert_eq!(plan.subjects.len(), 1);
    }

    /// a manifest-pinned sha is enforced against what cargo actually
    /// resolved — a moved tag is refused at build time, naming both revisions.
    #[test]
    fn a_moved_tag_is_refused_by_name() {
        let expected = "0123456789abcdef0123456789abcdef01234567";
        assert!(enforce_sha("kiddo", expected, Some(expected)).is_ok());

        let resolved = "fedcba9876543210fedcba9876543210fedcba98";
        let err = enforce_sha("kiddo", expected, Some(resolved)).unwrap_err();
        assert!(
            err.contains(expected) && err.contains(resolved) && err.contains("pin moved"),
            "{err}"
        );

        // No revision in the lockfile at all: unverifiable is also a refusal.
        assert!(enforce_sha("kiddo", expected, None).is_err());
    }
}
