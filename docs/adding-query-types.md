# Add a query type

Begin with an operation definition that each participating library can implement.
Use that definition to choose the query tag and driver calls.

## Define semantics and timing

Specify the metric and distance units, cardinality and ordering. For a radius
operation, define its boundary and whether the supplied threshold is a distance
or squared distance. Include behavior for empty results, duplicate points,
ties and fewer than `k` eligible points. Approximate queries need an explicit
error or recall criterion.

State whether the operation uses individual probes or a bulk API, its thread
policy, and reusable preprocessing. Mark which allocations, result traversal
and sorting belong inside timing. Define the denominator: a query loop divides
by probes executed, not neighbours returned. Construction/update operations
need their own timing boundary and units.

## Implement the operation

Add the query name to `UNIVERSAL` in
[vocab.rs](../crates/spatial-bench-core/src/vocab.rs). Reuse existing parameters
when their semantics fit. New parameters need compatible
[selection and result handling](reference.md#source-contracts).

Implement the operation in each intended bencher driver, then add its manifest
cases. Declare the supported dimensions, metrics and call shapes, marking axes
that require compile-time specialization. Follow the
[driver contract](https://github.com/spatial-bench/spatial-bench-benchers/blob/master/docs/driver-contract.md)
for transport and measurement.

## Check answers and measurements

Leave a runnable known-answer or reference check in the relevant driver's tests.
Use a tiny input with cases that distinguish your semantics: for example a point
exactly on the radius, tied distances and an empty result. Compare IDs,
distances and ordering while allowing every valid answer under the defined tie
rule. Check approximation against exact answers using its stated criterion.
Keep correctness checks outside the timer.

Run the core and driver checks, inspect selector expansion with `list`, and use
`conform --subject NAME` to verify compile-time registrations. Then run a
[small benchmark](running-benchmarks.md). Check its tags, units and normalization.
Conformance compares registrations; the reference test checks returned answers.

## Coordinate the changes

Link core vocabulary changes with the bencher implementation PRs. Update
`corpus.toml` when the operation belongs in the shared experiment, and describe
its semantics in the public [methodology](https://spatial-bench.org/methodology)
and [coverage](https://spatial-bench.org/coverage).

A new query under existing protocol fields does not by itself require a schema
or harness version bump. Changed field meanings or structure need a compatibility
decision for each affected contract. New metrics need result collation and chart
support. If correcting an existing query's meaning, identify affected historical
results with [results maintainers](https://github.com/spatial-bench/spatial-bench-results/blob/main/CONTRIBUTING.md).

Include the operation definition and reference-check command in the PR, along
with source revisions, the small-run command/seed and the normalization used.
