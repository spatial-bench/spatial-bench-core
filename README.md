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

**Every subject is vendored.** Manifests and harnesses for every library under
test live in this repository, including for libraries whose authors maintain
this one. A subject that declares what is measured about itself can flatter
itself; a comparison whose subjects wrote their own rules cannot be shown to be
fair. The cost is that this repository lags its subjects by a review cycle.

**The engine owns the harness.** Measurement methodology — point generation,
seeds, what sits inside the timed region — is the same code for every subject.
A library contributes its public API and nothing else, so a library that has
never heard of this project is as measurable as one that has.

See [docs/design.md](docs/design.md).

## Status

Working: catalog loading, the selector language, the interactive picker,
`list` / `describe` / `subjects`, machine hashing, result paths.

Not yet implemented: the drivers themselves, code generation, executing a run,
hardware fingerprinting, and dataset submission.
