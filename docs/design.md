# Tag-addressed benchmarking — design

Status: **spec, largely implemented**. The engine lives in this repo (`crates/`):
catalog/selector/vocabulary, two-phase codegen with a content-keyed build cache,
the criterion driver and harness contract, machine fingerprinting (§9), pinned
`cargo-git` builds with lockfile provenance (§4) and the CLI run path are working.
Still open here: the `perf` runner and wrapper (§11), the nanoflann shim (`exec`
adapter), `submit`/`conform` (§8, §13) and charting. This document was
originally written in the context of the kiddo project (`../kiddo`).

Supersedes an earlier draft that put a kiddo-specific catalog inside kiddo, and a second
that let willing subjects ship their own manifests. Both are wrong: the engine benches
libraries kiddo does not own, and no subject may declare its own cases (§4).

## 1. What's actually wrong today

Measured on `master` at `c299dde1`:

| Symptom             | Evidence                                                                                                                                                                                                                            |
| ------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Recipe explosion    | 156 `just` recipes: 33 `asm-*`, 18 `perf-*`, 14 `bench-*`, 12 `profile-*`, 12 `chart-*`, 9 `html-*`, 6 `objdump-*`, 3 `uprof-*`, 3 `cg-*`, 2 `mca-*`                                                                                |
| Non-orthogonal axes | Four env vars for one axis: `KIDDO_PROFILE_MIN_LOG2_POINTS`, `KIDDO_LARGE_MIN_LOG2_POINTS`, `KIDDO_PROFILE_POINTS`, `KIDDO_PROFILE_MIN`. Three for another: `KIDDO_PROFILE_QUERIES`, `KIDDO_PROFILE_MAX_QTY`, `KIDDO_BENCH_MAX_QTY` |
| Identity is prose   | A point is identified by binary name + criterion `group_id`/`function_id` + recipe name. Recovering what was measured means parsing a string, and every chart script has its own parser                                             |
| Output profusion    | 34 untracked files in the repo root right now: `bench_result-*.json`, `*.png`, `bench-status*.txt`, `base-zen5*/`. `.gitignore` covers none of them; `asm-*` recipes write `.asm` straight into the root                            |
| Cognitive overhead  | Running a comparison means knowing which of 25 bench binaries, which of ~40 env vars, and which of 12 chart scripts go together                                                                                                     |

The root cause is that **benchmark identity is encoded in names rather than data**. Fix
that and the recipe explosion, the chart sprawl and the file profusion collapse together.

## 2. Three components

Different cadences, different audiences, so they are separated:

| Component      | What it is                                                             | Cadence                    |
| -------------- | ---------------------------------------------------------------------- | -------------------------- |
| **engine**     | Library-agnostic runner: tags, selector, adapters, fingerprint, schema | Reviewed code, semver      |
| **statistics** | The accumulated dataset, append-only                                   | Submissions, grows forever |
| **charting**   | Front-end over the dataset                                             | Independent UI work        |

Three repos:

| Repo                    | Holds                                                     |
| ----------------------- | --------------------------------------------------------- |
| `spatial-bench`         | the engine, its CLI, and `subjects/` — every manifest     |
| `spatial-bench-results` | the dataset: `datasets/YYYY-MM/*.json`, `machines/*.toml` |
| `spatial-bench-charts`  | the front-end                                             |

The statistics repo especially wants isolation: it grows without bound and its pull
requests are data submissions rather than code review, so sharing a queue with engine code
serves neither.

kiddo itself gets **no new files at all** — see §4.

## 3. Core model

A **case** is a measurable thing, carrying:

- **fixed tags** — key/value pairs defining _what is measured_; invariant for that case
- **params** — axes it can be swept over, each with a domain and default
- **adapter** — how to execute it (§5)
- **runners** — which measurement backends apply (`criterion`, `perf`)

A **data point** is a case with every param resolved. Its identity is the full tag map,
fixed ∪ resolved. That map is what lands in the dataset and what charts group by. No name
is ever parsed.

```
case  { impl=kiddo_v6, query=exact_nn, k=1, dims=3, axis=f64, kiddo.stem=eytzinger }
        params: tree_size ∈ 2^16..2^25, query_count ∈ ℕ (default 1000)
             │
             ├── point { …case tags…, tree_size=1048576,  query_count=1000 }
             └── point { …case tags…, tree_size=16777216, query_count=1000 }
```

## 4. Subjects are always vendored

