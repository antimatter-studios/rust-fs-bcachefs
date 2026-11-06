## Checking

| A snapshot two subvolumes claim | Supported: on a clean image, at every format version, two subvolumes not read as deleted that name one snapshot are `snapshot_claimed_twice`, the reference's "is claimed by" | Unreleased (#536) | #521 | `tests/check_oracle_snapshot_claims.rs`, `src/check/snapshot_claims_tests.rs` |
