# Sagent offline benchmarks

Run the benchmark harness without changing tracked data:

```text
cargo run -p sagent-benchmarks --release -- --messages 10000 --iterations 7
```

It uses a temporary SQLite database and a fixed offline fixture. It never calls a Provider or
reads `SAGENT_HOME`. Fixture creation is one SQLite transaction, so the reported list/search
latency excludes per-row commit cost. The JSON includes an `fts_uses_virtual_table_index` query
plan check; a missing FTS5 index fails rather than silently producing an incomparable baseline.

Run both P1.3 scale checks before publishing a performance decision:

```text
cargo run -p sagent-benchmarks --release -- --messages 10000 --iterations 7
cargo run -p sagent-benchmarks --release -- --messages 100000 --iterations 7
```

The committed initial baseline uses 100 messages. `baseline.json` is only updated after an explicit
review:

```text
cargo run -p sagent-benchmarks --release -- --messages 10000 --iterations 7 --write-baseline
```

The emitted JSON reports medians and p95 values. Compare like-for-like OS, Rust version and
fixture size; absolute timings are not normal unit-test assertions.
