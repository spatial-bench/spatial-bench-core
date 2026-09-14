# spatial-bench

spatial-bench measures spatial-index libraries with explicit workload tags,
reviewed library adapters, and recorded run provenance. This repository contains
the engine, command-line tools, input generator, and result collation code.

**Understand the results:** start with the
[reading guide](https://spatial-bench.org/guide)
and [methodology](https://spatial-bench.org/methodology).
**Contribute:** start with [CONTRIBUTING.md](CONTRIBUTING.md).

## Run a small comparison

Follow [development setup](docs/development.md) to build the CLI and companion
generator and select the bencher catalog. Then inspect one workload:

```sh
spatial-bench list --select 'impl=kdtree,query=exact_nn,axis=f64,k=1,tree_size=2^16,query_count=100' --format tags
```

The same selector can be passed to `run`. The
[worked benchmark guide](docs/running-benchmarks.md) covers planning, execution,
output, and the distinction between a local check and a submission.

## Project organization

| Repository | Responsibility |
| --- | --- |
| [spatial-bench-core](https://github.com/spatial-bench/spatial-bench-core) | Catalog validation, selection, preparation, measurement contracts, input generation, collation |
| [spatial-bench-benchers](https://github.com/spatial-bench/spatial-bench-benchers) | Library manifests, source pins, drivers, Standard Corpus selections |
| [spatial-bench-results](https://github.com/spatial-bench/spatial-bench-results) | Reviewed run documents, machine records, published database snapshots |
| [spatial-bench-web](https://github.com/spatial-bench/spatial-bench-web) | Public explanations and the results explorer |

A case becomes a measured point when all its parameters are resolved. Its tag
map describes the workload; its run supplies the machine and software context.
Selectors filter cases and constrain parameter sweeps without parsing benchmark
names. See [architecture](docs/design.md) and [technical reference](docs/reference.md).

The implemented pipeline supports Rust drivers, C++ and Python exec drivers,
latency and Linux perf runs, registration conformance, local charting, result
submission, and SQLite collation. Coverage depends on the checked-out manifests
and drivers; published measurements are a further subset. Shared inputs and
contracts make comparisons auditable, but driver timing and statistical
procedures still require review, particularly across languages.
