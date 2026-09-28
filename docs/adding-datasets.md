# Add a dataset

Generated inputs live in core, but they only reach a benchmark through driver
cases in the bencher catalog, so adding a distribution is usually a change to both
repositories. Today the `dataset` tag accepts `uniform` or `gaussian`, and the
chosen distribution applies to both the construction points and the query points.

## Define construction and query inputs

Start by writing down what the input actually represents: the coordinate scale,
the dimensions and scalar types it supports, and how the query set relates to the
construction set. If the distribution involves clustering, duplicate points or any
ordering that changes how a driver consumes it, those properties belong in the
definition too, along with the seeds that make it reproducible.

A fixed real-world dataset needs more thought before code. Record how the data is
acquired, under what licence, and how it is preprocessed and split into
construction and query points; a content digest should pin the exact bytes you
used. Be aware that the current generator takes distribution parameters rather
than an arbitrary input file, so importing a file is not an existing extension
point. It requires a new input and provenance contract, and should be discussed
before implementation.

## Extend the shared generator

Implement the distribution in
[spatial-bench-dataset](../crates/spatial-bench-dataset/src/main.rs) and register
its name in the `dataset` vocabulary in
[vocab.rs](../crates/spatial-bench-core/src/vocab.rs). Existing generation uses
ChaCha8, deriving the query seed as `s.wrapping_add(1)` from the construction seed
`s`. Because each scalar type consumes the stream differently, a byte-for-byte
comparison is only meaningful within one type, so say which type you checked. The
public [methodology](https://spatial-bench.org/methodology) documents the
distributions that already exist.

The generator source is also the definition of the local binary format: a 29-byte
header followed by construction and then query coordinates in native endianness.
Keep that layout when adding a distribution. If the header or payload has to
change, version the stream and update every reader that consumes it, and remember
that a change to `CaseSpec` also affects the harness contract.

With the development tools on `PATH`, you can inspect an existing input and check
your understanding of the format:

```sh
spatial-bench-dataset --kind uniform --dims 3 --dtype f64 \
  --tree-count 4 --query-count 2 --seed 42 > /tmp/spatial-bench-input.bin
python3 - <<'PYCODE'
import struct
from pathlib import Path
raw = Path('/tmp/spatial-bench-input.bin').read_bytes()
assert raw[:4] == b'SBDS'
assert struct.unpack_from('=IIBQQ', raw, 4) == (1, 3, 1, 4, 2)
assert len(raw) == 29 + 6 * 3 * 8
assert all(0 <= x < 1 for x in struct.unpack_from('=18d', raw, 29))
PYCODE
```

## Add driver coverage

On the bencher side, check whether existing readers assume a particular
distribution or shape, and add manifest cases for the combinations you support.
A single matrix entry such as `matrix.dataset` can expose several distributions at
once. If the new input belongs in the shared experiment, add an explicit selection
to `corpus.toml` as well.

## Validate and propose the change

Add a deterministic generator test that covers repeatability, the decoded shape
and the defining properties of the new distribution. Exercise every supported
scalar type, and test reader boundaries with short or invalid input. Floating-point
transcendental functions can differ between platforms, so scope any byte-identity
claim to the platforms you actually checked.

Run the core checks and one small benchmark for each affected driver or reader,
and include the seed, the expected input properties and the run commands in both
the core and bencher PRs. Update the public
[methodology](https://spatial-bench.org/methodology) and
[coverage](https://spatial-bench.org/coverage) pages. If new input identifiers or
digests need to persist in results, coordinate the schema, collation and web-reader
changes as well.