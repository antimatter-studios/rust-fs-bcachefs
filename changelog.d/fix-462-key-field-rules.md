### Fixed

- **The checker names more keys the reference refuses (#462, #463, #464,
  #470).** A nonzero snapshot on `bucket_gens`, `subvolume_children` and
  `reflink_v` keys, a nonzero size on `need_discard` and `alloc_v4` keys, a
  subvolume or snapshot at offset `U64_MAX`, a snapshot at offset 0, a
  `reflink_v` key at a nonzero inode, a stray offset on `btree` and `snapshot`
  accounting keys, and a `reflink_p` whose index is below its `front_pad`.
