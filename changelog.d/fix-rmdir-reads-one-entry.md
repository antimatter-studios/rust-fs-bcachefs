### Fixed

- **Removing a directory reads up to its first entry, not all of them.**
  The check that a directory is empty read every dirents key it held, so
  refusing to remove a large one read its every leaf; it now stops at the
  first live entry (#193).
