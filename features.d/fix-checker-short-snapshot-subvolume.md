## Checking

| Snapshot and subvolume fixed-field value sizes | Supported: snapshot values below 24 bytes and subvolume values below 16 bytes are reported as `bkey_val_size_too_small`, with the key's position and sizes | Unreleased (#522) | #506 | `tests/check_oracle.rs`, `tests/oracle_check_key_field_bytes.rs`, `src/check.rs` unit tests |
