# spatial-bench — tag-addressed, library-agnostic spatial-index benchmarking

# Day 1.5: the catalog lives in the bencher repo (a sibling checkout).
# Override here if your checkout lives elsewhere.
export SPATIAL_BENCH_SUBJECTS := env_var_or_default('SPATIAL_BENCH_SUBJECTS', env_var('HOME') / 'projects' / 'spatial-bench-benchers' / 'subjects')

default:
    @just --list

# Interactive benchmark picker, then runs what you selected. Subjects build
# from their pinned refs by default; --subject-path points one at a checkout
# for development.
bench *ARGS:
    cargo run --release --bin spatial-bench -- {{ARGS}}

# Cases matching a selector, without running anything.
list *ARGS:
    cargo run --quiet --bin spatial-bench -- list {{ARGS}}

# Every vendored subject, its pinned ref and case count.
subjects:
    cargo run --quiet --bin spatial-bench -- subjects

test:
    # The driver crate lives in the bencher repo now (day 1.5), excluded from
    # the engine workspace — so it needs its own line, or the most API-fragile
    # crate (the macro names kiddo's types) has no coverage.
    cargo test --workspace
    cd {{SPATIAL_BENCH_SUBJECTS}}/kiddo/driver && cargo test

lint:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets -- -D warnings
    # Same exclusion, same reason.
    cd {{SPATIAL_BENCH_SUBJECTS}}/kiddo/driver && cargo fmt -- --check
    cd {{SPATIAL_BENCH_SUBJECTS}}/kiddo/driver && cargo clippy --all-targets -- -D warnings

fmt:
    cargo fmt --all
    cd {{SPATIAL_BENCH_SUBJECTS}}/kiddo/driver && cargo fmt
    taplo format

# Install the CLI so other projects can drive it from their own justfiles.
install:
    cargo install --path crates/spatial-bench-cli --locked
