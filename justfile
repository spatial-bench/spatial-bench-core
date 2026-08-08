# spatial-bench — tag-addressed, library-agnostic spatial-index benchmarking

default:
    @just --list

# Interactive benchmark picker.
bench *ARGS:
    cargo run --release --bin spatial-bench -- {{ARGS}}

# Cases matching a selector, without running anything.
list *ARGS:
    cargo run --quiet --bin spatial-bench -- list {{ARGS}}

# Every vendored subject, its pinned ref and case count.
subjects:
    cargo run --quiet --bin spatial-bench -- subjects

test:
    cargo test --workspace

lint:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets -- -D warnings

fmt:
    cargo fmt --all
    taplo format

# Install the CLI so other projects can drive it from their own justfiles.
install:
    cargo install --path crates/spatial-bench-cli --locked
