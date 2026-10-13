### Fixed

- **A crafted reservation, whiteout or extent size no longer aborts the
  process.** A reservation or whiteout read as zeros through a buffer as
  large as its key, and an extent with no checksum entry was read into a
  buffer as large as the extent, both before anything bounded them; a key
  claiming up to 2 TiB aborted the process. Reservations and whiteouts now
  zero only the bytes a read asked for, and an extent whose stored sectors
  run past the end of the device is refused as corrupt (#151).