A **subject** is a library under test. Every subject's manifest — vocabulary, cases,
params, build recipe, any shim source — lives in the **engine**, including kiddo's.

### Why not let willing projects self-declare

The obvious design lets a cooperative, fast-moving project like kiddo ship its own
manifest alongside its code. It is rejected on trust grounds: a subject that declares its
own cases controls what is measured about it and how, which makes flattering itself
possible — picking the tree sizes where it wins, quietly omitting an unfavourable `k`,
tuning build flags for its own cases only. Even if nobody ever does it, a comparison
dataset whose subjects wrote their own rules cannot be shown to be fair, and a benchmark
nobody can audit is worth very little.

Vendoring everything makes every declaration reviewable in one place, under one set of
eyes, with one changelog. A library cannot change how it is benchmarked without a PR
against the engine.

**The price, stated plainly.** kiddo moves fast, so every new stem strategy needs a
matching engine PR before it can be measured — the engine now lags its subjects by a
review cycle. That friction is the cost of the property; it is not free and it will
occasionally be annoying. Two things blunt it: a willing project is still welcome to
_author_ the PR against the engine (contribution stays open, only authority is
centralised), and the engine pins each subject by ref, so a kiddo release and its catalog
update are simply two commits in two repos rather than a coordinated release.

### Declared range versus what actually fits

A manifest's `tree_size` range says what is **worth sweeping**, and its other limits
express **genuine subject constraints** — Pkd-tree asserting on f32 above 2^21 is a real
property of Pkd-tree and belongs in its manifest.

What does _not_ belong there is host memory. 2^29 f64 points at K=3 is ~1.7 GB of raw
coordinate storage before any tree overhead: comfortable on a 64 GB workstation, impossible
on a 16 GB laptop. That ceiling is per-machine, so encoding it as a manifest constant would
be wrong everywhere except the machine it was written on.

The engine therefore estimates a lower-bound footprint per point from the core tags and
checks it against the host at run time, warning rather than refusing — the estimate ignores
stem and leaf overhead, and the operator may know better than it does.

### Pinning and provenance

Because the engine builds every subject, it pins each one, and every run records what it
actually built:

```jsonc
"subjects": {
  "kiddo_v6":  { "version": "6.0.0-alpha.4", "pinned_ref": "v6.0.0-alpha.4", "sha": "c299dde1…" },
  "nanoflann": { "version": "1.7.1",         "pinned_ref": "v1.7.1" }
}
```

Without this, results separated by months are not comparable and nothing says why. It is
the field that makes a long dataset trustworthy, so it is mandatory.

kiddo's existing `benches/cpp_shims/*.cpp` and `build_cpp_competitors.rs` are already this
pattern, built in the wrong place; they move into the engine as vendored subjects, and
kiddo stops carrying C++ build machinery for libraries it does not own.

### Toolchain policy

**One toolchain per run, enforced.** A run pins a single rustc, builds every Rust subject
with it, records it in the run header, and refuses to mix. Everything inside a run is then
comparable by construction, and a rustc upgrade shows up as a difference _between_ runs
where it can be faceted, rather than as an uncontrolled confound between two subjects in
the same chart.

Subjects declare a floor rather than a pin, so the engine can find a toolchain that
satisfies a whole selection:

```toml
[toolchain]
min_rustc = "1.89.0"
```

The engine resolves the run's toolchain as the highest floor among selected subjects,
unless `--rustc` overrides. If an explicit pin sits below some selected subject's floor the
run is refused rather than silently dropping that subject — a comparison quietly missing a
contender is worse than one that will not start.

## 5. Harness, drivers and code generation

### The harness belongs to the engine

An earlier draft had the engine drive each subject's _own_ benchmark binary, selecting
benchmarks through criterion's filter argument. That was wrong, and it contradicted §4.

Measurement methodology lives in the harness, not the declaration: how points are
generated, which seeds, whether the query set is cache-resident, what sits inside the
timed region, whether the result is black-boxed. If kiddo is measured by kiddo's harness
while nanoflann is measured by a shim the engine wrote, the two numbers are not
comparable, and kiddo controls its own measurement — the exact thing vendoring
declarations was meant to prevent.

So **the engine owns the harness for every subject**. Each subject gets a _driver_: a
program in the engine that calls the library's public API and nothing else. One harness
contract — same generator, same seeds, same query sets, same timed region — is what makes
numbers from different libraries mean the same thing.

A subject contributes its public API and nothing else. It needs no manifest, no
dependency, and no source change, which is what makes an uncooperative library measurable
at all.

