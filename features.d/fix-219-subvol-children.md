## Checking

| subvolume_children entries against their subvolumes | Supported: the checker reports `subvol_children_entry_wrong` for an entry at `parent:child` whose child is no subvolume or whose `fs_parent` is not `parent` | Unreleased (#418) | #219, #382 | `tests/check_oracle.rs` |
