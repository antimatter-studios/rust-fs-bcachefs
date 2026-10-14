### Fixed

- **A file held by pieces of checksummed regions can be rewritten
  (#169).** Where the reference overwrites part of a file, it leaves
  extents whose checksum entry covers more than they do, and the writer
  refused to free them ("freeing part of a checksummed extent is not
  implemented"), so any write, truncate or unlink of such a file failed.
  A piece of an uncompressed region is now freed as the reference counts
  it: its live sectors from its bucket, and its backpointer at the
  region's sector plus its offset. A piece of a compressed region, which
  no image has shown, is still refused.