### Two-phase drivers for generic libraries

A library as generic as kiddo makes one driver binary impossible to compile in reasonable
time. Every combination of scalar type, dimensionality, bucket size, index type, stem
strategy and leaf strategy is a separate monomorphisation; instantiating the full cross
product is what makes kiddo's own fuzz binary take an age to build.

So a driver may be **two-phase**. The engine generates a `main.rs` containing one macro
invocation per combination the selection actually needs, then builds and runs it. Compile
time then scales with the selection rather than the catalog.

This splits the tag space in two, and the split is the whole mechanism:

| Axis kind        | Examples                                                       | Cost                      |
| ---------------- | -------------------------------------------------------------- | ------------------------- |
| **Compile-time** | scalar, dims, bucket, index type, stem strategy, leaf strategy | one monomorphisation each |
| **Runtime**      | tree size, query count, k, seeds                               | a function argument; free |

Only compile-time axes drive generation. Cases differing solely in runtime axes share one
binary, so sweeping ten tree sizes is one compile rather than ten. A manifest marks which
of its keys are compile-time (§6).

The macro lives in the engine's driver crate, not in the subject:

```rust
// generated by `spatial-bench run`, one line per selected combination
bench_case!(scalar = f64, dims = 3, bucket = 32, idx = u32,
            stem = Eytzinger, leaf = FlatVec);
```

Each invocation expands to a registry entry — a tag map plus a function pointer that runs
the case for those types. The generated binary sweeps runtime axes internally.

Two things keep this from being painful, and both are requirements rather than
optimisations:

- **Deterministic generation.** Combinations are sorted, so the same selection always
  produces byte-identical source. Without that, caching cannot work at all.
- **Content-hashed build cache.** Keyed on the generated source, the toolchain, the
  subject's revision and its features. Without it every run recompiles and one slow build
  has merely become many.

### Adapters

An adapter turns (case, resolved params) into points.

| Adapter        | Executes                                                         |
| -------------- | ---------------------------------------------------------------- |
| `exec`         | a driver binary; params as templated arguments                   |
| `rust-codegen` | generates, builds and runs a two-phase driver, then as `exec`    |
| `perf-wrap`    | wraps either under `perf stat`, merging counters onto its points |

`exec` is what makes non-Rust subjects first-class: a Python or Julia driver is a command
plus an output contract. Adding a language means adding a driver and a manifest, not
engine code.

## 6. Vocabulary: universal, per-domain core, subject extensions

Three tiers, so cross-library charts stay sharp without committing the engine to one
problem shape forever.

**Universal** — true of anything measurable, present in every domain:

`impl` · `query`

**Domain core** — engine-owned, shared by every subject declaring that domain. This is what
makes a cross-library chart possible at all: two subjects can only be compared on an axis
they both name identically, and the domain core is what guarantees they do.

```toml
domain = "spatial_index"
# core: k · dims · axis · idx · metric · isa
#       tree_size · query_count · threads
```

A future domain — spatial join, raster, whatever comes — declares its own core rather than
being forced through `spatial_index`'s keys or dissolved into extensions. Adding one is an
engine change, reviewed once, and it cannot disturb an existing domain's data.

Keys are additionally marked **compile-time** or runtime, which is what the two-phase
driver generation keys on (§5):

```toml
[vocab]
stem.values = ["eytzinger", "donnelly_unrolled", …]
stem.compile_time = true      # a generic parameter: each value is a monomorphisation
```

**Subject extensions** — declared in a subject manifest, namespaced under the subject:

`kiddo.stem` · `kiddo.leaf` · `kiddo.bucket` · `kiddo.storage` · `kiddo.executor` ·
`nanoflann.leaf_max_size`

Namespacing is load-bearing: two libraries will both want a key called `layout` and mean
different things by it. Namespaced, they coexist, and grouping by a core key never silently
mixes them.

All three tiers are closed — an unknown key is rejected when a manifest loads. An open
vocabulary is precisely how the current suite ended up with four spellings of tree size.

### Why not a minimal core

Cutting the core to `impl`/`query` and making everything else an extension looks more
general, but it moves the coordination problem rather than removing it: comparing kiddo and
nanoflann on `k` would then require both manifests to have independently chosen the same
extension key. The domain core exists to make that agreement structural instead of a
convention nobody enforces.

## 7. Selector language

```
impl=kiddo_v6,query=exact_nn,axis=f64,k=1|5|20,tree_size=2^20..2^26
```

