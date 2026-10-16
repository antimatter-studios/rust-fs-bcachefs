### Fixed

- **The checker reports damaged inode metadata.** A directory whose size
  is not 0, an inode whose journal sequence is far past the newest the
  journal wrote, and an inode whose sector count is not what its extents
  add up to are reported rather than passed as clean.
- **The checker reports a dirent away from its hash slot.** An entry that
  a lookup from its name's hash slot cannot reach, because an empty slot
  lies between, is reported as a hash table key at the wrong offset;
  entries displaced past a collision or a whiteout stay valid.
- **The checker reports a bucket_gens key with a size.** A bucket_gens
  key whose size is not 0 is reported as `bkey_size_nonzero`, as the
  reference checker refuses it.
