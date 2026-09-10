# Sagent offline benchmarks

Run the benchmark harness without changing tracked data:

```text
cargo run -p sagent-benchmarks --release -- --messages 10000 --iterations 7
```

It uses a temporary SQLite database and a fixed offline fixture. It never calls a Provider or
reads `SAGENT_HOME`. The committed initial baseline uses 100 messages; P1.3 owns the separate
10k/100k bulk fixture and query-plan gate. `baseline.json` is only updated after an explicit
review:

```text
cargo run -p sagent-benchmarks --release -- --messages 10000 --iterations 7 --write-baseline
```

The emitted JSON reports medians and p95 values. Compare like-for-like OS, Rust version and
fixture size; absolute timings are not normal unit-test assertions.
