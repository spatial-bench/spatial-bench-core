# Open review findings — architecture

From the critical appraisal of the codebase (crates + CLI + driver). Points
already fixed are listed at the bottom so future readers know what not to
re-litigate. Each finding is standalone: file paths and the recommended fix are
included, so no conversation history is needed to act on it.

Priority order for the architecture set: **A6 next** (time-boxed: it gets
permanently more expensive once real data accumulates), then A3.

---

## A1 — The adapter abstraction is decorative; the design's central polymorphism doesn't dispatch — **RESOLVED**

`Case.adapter` is a string (`crates/spatial-bench-core/src/case.rs`) that the run
path never reads. `execute()` in `crates/spatial-bench-cli/src/main.rs` assumes
rust-codegen for every subject; a subject declaring `adapter = "exec"`
(nanoflann does) fails only incidentally — with "declares no driver macro",
which is the wrong error for the actual problem (no exec driver exists).

Design §5 defines three adapters (`exec`, `rust-codegen`, `perf-wrap`); today
there is one hard-coded path wearing the costume of three.

**Resolved:** `Case.adapter` is a typed `adapter::Adapter` enum, validated at
manifest load (unknown kinds are a load error naming the subject;
`ManifestError::UnknownAdapter`), and §5's dispatch lives in
`crates/spatial-bench-core/src/adapter/mod.rs` as `prepare`/`run` over a
`SubjectRequest`. `rust-codegen` is implemented there by composing
codegen + materialise + `build::compile` + `harness::drive` (moved out of the
CLI); `exec` refuses with an honest message naming the subject. The CLI resolves
only the inputs its adapter needs (`CodegenInputs`, `None` for exec), so an exec
subject can no longer die on a driver-source lookup suggesting `--engine-src`
would fix an adapter that does not exist. `conform` (§13) will reuse the same
seam.

## A2 — `main.rs` is becoming the second engine — **RESOLVED**

`execute()` did toolchain resolution, fingerprint gating, feasibility warnings,
generation, materialisation, cargo invocation, driving, lockfile provenance, and
document writing — 150+ lines of policy in the CLI crate.

§13's `conform` needs build + run + introspect; if it is written against
CLI-private helpers there will be two build paths, and they will drift.

**Resolved:** the whole orchestration now lives in
`crates/spatial-bench-core/src/run.rs` — `RunConfig`, `plan()` (the dry run),
`execute()` (the real pipeline: fingerprint gate → per-subject dispatch →
provenance → `write_document`), and `resolve_toolchain`. The CLI's `cmd_run` /
`run_selection` are rendering only: the summary line, `toolchain:`, the plan,
and `wrote <path>`. The fixed harness seeds moved to `harness.rs` where the
contract lives; `data_dir()` moved to core as engine-output policy. `conform`
can now be written against `run::plan`/`run::execute` with no new build paths.
Line count: `main.rs` 638 (from ~780), of which most is picker rendering and
command definitions.

## A3 — Criterion in core contradicts core's stated reason for existing — **RESOLVED**

Core was split from the CLI so a generated driver "emits the schema" without
dragging CLI weight into that build. Criterion sat in core
(`adapter/criterion_rust.rs`), so every driver — including future non-criterion
`exec` harnesses — linked criterion and its dependency tree.

**Resolved:** the measurement moved to a third crate,
`crates/spatial-bench-measure` (a workspace member, no subject dependencies), holding
`measure`, the estimates conversion, and the stdout fd guard. Core depends only
on blake3/serde/serde_json/toml/hmac/sha2 again; the kiddo driver crate depends
on core + measure (both dual path+version deps, publish-ready). The generated
driver's lockfile confirms the chain driver → measure → criterion. Core's own
test count dropped by the eight moved measurement tests.

## A6 — The machine hash is 30 bits, and identity is forever — **RESOLVED**

`Machine::hash()` returned 6 base32 chars ≈ 2³⁰ values
(`crates/spatial-bench-core/src/machine.rs`). Birthday collisions at ~37k
machines, and the hash is a filename component and the join key across the
whole dataset — a collision silently merges two machines' histories, the one
failure this machinery exists to prevent.

**Resolved:** widened to 10 base32 chars (50 bits), with the module doc now
stating that the hash's width, component set and canonical rendering are
permanent in practice — any change is a new generation belonging behind the
fingerprint file's `schema` field. Design doc §9 examples updated. Because the
change invalidates existing captures by construction, `Fingerprint::machine()`
detects a width mismatch and reports `StaleCapture` — "captured by a previous
engine generation, re-capture" — instead of the misleading "hand-edited";
`--allow-unfingerprinted` proceeds past a stale capture (degraded), while
checksum and hardware-change mismatches still bail unconditionally.

**Operational consequence:** this machine's fingerprint must be re-captured
(`sudo spatial-bench fingerprint --write`) — which also picks up
`mem_total_bytes` from B2.
continuity cost.

---

## Resolved from the same review (do not re-open)

- **B1** — `--runner perf` refused rather than recording criterion numbers under
  a perf header.
- **B3 / B4** — real `started_at`/`finished_at`; real ULID run ids.
- **A4** — driver crate in `just test`/`just lint`; §13 drift-guard check 2
  exists as `crates/spatial-bench-kiddo-v6/tests/compile_generated.rs` (it
  caught a real manifest/pin drift on its first run).
- **A5** — engine-source resolution (`--engine-src` / `SPATIAL_BENCH_ENGINE_SRC`),
  registry-dep branch for installed binaries, dual dep on core in the driver
  crate, and `driver_rev` in the build cache key.
