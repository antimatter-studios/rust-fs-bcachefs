## Checking

| `fsck.bcachefs` on a backpointer moved within its extent's sector, on every btree's own accounting key against its nodes, and on the data sectors of a per-snapshot counter over inline_data and dirent keys | Partial: a missing accounting key is judged for logged_ops and snapshot_trees only, and data sectors of other key types are not judged | Unreleased (#743) | #741 | `oracle_check_nightly_accounting.rs`, `check.rs` unit tests |
