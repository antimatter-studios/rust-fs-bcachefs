### Fixed

- **The checker reports a key at a position, or with value lengths, its
  btree does not allow (#219).** A subvolume not at `0:id` with an id above
  0, a snapshot_tree at offset 0, an accounting key of the `btree` kind with
  bits set below its btree id or at a snapshot, and an xattr whose name and
  value lengths run past its value were reported clean; the reference
  checker refuses each of them. They are now reported as `subvol_pos_bad`,
  `snapshot_tree_pos_bad`, `accounting_key_junk_at_end` and
  `xattr_val_size_too_small`.
