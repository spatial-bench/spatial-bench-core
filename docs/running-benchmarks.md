# Run and inspect a benchmark

This walkthrough runs one small case and then reads the document it writes: exact
nearest-neighbour queries against kdtree, using 65,536 uniformly distributed
three-dimensional points and 100 query points. Complete
[development setup](development.md) first, and run the commands below from the
core root in the same shell.

## Select one workload

Point the run at a fresh data directory so its output is easy to find later, then
ask the catalog what the selector matches:

```sh
export XDG_DATA_HOME="$(mktemp -d /tmp/spatial-bench-example.XXXXXX)"
export SPATIAL_BENCH_EXAMPLE='impl=kdtree,query=exact_nn,axis=f64,k=1,tree_size=2^16,query_count=100'
spatial-bench list --select "$SPATIAL_BENCH_EXAMPLE" --format json
```

You should get back a single tag map containing `dims=3`,
`metric=squared_euclidean` and `dataset=uniform`, all of which come from the
manifest rather than the selector. The selector pins the tree size and query
count; any runtime parameter you leave out falls back to its default. The
[selector syntax](reference.md#selectors) reference covers value lists, ranges and
the other expression forms.

## Preview and run

```sh
spatial-bench run --select "$SPATIAL_BENCH_EXAMPLE" --dry-run
spatial-bench run --select "$SPATIAL_BENCH_EXAMPLE" --random-seed 42 --allow-unfingerprinted
```

The plan should report one build combination using Rust 1.89.0. On the first
execution the runner compiles the driver against the pinned kdtree release, so
budget time for that as well as the measurement itself, which asks for three
seconds of warm-up, five seconds of collection and 30 samples.

`--allow-unfingerprinted` exists so this local check can run on a host that has no
captured machine fingerprint; it does not let a run proceed past a fingerprint
*mismatch*. If you intend the measurement for publication, follow the
[results contribution guide](https://github.com/spatial-bench/spatial-bench-results/blob/main/CONTRIBUTING.md)
instead.

A dry run is not purely informational for executable drivers: it may prepare
their environment, which for C++ and Python can mean downloading sources,
compiling code or creating a virtual environment. Use `list` when you want to
inspect the catalog without any preparation.

## Inspect the result

The runner prints the path of the JSON document it wrote under
`$XDG_DATA_HOME/spatial-bench/runs/YYYY-MM/`. Load it with Python:

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

The driver builds the index before timing starts, then repeatedly walks all 100
queries and divides each batch estimate by 100 to get nanoseconds per query. That
means `latency_ns` holds a mean with its confidence bounds, while `stats.median_ns`
is a separate statistic; the [methodology](https://spatial-bench.org/methodology)
explains how each one relates to what the chart plots.

Keep the command, the seed and both checkout revisions alongside the result. Note
that the run document as it currently stands does not record the seed or the
complete engine and catalog revisions, so those have to be preserved elsewhere;
the [provenance guide](https://github.com/spatial-bench/spatial-bench-results/blob/main/docs/format-and-provenance.md)
covers which fields are stored and how to interpret them.

## Continue the experiment

Widen the range to `tree_size=2^16..2^18` and inspect the expanded selection with
`list` before running it. To check that an adapter registers the combinations its
manifest claims, run `spatial-bench conform --subject kdtree`; this builds both
scalar variants but does not check that the queries return correct answers.

Adding `--runner perf` brings in Linux process counters and requires perf access on
the host. Those counters cover the entire driver process, including setup and
warm-up, so their scope is not the same as the timer around the query loop.