`,` = AND · `|` = OR within a key · `..` = inclusive range · `*` = key must exist ·
`!key=value` = negate · omitted key = unconstrained · repeated `--select` = union.

A missing key never satisfies a clause, negated or not, so `!kiddo.stem=eytzinger` excludes
a subject that has no stem rather than admitting it vacuously.

Any requested value that reaches **no** point is reported before the run starts. A value
that reaches some subjects but not others is not: subjects genuinely differ, and that is
the comparison working rather than failing.

One expression both filters which cases run and pins which param values sweep, so there is
no second flag to keep in sync. An unconstrained param uses its default;
`query_count=1000|10000` sweeps both.

## 8. CLI surface

```
spatial-bench list      [--select EXPR]...  [--format table|json|tags]
spatial-bench run       [--select EXPR]...  [--runner criterion|perf] [--dry-run]
spatial-bench subjects  [--source native|vendored]
spatial-bench describe                       # merged catalog as JSON
spatial-bench fingerprint [--write]          # see §9; --write needs root
spatial-bench machine   [--explain]
spatial-bench submit    [--to REPO] [--since DATE]
spatial-bench conform   [--subject NAME]
```

`spatial-bench` with no arguments enters the interactive picker: it walks the keys present in
the merged catalog, offers only values that keep the selection non-empty, and keeps a live
count.

```
  impl        [x] kiddo_v6   [ ] nanoflann   [ ] pkdtree
  query       [x] exact_nn   [ ] within_radius
  axis        [x] f64        [ ] f32
  k           [x] 1  [x] 5  [x] 20  [ ] 50
  tree_size   [x] 2^20  [x] 2^23  [x] 2^26   … 7 more
  kiddo.stem  [x] eytzinger  [x] donnelly_cyclic_simd_full   … 8 more

  36 cases · 108 points · criterion · est. 21 min
```

On confirm it prints the non-interactive equivalent **before** running — the CI repro
line, which must round-trip through the parser:

```
Reproduce this exact selection:

  spatial-bench run --runner criterion \
    --select 'impl=kiddo_v6,query=exact_nn,axis=f64,k=1|5|20,tree_size=2^20|2^23|2^26'

Run now? [Y/n]
```

## 9. Machine fingerprint

Two-phase, as discussed: a privileged capture once per machine, unprivileged validation on
every run.

### Capture

`sudo spatial-bench fingerprint --write` reads everything, including the parts needing root, and
writes `/etc/spatial-bench/fingerprint.toml`. Machine-scoped rather than per-user or per-checkout
— it describes hardware, and one machine with six kiddo worktrees should have one
fingerprint. `$SPATIAL_BENCH_FINGERPRINT` overrides for containers and CI.

```toml
schema  = 1
taken   = "2026-08-04T11:02:19Z"
machine = "k7f2qa9vm3-x7q2m"

[privileged]                       # needs root; captured once
mem_speed_mts = 6000
mem_timings   = "30-36-36-96"
mem_parts     = ["F5-6000J3036G32GX2-TZ5NR", "F5-6000J3036G32GX2-TZ5NR"]

[unprivileged]                     # re-validated on every run
cpu_model    = "AMD Ryzen 9 9950X 16-Core Processor"
cpu_base_mhz = 4300
board_vendor = "ASUSTeK"
board_name   = "ROG STRIX X870E-E"
chipset      = "AMD X870E"

checksum = "b3:9f2a…"               # see below
```

The hash is **two-part**, split on the privilege boundary the validation
already has. The 10-character prefix covers the unprivileged components — it is
the machine's identity, recomputable by any run without root, and unchanged by
a re-capture. The 5-character suffix covers the privileged components; it is
the literal `UNKNOWN` when memory was never verified (no fingerprint, or a
capture whose privileged block is empty). A readability change — installing
`decode-dimms`, capturing with or without root — moves only the suffix; the
prefix proves it is still the same box. The full string is the join key:
charts join on all of it, never the prefix alone, or unverified runs would
merge into verified history.

### Validation on every run

Before measuring, the engine re-probes the `[unprivileged]` block and compares:

| Outcome                   | Behaviour                                                         |
| ------------------------- | ----------------------------------------------------------------- |
| all match                 | proceed; use the recorded machine hash                            |
| any field differs         | **bail** — hardware changed or the file came from another machine |
| file missing              | **bail** with the exact `sudo` command to run                     |
| checksum mismatch         | **bail** — file was edited                                        |
| `--allow-unfingerprinted` | proceed, hash marked degraded, run flagged in the dataset         |

