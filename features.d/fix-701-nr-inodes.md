## Checking

| `fsck.bcachefs` on the inode count: a cleanly shut down filesystem whose `nr_inodes` counter, or its absence, disagrees with the inodes btree is reported as `accounting_mismatch` | Supported | Unreleased (#732) | #701 | `check.rs` unit test, `oracle_check_inode_count.rs`, `check_oracle.rs` |
