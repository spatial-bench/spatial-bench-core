# Add an input distribution or dataset

The current `dataset` axis names an input distribution. Core generates uniform
points on `[0,1)^D` or standard Gaussian coordinates (Box–Muller transform).
It does not provide an arbitrary-file upload/import interface. A new generated
distribution and a fixed real-world dataset therefore require different work.

## Specify the experiment first

Define the construction and query distributions, dimensionality/scalar support,
coordinate scale, correlations, duplicates, outliers, and relationship between
query and construction sets. Explain the workload being represented and how
radius and distance parameters relate to its scale. A query distribution need
not resemble the construction distribution, but that choice must be explicit.

For a real-world dataset, also specify source, redistribution/license terms,
immutable content digest, preprocessing, ordering, split selection and a
reliable acquisition/cache mechanism. These are requirements for a proposed
extension, not existing ingestion functionality. Link a core design discussion
before treating a local file path as reproducible input provenance.

## Implement the shared generator

Extend [the generator](../crates/spatial-bench-dataset/src/main.rs) and the
`dataset` values in [core vocabulary](../crates/spatial-bench-core/src/vocab.rs).
Keep point and query generation outside query timing. Existing semantics use
ChaCha8 seeded by a `u64`, with construction seed `s` and query seed
`s.wrapping_add(1)`. `f32` and `f64` consume their own streams; identical seeds
do not imply identical coordinates across scalar types. Gaussian calculations
use transcendental functions, so do not promise cross-platform byte identity
without checking it.

The local stream format is documented in the generator source: a 29-byte
header (`SBDS`, version 1, dimensions, dtype, construction/query counts), then
row-major construction coordinates and query coordinates, all in native
endianness. It is a same-host transport, not a portable archival format.
Preserving that shape generally avoids reader changes, but inspect each driver
for hardcoded kind/shape assumptions. Changing header, dtype or payload meaning
requires compatible updates to every affected reader and a stream version
change; changing `CaseSpec` also requires a harness compatibility decision.

Add a small deterministic regression check: verify identical output for repeated
arguments, the expected header and byte count, and distribution-specific
properties or known coordinates. Exercise both dtypes, odd coordinate counts
where relevant, and seed boundaries. Validation of invalid inputs belongs at
the generator/reader boundary; the existing permissive argument parser is not
a template for a new input format.

A runnable inspection of the current format, from a built core checkout:

```sh
spatial-bench-dataset --kind uniform --dims 3 --dtype f64 --tree-count 4 --query-count 2 --seed 42 > /tmp/spatial-bench-input.bin
python3 - <<'PY'
import struct
from pathlib import Path
raw = Path('/tmp/spatial-bench-input.bin').read_bytes()
assert raw[:4] == b'SBDS'
assert struct.unpack_from('=IIBQQ', raw, 4) == (1, 3, 1, 4, 2)
assert len(raw) == 29 + (4 + 2) * 3 * 8
assert all(0 <= x < 1 for x in struct.unpack_from('=18d', raw, 29))
PY
```

## Coordinate coverage and review

In the [bencher repository](https://github.com/spatial-bench/spatial-bench-benchers),
update affected readers, add manifest cases or a `matrix.dataset` axis, and
choose explicit entries in `corpus.toml` if the new workload belongs in the
Standard Corpus. A vocabulary value alone creates no cases. A new distribution
with unchanged transport may need only manifest changes there; verify this
with the actual drivers rather than assuming it.

Run core tests and the relevant bencher checks, inspect selectors with `list`,
then run one small case for each affected reader/language. Use a reference
check to establish that decoded construction/query data match the specification.
Include generator revision, seed, digest or deterministic fixture, decoded shape,
small-run command and result, and corpus rationale in the coordinated PRs.
`conform` is useful if compile-time registrations change but does not validate
input bytes or the distribution.

Update public [methodology](https://spatial-bench.org/methodology) and
[coverage](https://spatial-bench.org/coverage) where interpretation changes.
A new string tag using the existing result structure normally needs no results
schema migration; an external input identifier, hash, or new acquisition
contract may need result-schema, collation and web-reader changes. State those
dependencies and compatibility decisions in the review. Current run documents
do not retain the seed, so preserve it with the experiment command.
