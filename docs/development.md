# Set up core development

Clone core and the bencher catalog beside each other. Core builds the runner;
the catalog supplies the library manifests and drivers it executes. The commands
below use Linux, the verified benchmarking environment.

## Prerequisites

Install Git, Rust through rustup, a native build toolchain and Python 3 for the
inspection examples. Workspace charting builds also need fontconfig development
headers and pkg-config; on Debian or Ubuntu these packages are
`libfontconfig1-dev` and `pkg-config`. Initial builds download dependencies and
library sources.

Use stable Rust for engine development. Generated drivers select a toolchain
from their libraries' declared minimum versions; the kdtree example needs
Rust 1.89.0:

```sh
rustup toolchain install 1.89.0
```

## Build the tools

From a directory in which you keep project checkouts:

```sh
git clone https://github.com/spatial-bench/spatial-bench-core.git spatial-bench
git clone https://github.com/spatial-bench/spatial-bench-benchers.git
cd spatial-bench
cargo build --release --bin spatial-bench --bin spatial-bench-dataset
export PATH="$PWD/target/release:$PATH"
```

Keep both binaries available. Drivers invoke `spatial-bench-dataset` to obtain
construction and query points. The runner looks for it beside its executable,
then through PATH. If you use a custom Cargo target directory, put its `release`
directory on PATH instead.

## Select the catalog

From the core root:

```sh
export SPATIAL_BENCH_BENCHERS="$(cd ../spatial-bench-benchers && pwd)"
export SPATIAL_BENCH_SUBJECTS="$SPATIAL_BENCH_BENCHERS/subjects"
export SPATIAL_BENCH_ENGINE_SRC="$PWD"
spatial-bench subjects
spatial-bench describe > /tmp/spatial-bench-catalog.json
```

`subjects` lists libraries with their pins and case counts; `describe` exports
the catalog as JSON. The CLI reads `SPATIAL_BENCH_SUBJECTS`; core tests read
`SPATIAL_BENCH_BENCHERS`. Without those overrides, both look for the sibling
checkout shown above. `SPATIAL_BENCH_ENGINE_SRC` selects this checkout's engine
code for generated drivers. Keep the exports when using another directory layout.

## Run development checks

From the core root with the catalog selected:

```sh
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
RUSTDOCFLAGS='--deny broken_intra_doc_links' cargo doc --workspace --no-deps --document-private-items
```

For root TOML edits, also run `taplo format --check ./*.toml` with taplo installed.
These workspace checks exclude driver crates and `spatial-bench-measure`.
Changes to their contracts also need the relevant
[bencher checks](https://github.com/spatial-bench/spatial-bench-benchers/blob/master/CONTRIBUTING.md)
and a [small driver run](running-benchmarks.md).
