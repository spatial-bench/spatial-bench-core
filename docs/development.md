# Set up core development

Linux is the verified benchmarking environment: hardware probing uses Linux
interfaces, perf requires Linux facilities, and the Rust measurement helper
redirects Unix file descriptors. Compilation elsewhere does not establish a
working benchmark environment.

## Prerequisites and checkout layout

Install Git, a native C/C++ build toolchain, and Rust through rustup. The
workspace declares Rust 1.89.0 as its minimum; current stable is the normal
engine development toolchain. Generated Rust drivers resolve a separate
version from the selected manifests' highest `min_rustc` (or `run --rustc`).
For the example below, install `rustup toolchain install 1.89.0`.
Network access is needed for initial dependencies and pinned library sources.
Python 3 is used by the small output-inspection examples.

For workspace charting builds on Debian/Ubuntu, install `pkg-config` and
`libfontconfig1-dev`; CI installs these too. C++/Python library builds have
additional requirements documented in the
[bencher guides](https://github.com/spatial-bench/spatial-bench-benchers/blob/master/CONTRIBUTING.md).

From the parent directory in which you want the two checkouts:

```sh
git clone https://github.com/spatial-bench/spatial-bench-core.git
git clone https://github.com/spatial-bench/spatial-bench-benchers.git
cd spatial-bench-core
export SPATIAL_BENCH_BENCHERS="$(cd ../spatial-bench-benchers && pwd)"
export SPATIAL_BENCH_SUBJECTS="$SPATIAL_BENCH_BENCHERS/subjects"
cargo build --release --bin spatial-bench --bin spatial-bench-dataset
export PATH="$PWD/target/release:$PATH"
spatial-bench subjects
```

Both binaries are needed. Drivers find `spatial-bench-dataset` beside the CLI,
or invoke it by name through `PATH`. Building or installing only the CLI does
not install the generator. If using `CARGO_TARGET_DIR`, put its `release`
directory on `PATH` instead. To install both from the core repository:

```sh
cargo install --path crates/spatial-bench-cli --locked
cargo install --path crates/spatial-bench-dataset --locked
```

An installed CLI still needs the catalog. Set `SPATIAL_BENCH_ENGINE_SRC` to the
absolute core checkout to use its local core/measurement code in generated
Rust drivers. Local development builds discover their compiled-in checkout.

## Catalog discovery

The CLI uses `--subjects DIR`, then `SPATIAL_BENCH_SUBJECTS`, then
`spatial-bench-benchers/subjects` beside the discovered engine checkout.
`DIR` contains subject directories, each with a `subject.toml`.
There is no automatic catalog download. Explicit paths make worktrees and
unconventional layouts unambiguous:

```sh
spatial-bench --subjects "$SPATIAL_BENCH_BENCHERS/subjects" subjects
spatial-bench describe > /tmp/spatial-bench-catalog.json
```

Core tests have a separate discovery helper: `SPATIAL_BENCH_BENCHERS` points to
the **repository root**, not `subjects/`. Keep both exports in this guide when
working outside the conventional sibling layout. A test failure saying `no
bencher catalog found` indicates missing test inputs.

## Checks

From the core root, with the catalog exports above:

```sh
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
RUSTDOCFLAGS='--deny broken_intra_doc_links' cargo doc --workspace --no-deps --document-private-items
```

`spatial-bench-measure` and bencher driver crates are outside the workspace
member list, so these commands do not cover their tests. Generated Rust drivers
compile the local measurement crate through an engine path override. Changes
to that contract also need the relevant bencher checks and a small
[run](running-benchmarks.md). `conform --subject kdtree` builds both registered
scalar variants, so it costs more than the single-variant example run.

If editing root TOML files, CI also runs `taplo format --check ./*.toml`.
The repository's `just` recipes are conveniences, but include a separate Kiddo
driver check and assume a catalog path; use the explicit commands above for
engine development. They do not require a privileged machine fingerprint.
