### Fixed

- **The checker reports a snapshot key in the backpointers btree (#378).** A
  snapshot key there was called clean, while the reference checker refuses
  it as an invalid key. It is now reported as `bkey_invalid_type_for_btree`,
  as an inode key there is (#268).
