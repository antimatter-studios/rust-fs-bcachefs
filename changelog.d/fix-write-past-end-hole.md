### Fixed

- **A write past a file's end leaves the gap a hole (#156).** `Writer::write_at`
  stored the range between a file's old end and a write that starts a block or
  more past it as allocated zeros, because the file was rewritten whole. Now
  only the old data's blocks and the blocks the write covers are allocated,
  and the gap reads as zeros because nothing is stored there, as after a
  truncate up.
