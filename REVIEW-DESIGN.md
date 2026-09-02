# Review findings — design — **all resolved**

From the critical appraisal of the codebase. Every finding below has been
implemented with tests; the entries are kept for the record of what changed
and why.

---

## D1 — Selector metacharacters are unvalidated at the vocabulary boundary — **RESOLVED**

Extension values were closed-set-validated, but nothing checked that a declared
value *parses and round-trips*. Concrete warts: a manifest declaring
`stem.values = ["a|b"]` creates a value no selector can express; declaring
`"true"` produces a value that parses back as `Bool` — both silently wrong in
a chart or repro line.

**Resolved:** `ensure_selectable()` in `catalog_load.rs` runs on every tag
value at load — case tags, matrix values, param defaults, and *declared*
vocabulary values. Words containing `,`, `|`, `..` or equal to `*` are
rejected, and every value must survive a round trip through
`Selector::parse("key=<value>")` admitting itself, which catches
`true`/`false` type-changing spellings and anything else the explicit list
misses. Errors are `ManifestError::BadSelectorValue { subject, key, value,
why }`. Three new tests.

## D2 — `manifest.source.kind` is an unvalidated `String` — **RESOLVED**

Typos ("cargo-git " vs "cargo_git") surfaced at run time as "not implemented",
deep in the CLI's source resolution, rather than at manifest load.

**Resolved:** `lower()` accepts only `cargo-git` and `git`; anything else is
`ManifestError::UnknownSourceKind { subject, found }` at load — the same
closed-vocabulary rule as the adapter. One new test.

## D3 — `Catalog::unsatisfied` only reports `OneOf` clauses — **RESOLVED**

A range (`tree_size=2^40..2^41`) or an existence requirement (`k=*`) that
reached nothing was silent — inconsistent with the principle the function
exists to enforce: an unmet request is reported, never silently narrowed.

**Resolved:** `unsatisfied` now checks `Range` (reported as `k=lo..hi`, pow2
rendered) and `Any` (reported as `k=*`) alongside `OneOf`; negations stay
silent, as documented. One nuance the tests pinned down: a clause that reaches
nothing empties the whole point set, so *every* positive clause in the
selection reports — accurate ("this selection reached nothing, clause by
clause"), and strictly better than the old silence. Two new tests.

## D4 — The picker cannot choose a runner — **RESOLVED (plumbing; full value lands with perf)**

`runners_for()`'s intersection was displayed but not actionable; the
interactive menu hardcoded criterion.

**Resolved:** the picker's list now leads with a runner row — implemented
runners marked chosen, unimplemented ones named with "(not implemented yet:
…)" so their absence is explained rather than silent — and selecting it
switches the runner, which threads into `run_selection` like any other run.
Backed by `Runner::parse`/`implemented`/`Display` in core (`implemented` is
criterion-only until perf lands). When perf arrives, the menu picks it up with
no further UI work. One new core test.

## D5 — `impl` allow-list in `vocab.rs` duplicates subject facts — **RESOLVED (message half)**

Adding a subject required updating both the manifest and `UNIVERSAL` in
`vocab.rs`; the failure mode was an error carrying only the key name —
reading like a manifest bug.

**Resolved (message half):** `TagError` now implements `Display`, carries the
allowed list in `NotAllowed`, and — for `impl` specifically — appends "a new
subject belongs in `UNIVERSAL` in crates/spatial-bench-core/src/vocab.rs".
`catalog_load` maps vocabulary failures to `ManifestError::Vocab { subject,
message }` instead of collapsing them to the bare key name. The allow-list
itself stays in `UNIVERSAL`: the duplication is the engine-ownership story
working as designed. One new test.

## D6 — `measure()` silently clamped `sample_size` below 10 — **RESOLVED**

The measurement path raised a budget below criterion's minimum instead of
rejecting it. No live path reached it (the budget is engine-controlled), but
silent clamping inside the comparability contract was a trap waiting for the
first config surface.

**Resolved:** `measure()` refuses an impossible budget with the reason —
`sample_size` below criterion's minimum of 10, or zero warm-up/measurement
time — instead of clamping, in the same honest-refusal spirit as the harness
version check. One new test.

---

## Resolved from the same review (do not re-open)

- **B2** — `mem_total_bytes` in the fingerprint's `[unprivileged]` block;
  captured, validated as a hardware change, restored into run documents (the
  §4 footprint warning works on fingerprinted machines).
- **B5** — schema_version discipline: waived by decision; v1 has never left
  this machine, development is still defining v1.
