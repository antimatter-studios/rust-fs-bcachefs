### Fixed

- **A write refused for space leaves the file as it was.** A commit writes
  each btree's change to disk as it goes, and data could take a filesystem's
  last free buckets: a later node split then found none, and a write that
  returned an error had already changed the file (on `block4k`, the last
  byte of a 10-byte file). Data now leaves 32 buckets free for btree nodes,
  so a write that would need them is refused before anything is written
  (#168).
