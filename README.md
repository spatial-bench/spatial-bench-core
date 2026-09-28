# spatial-bench

spatial-bench benchmarks spatial-index libraries against a shared set of
workloads. Each library release is pinned, built by a small adapter, and measured
on generated points; the resulting record keeps the source revision and machine
details that produced it, so a published number can be traced back to the run it
came from.

If you are here to use the published comparisons rather than produce them, the
[reading guide](https://spatial-bench.org/guide) works through one of them and the
[results explorer](https://spatial-bench.org/explore) lets you build your own. The
[methodology](https://spatial-bench.org/methodology) describes what each value
measures and what its reported bounds actually refer to.

## Run a benchmark

[Set up the engine and catalog](docs/development.md), then
[run a small nearest-neighbour benchmark](docs/running-benchmarks.md) to see the
whole cycle end to end: choosing a workload, previewing the build, and reading the
result document it writes.

## Contribute

Engine work, which covers benchmark selection, input generation, measurement and
result handling, begins in [CONTRIBUTING.md](CONTRIBUTING.md). Library adapters and
version updates belong to the
[bencher contribution guide](https://github.com/spatial-bench/spatial-bench-benchers/blob/master/CONTRIBUTING.md).

## Repositories

The project is split across four repositories, each owning one stage of the path
from a library API to a published chart:

| Repository | Responsibility |
| --- | --- |
| [Core](https://github.com/spatial-bench/spatial-bench-core) | Engine, input generator, measurement contracts and collation |
| [Benchers](https://github.com/spatial-bench/spatial-bench-benchers) | Library manifests, source pins, drivers and corpus selections |
| [Results](https://github.com/spatial-bench/spatial-bench-results) | Run records and published database snapshots |
| [Web](https://github.com/spatial-bench/spatial-bench-web) | Explorer and public documentation |