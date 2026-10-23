### Fixed

- **A snapshot tree whose root snapshot is missing or belongs to another
  tree is reported (#267).** The checker accepted such a tree; the
  reference refuses it as pointing to a missing or incorrect snapshot. It
  is now named as `snapshot_tree_to_bad_root_snapshot`.
