### Fixed

- **The checker finds xattr, back-reference and options-flag damage
  (#163).** `check::check` and `fsck.bcachefs` now report an xattr whose
  inode is gone (`key_in_missing_inode`), an xattr its name's hash slot
  cannot reach (`hash_table_key_wrong_offset`), an inode whose
  back-reference names no entry or another inode's
  (`inode_points_to_missing_dirent`, `inode_points_to_wrong_dirent`), and an
  inode whose has-options flag disagrees with its option fields
  (`inode_has_inode_opts_flag_wrong`). The reference checker found each of
  these on images where this one reported nothing.
