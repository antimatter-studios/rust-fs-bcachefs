## Checking

| `alloc_v4` value-size fields | Supported: the checker reports `alloc_v4_val_size_bad` for a value shorter than 48 bytes or the start and backpointer count encoded in its flags, while accepting exact boundaries and trailing words | Unreleased (#482) | #452 | `tests/check_oracle.rs`, `tests/oracle_check_alloc_v4.rs`, `src/check.rs` unit tests |
