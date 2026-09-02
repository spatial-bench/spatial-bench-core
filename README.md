# spatial-bench

Tag-addressed, library-agnostic benchmarking for spatial indexes.

A benchmark data point is identified by its **tags** and nothing else — no name
is ever parsed to recover what was measured. One selector expression both
chooses which cases run and pins which axes are swept:

```
spatial-bench run --select 'impl=kiddo_v6,query=exact_nn,axis=f64,k=1|5|20,tree_size=2^20..2^26'
```

`spatial-bench` with no arguments opens an interactive picker that offers only
values which keep the selection non-empty, then prints the non-interactive
command above so any run is reproducible in CI.

## Two things shape the design

**Every subject is vendored — in the bencher repo.** Manifests, drivers and
shims for every library under test live in
[spatial-bench-benchers](https://github.com/sdd/spatial-bench-benchers), a
separate reviewed catalog, including for libraries whose authors maintain this
one. A subject that declares what is measured about itself can flatter itself;
a comparison whose subjects wrote their own rules cannot be shown to be fair.
The cost is that the catalog lags its subjects by a review cycle. 
relocated the catalog out of this engine, which now carries no subject
knowledge at all: point `--subjects` (or `SPATIAL_BENCH_SUBJECTS`) at a
bencher checkout.

**The engine owns the harness.** Measurement methodology — point generation,
seeds, what sits inside the timed region — is the same code for every subject.
A library contributes its public API and nothing else, so a library that has
never heard of this project is as measurable as one that has.

See [docs/design.md](docs/design.md).

## Status

Working: catalog loading, the selector language, the interactive picker,
`list` / `describe` / `subjects` / `conform`, machine fingerprinting,
two-phase driver generation with cached builds, executing runs (criterion and
perf runners), run documents under `~/.local/share/spatial-bench/runs/`.

Not yet implemented: charting, `submit`, exec drivers for non-rust subjects
(python, cxx), and dataset submission.
