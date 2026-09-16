# Run and inspect a benchmark

This example measures exact nearest-one queries in kdtree using 65,536 uniform
3D points and 100 query points. Complete [development setup](development.md)
first, then run the commands from the core root in the same shell.

## Select one workload

Use a fresh output directory so this example's result is easy to find:

```sh
export XDG_DATA_HOME="$(mktemp -d /tmp/spatial-bench-example.XXXXXX)"
export SPATIAL_BENCH_EXAMPLE='impl=kdtree,query=exact_nn,axis=f64,k=1,tree_size=2^16,query_count=100'
spatial-bench list --select "$SPATIAL_BENCH_EXAMPLE" --format json
```

The output should contain one tag map, including `dims=3`,
`metric=squared_euclidean` and `dataset=uniform`. These values come from the
manifest. The selector pins the tree size and query count; omitted runtime
parameters use their defaults. See [selector syntax](reference.md#selectors)
for alternatives and ranges.

## Preview and run

```sh
spatial-bench run --select "$SPATIAL_BENCH_EXAMPLE" --dry-run
spatial-bench run --select "$SPATIAL_BENCH_EXAMPLE" --random-seed 42 --allow-unfingerprinted
```

The plan reports one build combination and Rust 1.89.0. On first execution,
the runner builds the driver against the pinned kdtree release. Measurement
requests 3 seconds of warm-up, 5 seconds of collection and 30 samples; allow
additional time for compilation and analysis.

`--allow-unfingerprinted` allows this local check on a host without a captured
machine fingerprint. It does not bypass a fingerprint mismatch. For measurements
intended for publication, follow the
[results contribution guide](https://github.com/spatial-bench/spatial-bench-results/blob/main/CONTRIBUTING.md).

A dry run prepares drivers. For C++ and Python this can download sources,
compile code or create a virtual environment. Use `list` for catalog-only
inspection.

## Inspect the result

The runner prints the JSON document path under
`$XDG_DATA_HOME/spatial-bench/runs/YYYY-MM/`. Inspect it with Python:

```sh
python3 - <<'PYCODE'
import json, os
from pathlib import Path
root = Path(os.environ['XDG_DATA_HOME']) / 'spatial-bench' / 'runs'
paths = list(root.glob('*/*.json'))
assert len(paths) == 1, paths
doc = json.loads(paths[0].read_text())
assert len(doc['points']) == 1
point = doc['points'][0]
assert point['tags']['tree_size'] == 65536
assert point['metrics']['latency_ns']['point'] > 0
print('file:', paths[0])
print('subject:', doc['run']['subjects']['kdtree'])
print('machine:', doc['run']['machine_hash'])
print('toolchain:', doc['run']['toolchain'])
print('latency:', point['metrics']['latency_ns'])
print('statistics:', point.get('stats'))
PYCODE
```

The driver builds the index before timing, then measures repeated passes over
the 100 queries. Estimates are divided by 100 to obtain nanoseconds per query.
`latency_ns` contains the mean and its confidence bounds; `stats.median_ns` is
separate. The [methodology](https://spatial-bench.org/methodology) explains how
these relate to the plotted values.

Retain the command, seed and both checkout revisions with the result. The current
run document omits the seed and complete engine/catalog revisions. See the
[provenance guide](https://github.com/spatial-bench/spatial-bench-results/blob/main/docs/format-and-provenance.md)
for the recorded fields and their interpretation.

## Continue the experiment

Change `tree_size=2^16` to `tree_size=2^16..2^18` and inspect the expanded
selection before running it. To check an adapter's compile-time registrations,
run `spatial-bench conform --subject kdtree`; this builds both scalar variants
without testing query answers.

`--runner perf` adds Linux process counters and requires perf access on the
host. Those counters cover the complete driver process, including setup and
warm-up. Their scope differs from the query latency timer.
