### Fixed

- **Removing a file removes its extended attributes.** Unlinking a file's
  last name, removing a directory, or renaming over a file deleted the inode
  and freed its data but left its xattr keys behind, which the reference
  checker reports as `key_in_missing_inode`. They go in the same transaction
  as the inode now (#161).
