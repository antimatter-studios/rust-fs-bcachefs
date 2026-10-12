### Fixed

- **The checker reports an extent far past the end of its file.** It
  compared an extent's start with its file's size in bytes, and a start 2^55
  sectors in overflowed: a panic with overflow checks on, and in a release
  build a wrap to 0 that judged the extent inside the file. It compares in
  sectors now (#150).
