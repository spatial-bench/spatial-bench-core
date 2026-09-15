# Contributing to spatial-bench core

Use this repository for changes to the engine, shared input generator,
measurement contract, or collation. For a library adapter or version change,
start with the [bencher contribution guide](https://github.com/spatial-bench/spatial-bench-benchers/blob/master/CONTRIBUTING.md).

| Task | Guide |
| --- | --- |
| Build the engine and run checks | [Development setup](docs/development.md) |
| Measure and inspect a small case | [Running benchmarks](docs/running-benchmarks.md) |
| Follow selection through to recorded results | [Architecture](docs/design.md) |
| Extend generated inputs | [Adding datasets](docs/adding-datasets.md) |
| Define and implement another operation | [Adding query types](docs/adding-query-types.md) |
| Find protocol types and configuration | [Technical reference](docs/reference.md) |
| Submit or correct measurements | [Results contribution guide](https://github.com/spatial-bench/spatial-bench-results/blob/main/CONTRIBUTING.md) |
| Change public explanations or the explorer | [Web contribution guide](https://github.com/spatial-bench/spatial-bench-web/blob/main/CONTRIBUTING.md) |

Before proposing a change, trace the affected contract into its consumers. An
engine-only vocabulary addition does not implement a workload in a driver.
Conversely, an adapter cannot introduce a shared query or dataset name without
core vocabulary support. Link coordinated PRs and state the order in which their
compatible versions must become available.

A review should include the problem and resulting behavior, relevant unit or
reference checks, exact commands and source revisions for a small run when
measurement changes, and any compatibility implications. Registration
`conform` checks compile-time coverage; it does not verify query answers.
For semantic changes, include a known-answer or reference comparison outside
the timed region.

Check documentation impact when changing measurement, tags, manifests,
distributions, query semantics, result provenance, publication, or chart
interpretation. Update the owning task guide and link to the public
[methodology](https://spatial-bench.org/methodology)
for statistical interpretation. Use conventional commit subjects such as
`docs: explain catalog discovery`; commit subjects are linted in CI.
