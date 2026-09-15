# Technical reference

Verified against core `2104bfe` on 2026-09-14. Use source definitions for full
field lists; this index records the interfaces most likely to affect a change.

## Source map

| Interface | Definition and behavior |
| --- | --- |
| Manifest schema 1 | [manifest.rs](../crates/spatial-bench-core/src/manifest.rs), validation/lowering in [catalog_load.rs](../crates/spatial-bench-core/src/catalog_load.rs) |
| Shared tags and subject namespaces | [vocab.rs](../crates/spatial-bench-core/src/vocab.rs) |
| Selector grammar | [selector.rs](../crates/spatial-bench-core/src/selector.rs) |
| Parameter domains, expansion, defaults | [case.rs](../crates/spatial-bench-core/src/case.rs) |
| Driver generation and build inputs | [codegen.rs](../crates/spatial-bench-core/src/codegen.rs), [resolve.rs](../crates/spatial-bench-core/src/resolve.rs), [build.rs](../crates/spatial-bench-core/src/build.rs), [exec.rs](../crates/spatial-bench-core/src/exec.rs) |
| Harness version 2 | [harness.rs](../crates/spatial-bench-core/src/harness.rs): `RunSpec`, `CaseSpec`, `Budget`, JSON-lines framing and registration listing |
| Dataset stream version 1 | [dataset main.rs](../crates/spatial-bench-dataset/src/main.rs): header, dtype, native-endian payload, generation |
| Rust measurement and conversion | [measure lib.rs](../crates/spatial-bench-measure/src/lib.rs) |
| Run schema version 1 | [schema.rs](../crates/spatial-bench-core/src/schema.rs): `Document`, `Run`, `Point`, metrics and provenance |
| Host identity and context | [fingerprint.rs](../crates/spatial-bench-core/src/fingerprint.rs), [machine.rs](../crates/spatial-bench-core/src/machine.rs), [context.rs](../crates/spatial-bench-core/src/context.rs) |
| SQLite projection and CLI | [CLI main.rs](../crates/spatial-bench-cli/src/main.rs), particularly `cmd_publish` |

Manifest, harness, dataset-stream and result-schema versions describe different
contracts. Their current numbers need not match. Registration format details
and library implementation guidance live in the
[bencher driver contract](https://github.com/spatial-bench/spatial-bench-benchers/blob/master/docs/driver-contract.md).

## Selector syntax

Always quote selectors in a shell.

| Expression | Meaning |
| --- | --- |
| `impl=kdtree,axis=f64` | Both clauses must match |
| `k=1\|5\|20` | Any listed value for this key |
| `tree_size=2^16..2^18` | Inclusive range, constrained by the declared parameter domain |
| `kiddo.stem=*` | Key must exist |
| `!kiddo.stem=eytzinger` | Key must exist and not equal this value |
| Repeated `--select` | Union of the selections |

Ranges do not turn a declared powers-of-two domain into every integer between
its endpoints. Omitted parameters use defaults. Fixed matrix axes can still
select many cases when unconstrained. `list --format json` displays resolved
point tags; `describe` emits the catalog. Recognized vocabulary, manifest cases,
driver coverage and published results are distinct sets.

## Paths and environment

| Setting | Meaning |
| --- | --- |
| `--subjects` / `SPATIAL_BENCH_SUBJECTS` | Directory containing subject directories; CLI catalog override |
| `SPATIAL_BENCH_BENCHERS` | Bencher repository root used by core tests |
| `--engine-src` / `SPATIAL_BENCH_ENGINE_SRC` | Engine checkout for local driver dependencies |
| `--subject-path NAME=DIR` / `SPATIAL_BENCH_SUBJECT_PATHS` | Local subject checkout override; marks run as a working-tree experiment |
| `--build-dir` / `SPATIAL_BENCH_BUILD_DIR` | Generated driver output root; defaults to the data root's `builds/` |
| `XDG_DATA_HOME` | Data parent; default `~/.local/share`, with engine output under `spatial-bench/` |
| `SPATIAL_BENCH_FINGERPRINT` | Machine fingerprint file override; default `/etc/spatial-bench/fingerprint.toml` |
| `SPATIAL_BENCH_CHART` | Override path to the charting companion executable |

The generator is found beside the CLI, then through `PATH`. Keep both available.
`spatial-bench --help` and `spatial-bench COMMAND --help` are authoritative for
current CLI options. `chart` forwards arguments to `spatial-bench-chart`, built
from the optional workspace charting package. `publish` collates locally;
remote publication is a [results-repository workflow](https://github.com/spatial-bench/spatial-bench-results/blob/main/docs/publication.md).
