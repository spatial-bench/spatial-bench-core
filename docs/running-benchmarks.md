# Run and inspect a benchmark

This example validates the local pipeline with one `kdtree` case: 65,536
uniform 3D `f64` construction points, 100 query points, exact nearest-one, and
squared Euclidean distance. Follow [development setup](development.md) first.
Commands below run from the core root with the catalog and `PATH` exports set.

## Inspect the selection and plan

```sh
export XDG_DATA_HOME="$(mktemp -d /tmp/spatial-bench-example.XXXXXX)"
export SPATIAL_BENCH_EXAMPLE='impl=kdtree,query=exact_nn,axis=f64,k=1,tree_size=2^16,query_count=100'
spatial-bench list --select "$SPATIAL_BENCH_EXAMPLE" --format json
spatial-bench run --select "$SPATIAL_BENCH_EXAMPLE" --dry-run
```

`list` resolves one complete tag map. The plan reports one case, one point,
one build combination, and rustc 1.89.0 with the reviewed kdtree manifest.
A case is a catalog entry; a point is one resolved parameter combination.
Omitted parameter axes take their defaults, not their whole domain.

A dry run prepares build inputs. For Rust this generates and materializes a
package; for exec drivers preparation can fetch sources, compile a C++ shim,
or create a Python environment. Use `list` or `describe` when you need catalog
inspection without driver preparation.

## Execute locally

```sh
spatial-bench run --select "$SPATIAL_BENCH_EXAMPLE" --random-seed 42 --allow-unfingerprinted
```

The first run builds pinned sources; subsequent compatible selections can reuse
build artifacts. The default budget requests 3 seconds of warm-up, 5 seconds
of measurement, and 30 samples. The displayed estimate excludes much of the
first build and analysis overhead; actual wall time can be longer. The CLI
currently has no budget override.

The example's driver generates inputs and builds the index before timing. Each
timed iteration runs all 100 queries and consumes results; the shared Rust
measurement helper normalizes estimates by 100. This is still a `single_query`
API workload: looping over probes for measurement does not make the library
call a bulk query API. Other operations and drivers need their own timing
boundary review.

`--allow-unfingerprinted` permits a local experiment when no fingerprint exists;
it does not bypass a stale fingerprint mismatch. Runs without a verified
fingerprint are unsuitable for submission. The temporary `XDG_DATA_HOME` keeps
these run documents separate from ordinary runs.

## Inspect output and retain provenance

The CLI prints the completed JSON document path. The layout is
`$XDG_DATA_HOME/spatial-bench/runs/YYYY-MM/*.json`; without the override,
`XDG_DATA_HOME` defaults to `~/.local/share`. Inspect the example without
assuming the generated filename:

```sh
python3 - <<'PY'
import json, os
from pathlib import Path
root = Path(os.environ['XDG_DATA_HOME']) / 'spatial-bench' / 'runs'
paths = sorted(root.glob('*/*.json'))
assert len(paths) == 1, paths
doc = json.loads(paths[0].read_text())
assert doc['schema_version'] == 1 and len(doc['points']) == 1
run, point = doc['run'], doc['points'][0]
assert point['tags']['tree_size'] == 65536
assert point['metrics']['latency_ns']['point'] > 0
print(paths[0])
for key in ('selectors', 'subjects', 'machine_hash', 'source', 'toolchain'):
    print(key, run[key])
print('metrics', point['metrics'])
print('stats', point.get('stats'))
PY
```

The stored `latency_ns` is the normalized **mean**; its bounds describe that
mean. `stats.median_ns` is separate. The explorer prefers the median when
available, so use the [methodology](https://spatial-bench.org/methodology)
to interpret chart values and bounds.

Retain the exact command, random seed, core and bencher Git revisions, local
changes, and build logs with an experiment. The current document does **not**
record the selected random seed or a complete engine/catalog revision: engine
`source.git_sha` is unset, and `git_dirty` marks subject-path overrides rather
than inspecting every checkout. A selector alone is insufficient to reproduce
a non-default seed. Subject provenance records the resolved library revision
or PyPI artifact digest, not every experimental input.

For adapter changes, also run:

```sh
spatial-bench conform --subject kdtree
```

This checks the generated driver's registration set against the manifest's
compile-time coordinates. It neither executes queries nor checks their answers.

## From a local check to a publishable result

A working small run is evidence that the pipeline works, not a representative
performance study. Select the intended corpus and environment deliberately,
review timing and correctness, and follow the
[results submission guide](https://github.com/spatial-bench/spatial-bench-results/blob/main/CONTRIBUTING.md)
for fingerprints, provenance and review. `submit` creates a results PR; it is
not part of this local example.

`--runner perf` additionally requires Linux perf and permission to read the
selected counters. Each point gets its own driver process. Counter totals
cover that process, including input generation, index setup, warm-up and
analysis; they are not counters confined to the timed query closure. Source
compilation occurs before the measured process. See the methodology before
comparing those counters to per-query latency.
