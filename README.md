# spatial-bench

spatial-bench measures spatial-index libraries on specified workloads. The engine
builds pinned libraries, runs their benchmark drivers and records results with
machine and source information.

To explore published measurements, start with the
[reading guide](https://spatial-bench.org/guide) and
[results explorer](https://spatial-bench.org/explore). The
[methodology](https://spatial-bench.org/methodology) defines the measurements.

## Run a benchmark

[Set up the engine and catalog](docs/development.md), then
[run a small nearest-neighbour benchmark](docs/running-benchmarks.md). The worked
example covers selecting a workload, previewing its build and inspecting the
result document.

## Contribute

Use [CONTRIBUTING.md](CONTRIBUTING.md) for engine development, including new
input distributions and query types. For library adapters or version updates,
use the [bencher contribution guide](https://github.com/spatial-bench/spatial-bench-benchers/blob/master/CONTRIBUTING.md).

## Repositories

| Repository | Responsibility |
| --- | --- |
| [Core](https://github.com/spatial-bench/spatial-bench-core) | Engine, input generator, measurement contracts and collation |
| [Benchers](https://github.com/spatial-bench/spatial-bench-benchers) | Library manifests, source pins, drivers and corpus selections |
| [Results](https://github.com/spatial-bench/spatial-bench-results) | Run records and published database snapshots |
| [Web](https://github.com/spatial-bench/spatial-bench-web) | Explorer and public documentation |
