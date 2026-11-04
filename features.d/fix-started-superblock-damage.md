## Checking

| Superblock damage on a started filesystem | Supported: a superblock the reference refuses on a started filesystem is reported under `invalid_sb_*` and `device_size_too_small`, and nothing past it is read. What the reference finds only in a pass over the filesystem from a superblock it accepts is open: a layout that disagrees with the buckets (#553), the btree bitmap (#554), and `metadata_replicas` above what one device holds (#555) | Unreleased (#556) | #533 | `tests/check_oracle_started_superblock.rs`, `src/check/started_superblock_tests.rs` |
