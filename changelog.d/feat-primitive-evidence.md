### Added

- **A primitive layer that reports damage instead of refusing it
  (`fs_bcachefs::primitive`).** It reads every superblock copy, any btree
  node, every node on the device (older versions the live tree no longer
  reaches included) and every journal entry, each with a verdict: valid, bad
  magic, bad checksum, corrupt, unsupported or unreadable. A node with one
  bad bset keeps every key that still decodes, and a torn journal entry is
  listed and its bucket read on past it. Everything takes the source as
  `&dyn BlockRead`, so nothing in it can write.
