# Technical reference

## Selectors

Quote selectors so the shell preserves special characters.

| Expression | Meaning |
| --- | --- |
| `impl=kdtree,axis=f64` | Both clauses match |
| `k=1\|5\|20` | Any listed value |
| `tree_size=2^16..2^18` | Inclusive range within the declared parameter domain |
| `kiddo.stem=*` | Key exists |
| `!kiddo.stem=eytzinger` | Key exists and differs from this value |
| Repeated `--select` | Union of selections |

Omitted runtime parameters use defaults; omitted fixed matrix axes can select
several cases. A range intersects the declared domain: a powers-of-two domain
still yields powers of two. Inspect the result with `list --format json`.

## Paths and environment

| Setting | Value |
| --- | --- |
| `--subjects` / `SPATIAL_BENCH_SUBJECTS` | Catalog's `subjects/` directory |
| `SPATIAL_BENCH_BENCHERS` | Bencher repository root, for core tests |
| `--engine-src` / `SPATIAL_BENCH_ENGINE_SRC` | Engine checkout used by generated drivers |
| `--subject-path NAME=DIR` | Local upstream checkout instead of its source pin |
| `--build-dir` / `SPATIAL_BENCH_BUILD_DIR` | Generated driver/build directory |
| `XDG_DATA_HOME` | Data parent; defaults to `~/.local/share` |
| `SPATIAL_BENCH_FINGERPRINT` | Fingerprint path; defaults to `/etc/spatial-bench/fingerprint.toml` |
| `SPATIAL_BENCH_CHART` | Chart companion executable |

Run documents use `<data parent>/spatial-bench/runs/YYYY-MM/`; generated builds
default to `<data parent>/spatial-bench/builds/`. Subject-path runs are marked
as local checkout experiments. `run --rustc VERSION` overrides the selected Rust
toolchain, subject to manifest minimums. The CLI has no measurement-budget
override. Use `spatial-bench COMMAND --help` for available options.

## Source contracts

| Contract | Definition |
| --- | --- |
| Manifest schema 1 | [manifest.rs](../crates/spatial-bench-core/src/manifest.rs), [validation](../crates/spatial-bench-core/src/catalog_load.rs) |
| Vocabulary and parameters | [vocab.rs](../crates/spatial-bench-core/src/vocab.rs), [case.rs](../crates/spatial-bench-core/src/case.rs) |
| Harness version 2 | [harness.rs](../crates/spatial-bench-core/src/harness.rs) |
| Dataset stream version 1 | [generator](../crates/spatial-bench-dataset/src/main.rs) |
| Result schema 1 | [schema.rs](../crates/spatial-bench-core/src/schema.rs) |
| Rust measurement | [measurement crate](../crates/spatial-bench-measure/src/lib.rs) |
| Execution and collation | [run.rs](../crates/spatial-bench-core/src/run.rs), CLI `cmd_publish` in [main.rs](../crates/spatial-bench-cli/src/main.rs) |

These interfaces were checked against core `2104bfe` and benchers `1972c50`.
Recheck version numbers when modifying their definitions.
