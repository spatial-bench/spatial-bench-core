# Architecture and contracts

Verified against core `2104bfe` and benchers `1972c50` on 2026-09-14. This is a
reference to implemented behavior. The earlier design draft mixed proposals,
Kiddo-specific history and completed work; Git history retains that draft.

## Repository boundaries

```mermaid
flowchart LR
    B[Bencher manifests and drivers] --> E[Core engine measurements]
    E --> R[Reviewed run documents]
    R --> S[Published SQLite snapshot]
    S --> W[Web explorer]
```

The core repository owns contracts and orchestration. The bencher repository
owns adapters for library APIs, source pins, and declared cases. Library authors
can contribute these adapters; independent review is a contribution practice,
not a property that software can establish from authorship. Libraries under test
need not adopt a spatial-bench API or modify their own source trees.

## From manifest to point

[Catalog loading](../crates/spatial-bench-core/src/catalog_load.rs) reads sorted
subject directories, expands manifest matrices, merges namespaced extension
vocabularies, and validates tags against the shared vocabulary. A manifest
selects a driver compatible with the pinned library version. The
`defaults_or_tuned` tag is derived by comparing configuration tags with declared
manifest defaults. This checks consistency with the declaration; review must
establish whether that declaration represents the library's ordinary settings.

A **case** has fixed tags, parameter domains and defaults, an adapter, and
available runners. A **point** resolves all parameter values. For example,
`impl=kdtree,axis=f64,k=1` selects a case whose `tree_size` and `query_count`
remain parameters; pinning them yields a complete workload tag map. The
[selector](../crates/spatial-bench-core/src/selector.rs) filters fixed tags,
then [parameter expansion](../crates/spatial-bench-core/src/case.rs) enumerates
constrained parameter values and applies the selector to the resolved maps.
Unconstrained parameters contribute their defaults.

Tags identify workload/configuration axes, not a unique observation. Multiple
runs can measure the same tags. Machine context, source provenance, sampling
and run ID remain necessary to interpret those observations.

## Preparation and execution

[The run pipeline](../crates/spatial-bench-core/src/run.rs) resolves a common
Rust toolchain, validates the host fingerprint, warns about lower-bound memory
requirements, groups cases by subject, dispatches through adapters, collects
points and writes a document. A memory warning is not an allocation guarantee:
the estimate omits index overhead and transient build storage.

The two implemented [adapters](../crates/spatial-bench-core/src/adapter/mod.rs)
share the same input/output contract:

| Adapter | Preparation | Execution |
| --- | --- | --- |
| `rust-codegen` | Generate deterministic source for selected compile-time coordinates and materialize a Cargo package | Compile with the resolved toolchain, then invoke its driver |
| `exec` | Fetch/build pinned C++ sources and shim, or create the pinned Python environment | Invoke the prepared executable/interpreter |

Compile-time axes are driver-specific: kdtree specializes scalar type, whereas
other drivers may also specialize dimensionality or layout. Runtime parameter
sweeps reuse a generated binary. Content-based cache keys and Cargo incremental
checks reduce repeated build work; they do not substitute for recording source
revisions and local changes.

`run --dry-run` calls adapter preparation without executing the measurement or
requiring a fingerprint. Exec preparation can still build/download dependencies.
`conform` uses the same preparation and listing path to compare compile-time
registration sets. It does not prove runtime dispatch or query-answer correctness.

## Harness and measurement boundaries

[Harness version 2](../crates/spatial-bench-core/src/harness.rs) sends one JSON
`RunSpec` on stdin: a budget and resolved `CaseSpec` entries, including generator
path, dataset kind and random seed. Drivers emit one JSON `Point` per line on
stdout; diagnostics belong on stderr. The Rust parser rejects a mismatched
harness version and malformed output lines. A driver's `--list` mode emits
compile-time registrations without reading a run specification.

The [input generator](../crates/spatial-bench-dataset/src/main.rs) supplies
uniform or Gaussian construction/query coordinates through a local binary
stream. It uses the run seed for construction points and the wrapping successor
for query points. The old `POINT_SEED`/`QUERY_SEED` constants in `harness.rs` are
not the seeds used by this generator path.

The engine's runner named `criterion` dispatches both Rust and exec drivers.
Rust drivers use [spatial-bench-measure](../crates/spatial-bench-measure/src/lib.rs)
and Criterion; current C++ and Python drivers implement their own timed loops
and statistics. The shared transport and generator do not make the estimators
or timing scopes identical. A driver determines which API work, allocations
and result consumption enter the measured operation.

For Rust query drivers, the helper divides batch-duration estimates by the
number of queries passed to it. Stored `latency_ns` is the normalized mean,
`throughput_qps = 10^9 / latency_ns`, and median/dispersion statistics are
separate. Construction operations require an explicit denominator and unit
review; query normalization must not be assumed suitable for them. See the
[methodology](https://spatial-bench.org/methodology) for sampling details and
what the explorer plots.

The `perf` runner launches one process per point and attaches process-wide
counters to that point. It does not instrument only the timed query closure.
`asm` and `mca` remain vocabulary for unimplemented runner paths and are not
available as executable CLI runners.

## Documents, provenance and publication

[Schema version 1](../crates/spatial-bench-core/src/schema.rs) stores a run header
plus points. The header includes selectors, timestamps, machine continuity
hash and context, toolchain information, engine package version, and per-subject
source provenance. A subject's `sha` can identify a Git revision or a PyPI
artifact digest. Missing values are missing evidence, not zero measurements.

The current writer does not persist random seed or budget, and does not fill
engine `source.git_sha`. Its dirty marker tracks subject-path overrides only.
Retain exact commands and checkout revisions alongside experiments. Neither a
common Rust compiler nor a source pin captures all environmental confounders.

The CLI's [collation implementation](../crates/spatial-bench-cli/src/main.rs)
(`cmd_publish`) reads machine TOML and run JSON from a results checkout. Shared
identity fields become SQLite columns; other tags enter `point_tags`. This
projection is not the complete run document: original JSON remains important
for audits. Core `publish` writes an uncompressed SQLite database and local
manifest; the results repository owns compression, integrity metadata and
remote publication. Follow its
[format/provenance](https://github.com/spatial-bench/spatial-bench-results/blob/main/docs/format-and-provenance.md)
and [publication](https://github.com/spatial-bench/spatial-bench-results/blob/main/docs/publication.md)
guides.

## Retained rationale and extension boundaries

The retained design choices are explicit tag identity instead of parsing names,
separate reviewed adapters instead of relying on each library's benchmarks,
compile-time specialization only for selected cases, and keeping measurements
and public presentation in separate repositories. These reduce ambiguity and
coupling; they are not guarantees of fairness or reproducibility.

Adding a query or distribution usually requires coordinated engine vocabulary,
bencher implementation/coverage, and public explanation changes. It does not
necessarily require a schema migration. Change harness or stream versions when
their existing consumers cannot interpret the new shape or semantics safely;
change result format/readers when the persisted contract changes. Do not reuse
an old tag to silently redefine a previously measured workload.
