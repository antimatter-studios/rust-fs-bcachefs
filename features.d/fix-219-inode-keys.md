## Checking

| `inode_v3` keys' hash type and fields_start | Supported: the checker reports `inode_str_hash_invalid` for a string hash type of 4 or more and `inode_fields_start_bad` for a fields_start below 6 or past the value's length in u64s | Unreleased (#434) | #219 | `tests/check_oracle.rs`, `src/check.rs` unit tests |
