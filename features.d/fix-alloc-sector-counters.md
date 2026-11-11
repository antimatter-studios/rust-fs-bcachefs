## Checking

| Alloc key dirty and cached sector counts against what the bucket holds, on clean single-device filesystems | Supported: the checker reports `alloc_key_dirty_sectors_wrong` and `alloc_key_cached_sectors_wrong`. A bucket holding a pointer with an unobserved flag is not judged | Unreleased (#560) | #380 | `tests/check_oracle.rs`, `tests/oracle_check_alloc.rs`, `tests/write_oracle.rs` |
