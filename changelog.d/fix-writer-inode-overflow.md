### Fixed

- The writer refuses an inode allocation cursor at `u64::MAX` instead of overflowing while advancing it.
