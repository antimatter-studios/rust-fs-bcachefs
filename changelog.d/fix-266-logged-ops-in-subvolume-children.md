### Fixed

- **The checker reports a logged operation in the subvolume_children btree
  (#266).** A logged_op_truncate or logged_op_finsert key there was called
  clean, while the reference checker deletes it as an invalid key type. It
  is now reported as `bkey_invalid_type_for_btree`.
