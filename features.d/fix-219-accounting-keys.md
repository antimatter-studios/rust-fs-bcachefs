## Checking

| `nr_inodes`, `replicas` and `dev_data_type` accounting keys' own fields | Supported: the checker reports `accounting_key_junk_at_end` for a nonzero position byte after the kind's fields, `accounting_replicas_bad` for a replicas entry with no devices, more copies required than devices or devices not in increasing order, and `accounting_key_counters_bad` for more than three counters or none (`nr_inodes`, `replicas`) | Unreleased (#421) | #219 | `tests/check_oracle.rs`, `src/check.rs` unit tests |
