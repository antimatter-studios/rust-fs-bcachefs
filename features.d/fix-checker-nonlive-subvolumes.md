## Checking

| Subvolume state against the filesystem | Supported: unlinked and deleted subvolumes cannot satisfy root or dirent references; deleted subvolumes cannot satisfy live snapshot, snapshot-tree-master or subvolume-children references. A consistent unreferenced deleted subvolume is accepted. Root, snapshot and dirent state checks apply to clean images from version 1.39 | Unreleased (#524) | #507 | `tests/check_oracle.rs`, `tests/oracle_subvolume_states.rs`, `src/check.rs` unit tests |
