# spatial-bench — TODO

Scope split agreed 2026-08-29 and extended 2026-08-31: day one is the engine
running kiddo v6 end-to-end from pins with fingerprinting; day 1.5 is the
bencher split and the exec adapter; day two is charting, `submit` and
submission trust (perf and conform landed early). Detail and rationale for
review-driven items live in `REVIEW-ARCHITECTURE.md`, `REVIEW-DESIGN.md`,
`REVIEW-SECURITY.md` — this file is the plan, not the evidence.

## Day one

### Done

- [x] Machine fingerprinting (§9): unprivileged probe, privileged capture via
      dmidecode/decode-dimms, `/etc/spatial-bench/fingerprint.toml` with
      HMAC-SHA256 tamper-evidence checksum, run-gate validation,
      `--allow-unfingerprinted`, `fingerprint`/`machine` commands.
- [x] Criterion driver (§11): measurement in core, identical for every Rust
      subject; fd guard keeps criterion's stdout out of the JSON-lines
      contract; estimates + sample count converted to points.
- [x] Builds from pins (§4): kiddo as a cargo git dep (`tag`/`rev`), lockfile
      provenance (`subjects` block with version + sha in the run header),
      one-toolchain-per-run enforced via `cargo +<version>`.
- [x] CLI runs kiddo v6 end-to-end: selector → generate → build → drive → run
      document under `~/.local/share/spatial-bench/runs/YYYY-MM/`, with
      probed context (kernel/governor/SMT/boost), ULID run ids, real
      started/finished timestamps.
- [x] Review fixes B1–B4 (perf lie, `mem_total_bytes` in the fingerprint,
      timestamps, ULID). B5 waived: v1 has never left this machine.
- [x] A4: driver crate in `just test`/`just lint`; §13 drift-guard check 2 as
      `tests/compile_generated.rs` (caught the alpha.4/cyclic-SIMD manifest
      drift on first run; kiddo pinned to v6.1.0).
- [x] A5: `--engine-src`/`SPATIAL_BENCH_ENGINE_SRC` dev mode, registry-dep
      branch for installed binaries, dual dep on core in the driver crate,
      `driver_rev` in the build cache key.

### Remaining before day two

Ordered; A1/A2 are prerequisites for the day-two `exec`/`perf` work.

- [x] A1 — real adapter dispatch on `Case.adapter` (see REVIEW-ARCHITECTURE).
- [x] A2 — build/run pipeline moved to `spatial-bench-core::run`; the CLI is
      parsing, rendering, and exit codes.
- [x] A6 — machine hash widened to 10 base32 chars (50 bits); hash width and
      inputs documented as permanent; stale-generation captures get a
      dedicated error and are overridable by `--allow-unfingerprinted`.
      **Requires re-capturing this machine's fingerprint** (sudo).
- [x] A3 — criterion measurement in its own `spatial-bench-measure` crate;
      core is schema + catalog + selector again.
- [x] S1 — manifest `sha` field, refuse a moved tag at build time. Required
      before any submission is accepted. kiddo's manifest pins v6.1.0's sha.
- [x] D1–D6 — design touch-ups (selector round-trip validation, source-kind
      validation, range reporting in `unsatisfied`, budget refusal instead of
      clamping, remedy-naming error messages, picker runner row). See
      REVIEW-DESIGN.
- [x] S2–S6 — hardening (path validation, fd-guard invariants, absolute paths
      under sudo, fingerprint-source visibility, sudo/env-var warning). See
      REVIEW-SECURITY.
- [ ] CI: `just test`/`just lint` as the pipeline, including the drift guard.
      Blocked on repo push (below).

### Operational (not code)

- [x] Re-capture this machine's fingerprint with a current binary — done
      2026-08-31: 10-char hash, `mem_total_bytes` present, run gate validates
      without `--allow-unfingerprinted`.
- [ ] Push to remote — **blocked on explicit permission**.
- [ ] After push: enable CI, then treat `just lint` as the merge gate.

## Day 1.5 — the bencher split + the exec adapter

Decisions locked 2026-08-31: **one contract for all drivers** — every driver
(rust, cxx, python) speaks the harness contract (optional `--list`, RunSpec
JSON on stdin, JSONL points on stdout); the templated-command design is
dropped, which makes conform's `--list` and perf's single-case specs work for
exec subjects with no extra machinery. And **one bencher repo** to start
(`spatial-bench-benchers`), split per language only when CI friction actually
appears — self-contained subject dirs make a later split mechanical.

The trust model of §4 is relocated, not weakened: the bencher repo is the
reviewed catalog — manifests, drivers, shims; PR-gated; not the subjects' own
repos — and the engine becomes pure measurement machinery with zero subject
knowledge.

