## Checking

| Per-snapshot btree accounting counters | Supported: the checker reports `accounting_mismatch` when a btree's key count or unpacked bytes in a snapshot disagree with its keys, on clean filesystems; the btree is read as its 32-bit field, and a counter whose btree id is past a byte is held to zero | Unreleased (#414, #610) | #163 | `tests/check_oracle.rs`, `src/check.rs` unit tests |
