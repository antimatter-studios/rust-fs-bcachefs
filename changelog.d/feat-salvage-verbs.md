### Added

- **Salvage verbs on the primitive layer (`fs_bcachefs::salvage`).**
  `Salvage::open` needs one valid superblock copy and nothing else: no
  strict open, no readable root. It finds every inode in every btree node on
  the device, live or superseded by an older node version; walks a
  candidate's extents, reflinked data included; rebuilds the tree from every
  directory entry found, naming orphans and conflicting parents; reports
  damage; and materializes a file from a plan to a caller's writer, naming
  where every byte came from and reporting what does not decode as missing.
  Encrypted and multi-device volumes are refused by name.
