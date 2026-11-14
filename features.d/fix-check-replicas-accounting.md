## Checking

| Replicas accounting counters | Supported: on a clean single-device filesystem, a replicas key's sectors are compared with the allocation btree's sectors for its data type; entries with no required copy or only the no-device marker expect zero. Other replicas entries retain their existing read-time checks | Unreleased (#497) | #497 | `tests/check_oracle.rs`, `tests/oracle_check_replicas_accounting.rs` |