### Phase 1 — catalog migration (engine carries zero subjects)

- [x] Create the `spatial-bench-benchers` repo: same `subjects/` layout as the
      engine's today; every subject directory self-contained (subject.toml
      plus its driver assets beside it).
- [x] Move `subjects/` (kiddo, nanoflann manifests) into the bencher repo.
- [x] Move kiddo's driver crate (`spatial-bench-kiddo-v6`) next to its
      manifest; rust driver crates resolve **manifest-relative**
      (`DriverSource::Path` from the manifest's own directory) instead of
      engine_root. The crate carries version-only deps on core + measure; the
      generated package gets a `[patch.crates-io]` table pointing them at the
      engine checkout, and the bencher workspace root carries the same patch
      for standalone `cargo test` in the driver.
- [x] CLI subjects-dir resolution: `--subjects` / `SPATIAL_BENCH_SUBJECTS`
      point at the bencher checkout (sibling-discovery fallback); the engine's
      `subjects/` fallback is gone; justfile, drift-guard test paths, and
      catalog tests all follow. Resolution helpers deduplicated into
      `core::resolve` after the conform mirror drifted from the run path.
- [x] Verify: the engine workspace has no subject dependency at all (finishes
      the A3 story); runs and conform work unchanged against a bencher
      checkout. **Repo push needs the usual permission.**

### Phase 2 — the exec adapter (per-language builds, one contract)

- [x] Rework `[driver]` in the manifest: `lang` + `entry` (the driver's
      source file, beside the manifest) plus the `[build]` recipe; the
      `command`/`output` templating is gone — the harness contract is the
      only interface.
- [x] python builder: stdlib venv (no uv on this host) + the pinned lib
      installed from PyPI/git, cached per pin with an install marker;
      provenance records the pin (PyPI versions are immutable — the S1 story
      for python is inherent).
- [x] **pykdtree** — the first python exec subject: driver.py with a
      numpy-vectorized ChaCha8 byte-identical to the rust drivers' generator
      (self-checked at every startup against pinned vectors), per-probe
      query loop, `pypi` source kind, `euclidean` added to the domain core's
      metric allow-list (§6's review process working), `pykdtree` in
      `UNIVERSAL`. Criterion and perf runs both recorded.
- [x] cxx builder: compiles the shim against the lib's sources fetched at
      the manifest's pin, cache keyed by source sha + flags + compiler + shim
      content; the fetched commit is verified against the declared sha (S1
      for cxx) and recorded in the run header.
- [x] exec run path in the adapter dispatch: same `drive()` (now taking a
      `Command`) as rust-codegen; `adapter::list` serves both; conform now
      checks nanoflann — `1 manifest case, 1 driver registration — match` —
      instead of skipping it.
- [x] The nanoflann shim actually written (json.hpp, chacha8.hpp, shim.cpp):
      byte-identical data generation to the rust drivers (verified against
      their generator vectors, self-checked at every startup), nanoflann index
      + exact_nn queries, harness-contract output. Measurable end-to-end:
      criterion and perf runs both recorded — the first cross-subject data.

### Phase 3 — bencher CI

- [ ] Release-watcher workflow: one implementation covering all three pin
      kinds via `source.kind` — crates.io, GitHub releases (yields the tag's
      commit sha, feeding S1), PyPI — opening pin-bump PRs against the
      manifest. dependabot is deliberately NOT the pin mechanism (it cannot
      bump a pinned_ref+sha pair in TOML); it handles the drivers' own
      dependencies and action versions.
- [ ] Per-language, path-filtered bench workflows on pin-bump PRs: build the
      subject, bench it, prepare results for `submit`, and post an
      old-vs-new chart comment (`spatial-bench-charting`, see Charting).
- [ ] First cross-language result: kiddo vs nanoflann on one chart — the
      moment the split pays for itself.

## Charting (day two — priority-ordered 2026-08-31)

The primary charting surface is the **web front-end** over the dataset;
second, **charts on PR comments** — the pin-bump workflow's payoff, where the
people who need "what did this version bump cost" already are. The terminal
chart is a niche, interactive nicety and lands last.

1. **Web front-end (primary)** (§12): charts, selectors and comparisons over
   the dataset repo (after `submit`) and, before that exists, directly over
   local run documents. The run documents' schema is the whole contract — no
   shared rendering code is needed; a separate front-end repo per §2.
2. **PR-comment charts**: a `spatial-bench-charting` crate rendering
   old-vs-new comparison PNGs of the bumped subject, posted on pin-bump PRs
   by the release-watcher workflow (pairs with bencher CI Phase 3 below).
3. **CLI chart (niche)** — see the crate design below; lands last.

### The `spatial-bench-charting` crate (serves 2 and 3)

- Own binary, `spatial-bench-chart`; own CLI (`clap`). The runner binary
  never links it — charting deps and code stay out of the bench runner's
  build entirely. Workspace member so `just test`/`just lint` cover it.
- Depends on `spatial-bench-core` (publish-ready dual-dep) for
  `schema::Document` parsing, `SCHEMA_VERSION` gating and the **selector
  language** — charts understand the same `--select` grammar as runs.
- **Rendering via `plotters`** (BitmapBackend → the RGBA buffer we transmit
  and save): real typography, anti-aliasing, log scalers, error bars and
  legends — instead of hand-rolling ~2k lines of raster/font code that would
  look worse. The dependency tree is contained to this optional crate: the
  A3 leanness argument applied to the runner and harness, which stay
  dependency-free; an optional side tool should use the best tool for the
  job. An embedded TTF keeps rendering hermetic (CI containers often ship
  no fonts). If plotters fights the terminal-oriented workflow,
  `tiny-skia` + `ab_glyph` is the control-heavy fallback.
- **PNG output** (`--save out.png`) via plotters/`image` — real compression,
  not a stored-deflate hand-roll: PR-comment uploads should not be megabytes.
- **Kitty graphics protocol** for terminal display — the one piece that stays
  bespoke (no crate speaks it): raw-RGBA `f=32` transmission of the plotters
  buffer, base64 split into ≤4096-byte chunks with `m=1`/`m=0` continuation
  flags, `a=T,C=1` placement with `c=<cols>,r=<rows>` cell-box reservation,
  `CSI 16 t` cell sizing (fallback 10×20px), capability probe (env fast-path
  + escape query). `--save` is the universal fallback for non-KGP terminals;
  a block-character fallback is deferred.
- Chart kinds v1: bars (cross-impl comparison, CI whiskers, value labels) and
  lines + markers + CI band (scaling via `--x tree_size`).
- Data: run documents (default `data_dir/runs`, `--latest N` newest-first),
  selector filtering, latest-wins dedupe per (series, x).
- Data flow is pure-logic (`chart::model`, no IO) and unit-tested; the KGP
  encoder is structurally tested; golden PNG bytes for fixed models.

### The main-CLI trampoline (cargo external-subcommand pattern)

- `spatial-bench chart` defines **no** chart arguments: it discovers
  `spatial-bench-chart` (beside the parent binary / `SPATIAL_BENCH_CHART` /
  PATH), forwards trailing args verbatim with **inherited stdio** (the child
  needs the tty for KGP and raw-mode CSI reads), and propagates exit codes.
  Untyped passthrough = no version coupling with the child's CLI.
- Soft-fail ladder when absent: interactive (tty stdin) **and** an engine
  checkout reachable → prompt, then `cargo install --path
  crates/spatial-bench-charting` with inherited stderr; otherwise print the
  exact install command and exit 2. Never silently installs;
  `SPATIAL_BENCH_NO_INSTALL=1` opts out.

## Day two

- [x] `perf` runner (§11): one point per process, `perf stat` counters merged
      onto points by tag tuple (open metrics map is already in place).
      Includes `perf-wrap` adapter shape and picker runner choice (D4).
- [ ] Web front-end (primary charting surface): own repo per §2; reads the
      dataset repo (after `submit`) and, until then, local run documents.
- [ ] PR-comment charts: `spatial-bench-charting` PNGs posted on pin-bump
      PRs by the release-watcher workflow (pairs with bencher CI Phase 3).
- [x] CLI chart (niche): the `spatial-bench-charting` binary + the main-CLI
      trampoline per the design above — kitty rendering, `--save` PNG,
      soft-fail/self-install ladder. Rendering via plotters (bitmap backend +
      ttf), dependency-free KGP transmitter, soft-fail guidance in the parent.
- [ ] `submit` (§8, §10): extract machine detail to `machines/<hash>.toml`,
      open a PR against the statistics repo. Depends on that repo existing and
      on S1 above.
- [x] `conform` (§13): build each driver, list its cases, assert set-equality
      with the manifest. Needs a driver introspection mode in the harness
      contract (noted in the drift-guard test's doc comment).
- [ ] Submission trust (§15): decide attestation vs trusted-machine allowlist —
      deferred until the dataset accepts submissions from more than one person.

### Explicitly out of scope for now

- `asm`/`mca`/`objdump` runners (§11: artefacts, not timing points — need a
  second output shape).
- Publishing any crate to a registry. The dual dep and registry branch are
  publish-*ready*; publishing itself awaits a decision.
