### Fixed

- **A file whose inode claims an impossible size is refused instead of
  taking down the process.** Reading a file sized its buffer from the
  inode's size before reading anything: past `isize::MAX` that panicked, and
  past what memory holds the allocation failed and Rust aborted, which no
  caller and no C ABI guard can catch. A whole-file read now refuses a file
  that claims more than its device holds (it can only be mostly holes; read
  it in ranges), the buffer is reserved fallibly, and `fs_bcachefs_readlink`
  refuses a symlink target over 4096 bytes; the writer's partial writes are
  bounded the same way (#149).
