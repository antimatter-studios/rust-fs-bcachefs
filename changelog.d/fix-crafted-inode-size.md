### Fixed

- **A file whose inode claims an impossible size is refused instead of
  taking down the process.** Reading a file sized its buffer from the
  inode's size before reading anything: past `isize::MAX` that panicked, and
  past what memory holds the allocation failed and Rust aborted, which no
  caller and no C ABI guard can catch. The buffer is now reserved fallibly
  and an impossible size is an error, for the reader and for the writer's
  partial writes alike, and `fs_bcachefs_readlink` refuses a symlink target
  over 4096 bytes (#149).
