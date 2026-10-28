### Fixed

- The checker reports `inode_v3` keys the reference refuses on read, where it called the image clean: a string hash type of 4 or more (`inode_str_hash_invalid`), and a fields_start below 6 or past the value's length in u64s (`inode_fields_start_bad`).
