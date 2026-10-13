### Fixed

- **A write or truncate past the device's size no longer aborts the
  process.** The writer rewrites a file whole and held the new size in
  memory before checking it, so a truncate to 1 TiB, or a one-byte write
  at 1 TiB, asked for a terabyte and aborted the caller, C ABI hosts
  included. A new size larger than the device is now refused before
  anything is allocated (#156).
