### Fixed

- **A superblock root that will not read is replaced by the journal's
  (#166).** The journal records every btree root again, and on every clean
  fixture it records the same roots as the superblock's clean field. The
  reference opened images whose clean-field root had been damaged, and so
  does this reader now: when a root's key does not decode, its node does not
  read, or the node belongs to another btree or level, `Filesystem::open`
  and the checker read the filesystem through its journal, as after an
  unclean shutdown, instead of refusing it or failing that btree.