Bailing on a partial match is the point: a stale fingerprint would otherwise attribute new
hardware's numbers to the old machine's history, which is worse than refusing to run.

### The checksum

The `checksum` field is **tamper-evidence, not tamper-resistance**, and is named that way
deliberately.

It is an HMAC-SHA256 over the canonical TOML body, keyed by a compile-time constant in the
fingerprinter and truncated. That reliably catches the realistic failure modes — an
accidental edit, a half-finished hand-tweak, a file copied from another machine and
adjusted to fit — and a mismatch is fatal (see the table above).

It stops nobody who wants to forge a result: the key sits in an open-source binary and
comes out with `strings`. The field is therefore never described as a signature anywhere
in the tooling, output or documentation, because a passing check is not evidence of
provenance and a name like "signature" invites someone downstream to treat it as though it
were.

Genuine provenance, if it is ever wanted, comes from **attestation at submission** — a PR
from a known machine, or a CI runner signing with a key the submitter never holds — and is
tracked separately (§15).

## 10. Result documents and layout

One document per run: a `run` header plus `points[]`. Every point carries its full tag map,
metrics as an open map (so `perf-wrap` adds `cycles`, `instructions`, `branch_misses` to a
point with identical tags, letting timing and counters join on the tag tuple), and stats.

The run header carries `run_id`, timestamps, selectors, `machine_hash`, the `subjects`
block from §4, plus toolchain, source and boot-environment context (kernel, bench profile,
governor, SMT, boost, isolated CPUs). Environment is **run-level, not point identity** —
that is exactly what lets one chart overlay the same case across kernels or boot profiles
and facet by them.

Machine detail is not repeated in every run. `spatial-bench submit` extracts it once to
`machines/<hash>.toml` in the dataset and asserts consistency thereafter.

### Naming

You said the example filename was a starting point, so:

```
datasets/2026-08/20260804T123456Z-k7f2qa9vm3-x7q2m-criterion-01k2y7f3.json
                 └── UTC, ISO basic ──┘ └machine┘ └runner─┘ └run┘
```

- **ISO basic form, no separators** — sorts lexicographically = chronologically, and
  contains no colon, which is illegal in Windows filenames. Charting tools read this
  dataset from anywhere even though runs only happen on Linux.
- **Machine hash in the name** — `ls datasets/2026-08/*-k7f2qa9vm3-*` answers "what did this
  box do last month" without opening anything.
- **Runner in the name** — a criterion and a perf run of the same selection on the same
  machine cannot collide, and listings are informative.
- **Run-id suffix, not milliseconds** — guarantees uniqueness and ties the file to its
  `run_id` without pretending the timestamp is precise.
- **Month directories** — bounded directory size over years, as you wanted.

### Where results land locally

Not in the kiddo checkout at all. Default output is
`${XDG_DATA_HOME:-~/.local/share}/spatial-bench/runs/`, and `spatial-bench submit` opens a PR against
the statistics repo. A kiddo working copy stays clean by construction, which is the actual
fix for the 34 stray files — `.gitignore` would only have hidden them.

## 11. Runners

**criterion** — used _inside_ the engine's own driver, identically for every Rust subject,
rather than being the subject's. It supplies warm-up, outlier detection and confidence
intervals, which are worth not reimplementing. The harness contract is kept independent of
it so it can be replaced later without changing what a driver must emit.

**perf** — same cases, different measurement. One constraint shapes it: `perf stat`
attributes counters to a **process**, so a perf run executes one case per process rather
than a whole sweep. Slower, and the menu's estimate must reflect that.

`asm`/`mca`/`objdump` are out of scope here — they emit artefacts, not timing points, so
they need a second output shape. Same catalog and selector when they land.

## 12. Crate layout and local development

```
spatial-bench/                        # engine repo
  crates/
    spatial-bench-core/               # tags, selector, catalog, manifest, schema, machine
    spatial-bench-cli/                # the `spatial-bench` binary — cargo install target
    spatial-bench-kiddo-v6/           # bench_case! macro + harness; depends on kiddo
    spatial-bench-nanoflann/          # C++ shim and build script
  subjects/
    kiddo/subject.toml                # kiddo is vendored too — no exceptions
    nanoflann/{subject.toml,shim.cpp}
    pkdtree/, alglib/, scipy/, …

spatial-bench-results/                # statistics repo
  datasets/2026-08/*.json
  machines/*.toml

spatial-bench-charts/                 # front-end repo

kiddo/                                # unchanged: no manifest, no dependency
```

