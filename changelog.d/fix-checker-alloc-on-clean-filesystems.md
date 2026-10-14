### Fixed

- **The checker judges allocation information only on a filesystem shut
  down cleanly (#163).** After an unclean shutdown the allocation btrees
  can lag the rest of the filesystem, as the reference leaves them in its
  own crash images (a bucket's generation in bucket_gens one ahead of its
  alloc key), and the reference rebuilds them in recovery. The checks of
  bucket_gens, backpointers, data types, the fragmentation lru,
  need_discard and accounting reported such an image as damaged; they are
  now made only when the filesystem was shut down cleanly.
