### Fixed

- **One damaged alloc node no longer makes every file unreadable.** Every
  data read checked its pointer's generation against the bucket's alloc
  key, so a torn alloc node failed every file though the data and its
  checksums were intact. When the alloc btree cannot be read, the
  generation now comes from the bucket_gens btree, and when that has none
  either, the extent's own data checksum stands in; an extent with no
  checksum is still refused (#167).
