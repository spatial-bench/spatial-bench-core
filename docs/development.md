# Set up core development

Core builds the runner that executes benchmarks; the bencher catalog beside it
supplies the library manifests and drivers that runner launches. This guide sets
up both checkouts, builds the two binaries the engine needs, and points the CLI at
the catalog. The commands assume Linux, which is the environment the project
actually benchmarks on.

## Prerequisites

You will need Git, Rust installed through rustup, a native build toolchain, and
Python 3 for the inspection examples later in the guides. If you also build the
workspace charting crate, install fontconfig development headers and pkg-config;
on Debian and Ubuntu those come from `libfontconfig1-dev` and `pkg-config`.
Expect the first build to spend some time downloading dependencies and library
sources.

Engine development should use stable Rust, but generated drivers do not
necessarily run on the same toolchain: each selects one from the minimum versions
declared by its libraries. The kdtree example used throughout these guides needs
Rust 1.89.0, so install it now:

```sh
rustup toolchain install 1.89.0
```

## Build the tools

From whichever directory you keep project checkouts in, clone both repositories
and build the runner and generator:

```sh
git clone https://github.com/spatial-bench/spatial-bench-core.git spatial-bench
git clone https://github.com/spatial-bench/spatial-bench-benchers.git
cd spatial-bench
cargo build --release --bin spatial-bench --bin spatial-bench-dataset
export PATH="$PWD/target/release:$PATH"
```

Both binaries need to stay reachable. Drivers call `spatial-bench-dataset` to
obtain their construction and query points, and the runner looks for it first
beside its own executable and then through `PATH`. If you have set a custom Cargo
target directory, put that directory's `release` folder on `PATH` instead.

## Select the catalog

The CLI needs to be told where the bencher checkout is. From the core root:

```sh
export SPATIAL_BENCH_BENCHERS="$(cd ../spatial-bench-benchers && pwd)"
export SPATIAL_BENCH_SUBJECTS="$SPATIAL_BENCH_BENCHERS/subjects"
export SPATIAL_BENCH_ENGINE_SRC="$PWD"
spatial-bench subjects
spatial-bench describe > /tmp/spatial-bench-catalog.json
```

`subjects` prints each library with its pin and case count, while `describe`
exports the whole catalog as JSON. The CLI reads `SPATIAL_BENCH_SUBJECTS`, and
core's own tests read `SPATIAL_BENCH_BENCHERS`; without those overrides both fall
back to the sibling checkout shown above. `SPATIAL_BENCH_ENGINE_SRC` tells
generated drivers which engine checkout to compile against. If you keep the
repositories somewhere other than siblings, export all three to match your layout.

## Run development checks

From the core root, with the catalog still selected:

```sh
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
RUSTDOCFLAGS='--deny broken_intra_doc_links' cargo doc --workspace --no-deps --document-private-items
```

If you edited a root TOML file, also run `taplo format --check ./*.toml` with
taplo installed. Note that the workspace checks skip the driver crates and
`spatial-bench-measure`; changes to the contracts they implement still need the
relevant [bencher checks](https://github.com/spatial-bench/spatial-bench-benchers/blob/master/CONTRIBUTING.md)
and a [small driver run](running-benchmarks.md).