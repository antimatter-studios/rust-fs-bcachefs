### Fixed

- **The checker reports three more key fields the reference refuses
  (#219).** A snapshot key or a subvolume_children key with a size is
  reported as `bkey_size_nonzero`, and a snapshot_tree key at a snapshot
  as `bkey_snapshot_nonzero`, where the image had been called clean.
