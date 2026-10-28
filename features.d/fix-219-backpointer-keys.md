## Checking

| Backpointer key snapshot and level | Supported: the checker reports `bkey_snapshot_nonzero` for a backpointer at a nonzero snapshot and `backpointer_level_bad` for one whose level is above 4, as the reference refuses them on read | Unreleased (#425) | #219 | `tests/check_oracle.rs` |
