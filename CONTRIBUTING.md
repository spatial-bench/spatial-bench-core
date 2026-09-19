# Contributing to core

This repository holds the engine: how benchmarks are selected, how inputs are
generated, how measurements are taken and how results are handled. If your change
concerns a library adapter or a source pin rather than the engine itself, it
belongs in the [benchers repository](https://github.com/spatial-bench/spatial-bench-benchers/blob/master/CONTRIBUTING.md)
instead.

## Where to start

New contributors usually begin with [development setup](docs/development.md) and
the [small benchmark walkthrough](docs/running-benchmarks.md), which together get
a working engine and one measured case in front of you. Once that works,
[engine architecture](docs/design.md) follows a selection through to the run
document it produces.

Task-specific guides cover the two most common engine extensions:
[adding a dataset](docs/adding-datasets.md) and
[adding a query type](docs/adding-query-types.md). The
[technical reference](docs/reference.md) lists selector syntax, environment
variables and the source files that define each contract. To submit or correct
the measurements you produce, use the
[results contribution guide](https://github.com/spatial-bench/spatial-bench-results/blob/main/CONTRIBUTING.md).

## Preparing a change for review

A reviewer needs to know what behaviour changes and how you verified it, so
describe the resulting behaviour and give commands that demonstrate it. Run the
[development checks](docs/development.md#run-development-checks), and add a small
driver run whenever your change touches inputs or measurement; the extension
guides list the extra correctness evidence expected for datasets and queries.

If your change affects other repositories, say which drivers or manifests are
implicated and link the coordinated bencher PR. Protocol changes need more: state
which consumers must be updated and how incompatible protocol versions will be
rejected. Update the relevant task guide, and the public
[methodology](https://spatial-bench.org/methodology) when the interpretation of a
measurement changes.

Commit subjects follow the conventional form, for example
`docs: explain benchmark setup`.