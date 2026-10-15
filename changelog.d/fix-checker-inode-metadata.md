### Fixed

- **The checker reports damaged inode metadata.** A directory whose size
  is not 0, an inode whose journal sequence is far past the newest the
  journal wrote, and an inode whose sector count is not what its extents
  add up to are reported rather than passed as clean.