Core is split from the CLI because a generated driver links core — it must emit the
schema — and should not drag clap and terminal handling into that build.

Driver crates are build inputs the CLI compiles on demand. They are never something a user
adds to their own project.

### Running it while developing a subject

The CLI installs with `cargo install spatial-bench`, and `--subject-path` builds a subject
from a working tree instead of its pinned ref:

```justfile
# kiddo/justfile
bench *ARGS:
    spatial-bench run --subject-path kiddo_v6={{justfile_directory()}} {{ARGS}}
bench-menu:
    spatial-bench --subject-path kiddo_v6={{justfile_directory()}}
```

kiddo gains a justfile recipe and nothing else: no dependency edge, no manifest, no change
to its Cargo.toml. A `spatial-bench-kiddo-v6` crate imported _by_ kiddo was considered and
rejected — the driver depends on kiddo, so kiddo depending back on it is a cycle. Cargo
permits cyclic dev-dependencies so it would likely build, but it would pull the whole
benchmark engine into kiddo's dev-dependency tree and lockfile and tie kiddo's MSRV to the
engine's. For a published library that is a bad trade, and nothing needs it.

**This does not weaken §4.** The safeguard is on _submission_, not execution. Anyone may
run the tool; what matters is that manifests and drivers live in the engine and are
reviewed there, and that the shared dataset only accepts runs from a released engine
against pinned refs. A local run is recorded as such — engine version, subject built from
a working tree, dirty flag — so it is useful to you and rejectable by the dataset.

One consequence: under `--subject-path` the driver compiles against your working tree, so
the macro's generic parameters must match the API as it currently stands. Rename a stem
strategy and the driver stops compiling until the engine is updated. That is the
review-cycle cost §4 already accepts, met during development rather than at release.

### Local charts

`spatial-bench chart` reads run documents from the local runs directory and writes a
self-contained HTML file. No dataset repository, no network. It shares rendering code with
the hosted front-end. Change a strategy, re-run, look at a chart is the loop that makes
this worth using during development at all.

## 13. Guarding against drift

1. **Vocabulary** — every key in a manifest is core or a declared extension of that
   subject, and every compile-time key is one the driver's macro accepts.
2. **Generation** — the source generated for a selection compiles. This is the check that
   catches a subject renaming a type the macro names.
3. **Conformance** — `spatial-bench conform` builds each driver, asks it to list the cases
   it contains, and asserts set-equality with the manifest. This is what makes
   declared-rather-than-registered catalogs safe. It needs drivers built, so it belongs in
   CI rather than pre-commit.

## 14. What this retires

| Today                                   | After                                                                                                                            |
| --------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------- |
| 14 `bench-*` recipes                    | `spatial-bench run`                                                                                                              |
| 18 `perf-*` recipes                     | `spatial-bench run --runner perf`                                                                                                |
| 12 `chart-*` recipes + 13 chart scripts | the charting front-end                                                                                                           |
| ~40 `KIDDO_*` env vars                  | one selector expression                                                                                                          |
| `tools/criterion-export`                | criterion used inside the engine's own driver                                                                                    |
| kiddo's 40 `benches/*.rs`               | one driver in the engine (kiddo keeps whatever it wants for its own microbenchmarks, but nothing in the dataset comes from them) |
| `benches/cpp_shims/`, C++ build script  | vendored subjects in the engine                                                                                                  |
| kiddo's own bench orchestration         | nothing in kiddo at all                                                                                                          |
| Result files in the repo root           | `~/.local/share/spatial-bench/runs` → dataset PR                                                                                 |

The 33 `asm-*` / 6 `objdump-*` / 2 `mca-*` recipes are untouched in this pass.

## 15. Still open

**Submission trust.** Whether a dataset pull request needs attestation beyond the
fingerprint checksum (§9) — a CI runner signing with a key the submitter never holds, or
simply a trusted-machine allowlist. Only bites once `spatial-bench-results` accepts
submissions from more than one person, so it is deferred rather than unresolved.

Everything else is decided: three repos (§2), subjects always vendored (§4), one toolchain
per run (§4), the harness owned by the engine and two-phase generation for generic
subjects (§5), universal + per-domain core + namespaced extensions with compile-time
marking (§6), checksum not signature (§9), crate split with `--subject-path` for local
development (§12).
