# Contributing to core

Contribute engine changes here: benchmark selection, input generation,
measurement and result handling. For library adapters and source pins, follow the
[bencher contribution guide](https://github.com/spatial-bench/spatial-bench-benchers/blob/master/CONTRIBUTING.md).

## Choose a starting point

- [Set up development](docs/development.md) and [run a small benchmark](docs/running-benchmarks.md).
- Read [engine architecture](docs/design.md) to follow a selection into a run document.
- [Add a dataset](docs/adding-datasets.md) or [add a query type](docs/adding-query-types.md).
- Use the [technical reference](docs/reference.md) to locate protocols and configuration.
- Follow the [results guide](https://github.com/spatial-bench/spatial-bench-results/blob/main/CONTRIBUTING.md) to submit or correct measurements.

## Prepare a change for review

Describe the resulting behavior and provide commands that verify it. Run the
[development checks](docs/development.md#run-development-checks), plus a small
driver run when inputs or measurement change. The extension guides specify the
additional correctness evidence for datasets and queries.

Identify affected drivers and link coordinated bencher PRs. If a protocol changes,
state which consumers need updating and how incompatible versions are handled.
Update the affected task guide and, when interpretation changes, the public
[methodology](https://spatial-bench.org/methodology).

Use a conventional commit subject, for example `docs: explain benchmark setup`.
