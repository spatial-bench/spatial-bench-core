# Open review findings — security

From the critical appraisal of the codebase. Ordered by the order they should
be addressed: **S1 before the dataset accepts submissions** (it changes what a
manifest review means), the rest as hardening when touched.

Threat model, restated: manifests and driver crates are *trusted* engine
content (that is §4's whole argument — vendored, reviewed, centralised). The
untrusted inputs are subject pins (third-party git repos), user-supplied paths,
and the host environment. Nothing below assumes the trusted tier is hostile.

---

## S1 — Git tag pins are mutable; provenance detects but nothing enforces — **RESOLVED**

`materialise()` writes `tag = "v6.0.0-alpha.4"` into the generated Cargo.toml;
cargo resolves the tag at fetch time. If the tag moves — repo compromise, or
more realistically a third-party repo simply re-tagging — a fresh build
(new cache key) compiles different code while the run header still records the
same `pinned_ref`. The recorded `sha` makes this *auditable after the fact*,
which meets the design's stated bar, but a dataset whose value is comparability
over months should refuse, not merely detect.

**Resolved:** `Source` gained an optional `sha` (validated as a full commit id
at load — anything shorter is `ManifestError::BadSha`), carried through
`SubjectFacts::expected_sha`. After a build, `run::execute` compares the
lockfile-resolved revision (`build::built_subject`) against it via
`run::enforce_sha`: a moved tag is refused naming both revisions; a lockfile
with no revision is refused as unverifiable. Worktree builds are exempt — no
revision by definition, already marked `git_dirty`. The kiddo manifest pins
`v6.1.0` + its sha; unit tests cover match, mismatch and unverifiable.

## S2 — TOML injection through `--subject-path` values — **RESOLVED**

`materialise()` interpolates the subject path into the generated Cargo.toml
with `{:?}` only. A directory containing `"` (legal on Linux) corrupts or
augments the manifest the engine then builds. Self-inflicting only today, but
the same pattern applies to manifest `repo`/`pinned_ref` fields — trusted by
design, but if manifests are ever accepted as third-party PRs they become
injection points into an executed build.

**Resolved:** `build::toml_safe_path` rejects paths containing `"`, `\`,
newline or carriage return, with a unit test; the CLI validates
`--subject-path` values through it after canonicalization, naming the
offending path. `materialise()`'s doc comment now marks the function as a
trusted-input boundary: paths are validated, manifest strings are trusted
engine content (§4) by explicit decision.

## S3 — The fd-juggling `StdoutToStderr` guard deserves audit-hardening — **RESOLVED**

The dup/dup2/restore sequence is correct and Drop-restores on panic, but it is
`unsafe` in the comparability path.

**Resolved:** the guard's doc comment now states the invariants explicitly —
fd 1 refers only to the harness pipe or fd 2; `saved` is moved into the guard
and closed exactly once in `Drop`, which runs on panic; failure to `dup` means
the guard does not engage (noisier, not wrong) — and the process-global,
unlocked nature of the swap, with the requirement that a stricter mechanism or
process isolation precede any concurrency around it. The proposed runtime test
was dropped as unobservable without fd capture in-process; the invariant
comment is the audit artefact.

## S4 — Privileged probe trusts root's PATH under `sudo` — **RESOLVED**

`run_capture()` tries the bare name `dmidecode`/`decode-dimms`/`lspci` before
the absolute fallbacks. Under `sudo fingerprint --write`, the bare name resolves
through root's PATH — a planted `dmidecode` earlier in root's PATH executes as
root.

**Resolved:** `run_capture` candidate lists order absolute paths first —
`/usr/sbin/dmidecode`, `/sbin/dmidecode`, then bare `dmidecode`; same shape for
`decode-dimms` and `lspci` (plus `/usr/bin/lspci`). A planted bare name earlier
in root's PATH no longer wins over the standard locations.

## S5 — A custom fingerprint source is invisible in the run document — **RESOLVED**

`SPATIAL_BENCH_FINGERPRINT` lets any user point validation at a file they
control. Validation still requires the unprivileged fields to match this host,
so a foreign file fails — but a locally crafted file passes by construction.
That is within the design's threat model (CI/containers), yet the run document
cannot distinguish "validated against /etc" from "validated against a
user-supplied file".

**Resolved:** `fingerprint::validate_for_run` returns a `ValidatedHost { 
machine, fingerprint }` — the machine to record and the file that vouched for
it. `run::execute` threads that into the run header:
`Context.fingerprint` names the file (e.g. `/etc/spatial-bench/fingerprint.toml`)
for a validated run and is `None` for an unverified one, so the dataset can
distinguish trust sources. Test asserts the unverified path records no source.

## S6 — `sudo` + env-var capture/write mismatch — **RESOLVED**

`sudo spatial-bench fingerprint --write` may drop `SPATIAL_BENCH_FINGERPRINT`
(env_reset), writing to `/etc` while the user pointed the variable elsewhere —
subsequent runs then validate against a different file than was captured. Has
already caused one confusing debugging round on this machine (stale release
binary aside).

**Resolved:** `fingerprint --write` warns at write time when
`SPATIAL_BENCH_FINGERPRINT` is set, naming both the env path and the path this
process will write, and suggesting `sudo --preserve-env=…` for keeping it.
Verified live against the debug build.