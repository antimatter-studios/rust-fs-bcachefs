### Fixed

- **Truncating a file up leaves the new range a hole (#156).** A truncate
  up rewrote the file with the added range as allocated zeros, so an
  empty file truncated to 1 MiB took a MiB of buckets, and a sparse file
  lost its holes. Only the file's last block is rewritten now, zero-filled
  past the old end, and the size set; the added range has no extent and
  reads as zeros. A write past the end still writes the gap as zeros.
