### Fixed

- **The checker reports an inode key in the backpointers btree (#268).** An
  inode_v3 key there was called clean, while the reference checker deletes
  it as an invalid key type. It is now reported as
  `bkey_invalid_type_for_btree`.
