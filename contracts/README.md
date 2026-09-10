# Sagent behavior contracts

These JSON fixtures specify repository-owned, observable behavior across crate boundaries.
They are not source snapshots and do not require Python or another runtime.

Every fixture has `contract_version`, `kind`, `input`, and `expected`.  Inputs must use fixed
IDs and fixture-only data. Results compare normalized JSON exactly: event order, error class,
roles, tool-call links, capabilities, and persistent facts are significant. Temporary paths,
timestamps, and generated IDs must be normalized by a fixture runner before comparison rather
than omitted from the behavior under test.

Run all fixtures with:

```text
cargo run -p sagent-contracts
```

Supported initial kinds are `rpc_hello`, `transcript`, `tool_registry`, `command_policy`,
`profile_name`, and `store_schema`. Add a runner branch and a fixture in the same change when a
new externally observable boundary is introduced.

