# Add a query type

Define the operation independently of any one library's API before choosing its
`query` tag. A shared tag promises a shared workload meaning; a library-specific
optimization or API name is not itself a new query definition.

## Define semantics and timing

Record the input, output and timed work, including:

- Distance metric and units: Euclidean radius and squared-distance threshold
  are different quantities. Specify conversion and whether the boundary is
  strict or inclusive.
- Cardinality: exact/up-to `k`, empty results, fewer than `k` eligible points,
  duplicate coordinates, ties, and self matches.
- Ordering: distance order, item order, or unspecified order; whether sorting
  and result materialization are part of the requested operation.
- Exactness or approximation: error/recall guarantees, tuning parameters and
  the evidence used to check them. Do not label approximate results exact.
- Call shape and concurrency: one probe per call or a bulk query API, allowed
  threads, and any preparation reusable across queries.
- Measurement boundary: index build, preprocessing, query allocation, result
  traversal/consumption, batching denominator and reported units. A tree-to-tree
  radius enumeration is a different workload from all-points nearest-neighbor
  search.

A construction benchmark needs separate treatment of setup, object destruction,
repeated builds and throughput units. The query helper's `ns/query` output
cannot automatically describe every `build` or `add_points` workload.

## Trace the changes across repositories

1. In core, add the shared query name to `UNIVERSAL` in
   [vocab.rs](../crates/spatial-bench-core/src/vocab.rs). Reuse existing shared
   axes where their meaning fits; introduce shared parameters only for new
   concepts. Check [manifest lowering](../crates/spatial-bench-core/src/catalog_load.rs),
   [selection/expansion](../crates/spatial-bench-core/src/case.rs), and
   [measurement conversion](../crates/spatial-bench-measure/src/lib.rs) for
   assumptions affected by the new operation.
2. In [benchers](https://github.com/spatial-bench/spatial-bench-benchers), implement
   the operation in each intended driver and add honest manifest cases. Identify
   compile-time dispatch changes separately from runtime parameters. Omit
   unsupported operations; an absent measurement is not zero cost. Use the
   [driver contract](https://github.com/spatial-bench/spatial-bench-benchers/blob/master/docs/driver-contract.md)
   and [updating guide](https://github.com/spatial-bench/spatial-bench-benchers/blob/master/docs/updating-a-library.md).
3. Choose explicit Standard Corpus selections in bencher `corpus.toml`, with
   cost and coverage rationale. Do not widen the corpus just because a query
   name became legal.
4. Update the web [methodology](https://spatial-bench.org/methodology) and
   [coverage](https://spatial-bench.org/coverage), and check explorer filtering,
   labels and metric units. Update result collation/readers if new persisted
   metrics or fields must be displayed.

A new query represented by existing tags does not alone require a harness or
schema bump. A changed input/output shape or incompatible semantics does.
Check `HARNESS_VERSION`, generator stream version and `SCHEMA_VERSION`
separately; coordinate the consumers of the contract actually changed.
Existing unknown tags can survive collation in `point_tags`, but a new metric
is not automatically exposed by the current SQLite projection or explorer.

## Produce semantic evidence before timing evidence

Leave a small runnable known-answer or brute-force/reference check in the
adapter's existing tests. Construct a tiny point set with known ties, a point
on the radius boundary, and an empty-result case where applicable. Compare
returned IDs, distances and ordering according to the defined semantics;
handle equally valid tied answers rather than accidentally requiring one
library's tie-breaking rule. For approximation, check the stated bound or
recall criterion against exact reference answers.

Then run core and relevant driver checks, inspect the new selector with `list`,
check registrations with `conform --subject NAME`, and run one small selected
case using [the local workflow](running-benchmarks.md). Conformance compares
compile-time registrations only; the known-answer check must establish query
correctness separately. Preserve the exact command, seed, source revisions and
normalization calculation with the small-run evidence.

Link core, bencher and web PRs. The review should be able to verify the semantic
definition, API mapping, correctness evidence, timing boundary, default/tuned
classification, and the order of compatible changes before wider measurements
are requested. Reusing an old query label for a corrected meaning also needs a
review of existing results; coordinate that with
[results maintainers](https://github.com/spatial-bench/spatial-bench-results/blob/main/CONTRIBUTING.md).
