## Checking

| Snapshot reserved pad word | Supported: the checker reports `snapshot_pad_nonzero` for a nonzero bit in bytes 60..64 of a snapshot value, and for no other byte | Unreleased (#483) | #465 | `tests/check_oracle.rs`, `tests/oracle_check_key_field_bytes.rs`, `src/check.rs` unit tests |
| Subvolume state unset | Supported: the checker reports `subvol_state_bad` for a zero word at bytes 40..44 of a subvolume value, or a value of at least 16 bytes that ends before it, from format version 1.39. Other state words have a row of their own | Unreleased (#483) | #469 | `tests/check_oracle.rs`, `tests/oracle_check_key_field_bytes.rs`, `src/check.rs` unit tests |
| Dirent type byte | Supported: a dirent's type is the low five bits of its byte and bit 7 its casefolded layout. Bits 5 and 6 are not read | Unreleased (#483) | #473 | `tests/check_oracle.rs`, `tests/oracle_check_key_field_bytes.rs`, `src/inode.rs` unit test |
