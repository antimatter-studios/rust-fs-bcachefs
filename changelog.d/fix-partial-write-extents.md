### Changed

- **A partial write rewrites only the extents it overlaps (#366).**
  `Writer::write_at`, `append` and `truncate` read the whole file into
  memory, freed every extent and wrote all of it back, so a 10-byte write
  into a large file cost the whole file in memory, I/O and fresh buckets.
  They now read, free and rewrite only the extents the write overlaps, and
  on a truncate down only those from the block the file now ends in;
  every other extent keeps its key, pointer and bucket.
- **A write or truncate past the device's size is written sparsely.** With
  nothing held whole, a one-byte write at 1 TiB or a truncate to 1 TiB on a
  small filesystem succeeds and leaves the range a hole, where it was
  refused (#196).
