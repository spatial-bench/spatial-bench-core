# Add a query type

Every query type begins as an operation definition that each participating library
is able to implement. Settle that definition first, because it determines the
vocabulary tag the query is filed under and the shape of the calls each driver
makes.

## Define semantics and timing

The definition needs to cover the metric and its distance units, how many results
are returned and in what order. For a radius operation, state the boundary
convention and whether the supplied threshold is a distance or a squared distance.
Say what happens when the result is empty, when points are duplicated or tied, and
when fewer than `k` points are eligible. Approximate queries additionally need an
explicit error or recall criterion.

Timing is part of the definition, not an implementation afterthought. Decide
whether the operation is expressed as individual probes or a bulk API call, what
its thread policy is, and whether any preprocessing can be reused across queries.
Then mark exactly which costs fall inside the timed region: allocations, result
traversal and sorting in particular. The denominator matters too: a query loop
divides by the probes it executed, never by the number of neighbours returned.
Construction and update operations need their own timing boundary and units
rather than reusing the query one.

## Implement the operation

Add the query name to `UNIVERSAL` in
[vocab.rs](../crates/spatial-bench-core/src/vocab.rs). Reuse existing parameters
wherever their semantics already fit; genuinely new parameters have to be
compatible with [selection and result handling](reference.md#source-contracts).

Implement the operation in each bencher driver that should support it, then add the
corresponding manifest cases. Those cases declare the supported dimensions,
metrics and call shapes, and must mark any axis that requires compile-time
specialization. The
[driver contract](https://github.com/spatial-bench/spatial-bench-benchers/blob/master/docs/driver-contract.md)
defines the transport and measurement rules to follow.

## Check answers and measurements

Leave a runnable known-answer or reference check in the relevant driver's tests.
A tiny input is enough if it distinguishes your semantics: a point exactly on the
radius boundary, tied distances, and an empty result. Compare identifiers,
distances and ordering, allowing for every answer that is valid under your stated
tie rule, and check approximation against exact answers using its own criterion.
Keep all of this outside the timer.

Run the core and driver checks, inspect the selector expansion with `list`, and use
`conform --subject NAME` to confirm the compile-time registrations match the
manifest. Then run a [small benchmark](running-benchmarks.md) and check its tags,
units and normalization. The two kinds of check answer different questions:
conformance verifies that registrations line up, while the reference test verifies
that the queries return correct answers.

## Coordinate the changes

Core vocabulary changes and bencher implementations have to land together, so link
the PRs. Update `corpus.toml` if the operation belongs in the shared experiment,
and describe the semantics on the public
[methodology](https://spatial-bench.org/methodology) and
[coverage](https://spatial-bench.org/coverage) pages.

A new query that fits within the existing protocol fields does not by itself
require a schema or harness version bump. Changing the meaning or structure of an
existing field does, and each affected contract needs its own compatibility
decision. New metrics also require result-collation and chart support. If you are
correcting the meaning of an existing query, work with
[results maintainers](https://github.com/spatial-bench/spatial-bench-results/blob/main/CONTRIBUTING.md)
to identify the historical results that are affected.

Include the operation definition and the reference-check command in the PR, along
with the source revisions, the small-run command and seed, and the normalization
you applied.